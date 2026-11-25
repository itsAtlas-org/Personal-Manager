// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Uninstalling the Pinboard keeps its notes (Mods spec D9; PR C, K5b).
//!
//! The Pinboard's un-ingested notes live in the mod's own storage, which goes with the mod, so the
//! uninstall dialog offers to keep them first. Three commands, each one step the page drives in turn
//! (the page owns the order and the Cancel button; Rust owns what each step may do):
//!
//! 1. [`plan`] reads the board the Pinboard would load now ([`pinboard_legacy::stored_board`]: the
//!    mod's board after the move, else `settings['pinboard']`) and sorts its notes: which still need
//!    saving, which are saved already, which can never be documents.
//! 2. [`save_one`] saves ONE note as a vault document (`notes::save_as`, the legacy `note:<key>` name
//!    every earlier note has, landing already reviewed so N saved notes are not N items to review),
//!    then reads its document row back. It returns only when the row is there. The page calls it note
//!    by note, so there is progress and a Cancel that stops between notes, and **no DB guard is held
//!    across a note**: each step takes the guard for one read and lets go, and the embed holds none.
//! 3. [`export_into`] is the way out when the document engine is not set up: one `.md` file per note,
//!    and the whole board's exact bytes as `pinboard-board-<date>.json`, in a folder the person picks
//!    (picked here, in the backend, so the page never names the path).
//!
//! The uninstall comes last and only when every step before it answered ok: a failure stops it with
//! nothing deleted, and a retry saves nothing twice (a note whose document is already current is
//! skipped, never re-embedded).
//!
//! **Untrusted text.** The board may come from a shared vault: a note's title and text are data. They
//! are never put in an error message or a log line; the page shows a title as plain text, clamped.
//!
//! **What the saved document holds.** The card's own text, exactly as the board stores it, not the
//! Markdown the Pinboard renders from its shorthand when a person presses Ingest (the renderer lives in
//! the page code that leaves core with the Pinboard). A note that is already a document and that the
//! board says is unchanged ([`LegacyNote::recorded_current`]) is therefore never rewritten from the
//! card's text: that would replace a rendered document with its shorthand.

use std::collections::HashSet;
use std::io::Write;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use super::error::{Code, ModsError};
use crate::ingest::{note_is_current, valid_note_id, SOURCE_TYPE_VAULT};
use crate::mods::api::limits::{NOTE_KEY_MAX_LEGACY, NOTE_TITLE_MAX_CHARS};
use crate::mods::ids::ModId;
use crate::notes::{self, Arrival, Namespace};
use crate::pinboard_legacy::{self, BoardSource, LegacyNote, PINBOARD_MOD_ID};
use crate::AppState;

/// The bytes of a file name's title part and key part: well under any file system's 255.
const NAME_PART_MAX_BYTES: usize = 100;
const KEY_PART_CHARS: usize = 8;

/// A note as the dialog lists it. `title` is for display only (clamped, one line).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NoteRef {
    pub key: String,
    pub title: String,
}

/// Where the board came from, for the dialog's sentence about the old copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// The mod's own board (it has moved).
    Mod,
    /// The board in PM's own store (it has not).
    Legacy,
}

/// What the dialog offers for the Pinboard's notes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotesPlan {
    /// Notes with some text: every one worth keeping.
    pub total: u64,
    /// The notes to save now, in board order: no document yet, or edited since the last one.
    pub to_save: Vec<NoteRef>,
    /// Saved as documents already and unchanged since: skipped, never embedded again.
    pub already_saved: u64,
    /// Notes that cannot be documents (an id that is not a document name, one that Chat's saved
    /// answers use, a second card with the same id). The whole-board file and the `.md` export
    /// keep them.
    pub unsaveable: u64,
    /// Whether PM's document engine is set up. Saving as documents needs it; the `.md` export does not.
    pub engine_ready: bool,
    /// The stored board's size, 0 when there is none.
    pub board_bytes: u64,
    pub source: Source,
}

// ---- Which mod, which notes ---------------------------------------------------------------------

/// Whether `id` is the first-party Pinboard as installed (signed, tier First). An unsigned folder mod
/// that calls itself `atlas.pinboard` has no notes of its own by the old names, and is offered nothing.
pub fn is_pinboard(conn: &Connection, id: &ModId) -> rusqlite::Result<bool> {
    if id.as_str() != PINBOARD_MOD_ID {
        return Ok(false);
    }
    Ok(conn
        .query_row(
            "SELECT signed = 1 AND tier = 'first' FROM mods WHERE id = ?1",
            [id.as_str()],
            |r| r.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false))
}

/// Whether a card's id can name a note document: the legacy rules (letters, digits, `-` and `_`, at
/// most 200), and not Chat's (`chat-` is where Chat saves its answers, and a board planted in a shared
/// vault must not be able to touch one).
fn can_be_document(key: &str) -> bool {
    valid_note_id(key) && key.len() <= NOTE_KEY_MAX_LEGACY && !key.starts_with("chat-")
}

/// One line of at most [`NOTE_TITLE_MAX_CHARS`] characters: every run of whitespace or control
/// characters is one space.
fn one_line(raw: &str) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    for c in raw.chars() {
        if c.is_whitespace() || c.is_control() {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(c);
        }
        if out.chars().count() >= NOTE_TITLE_MAX_CHARS {
            break;
        }
    }
    out.chars().take(NOTE_TITLE_MAX_CHARS).collect()
}

/// The title a note is known by: the card's own, else its first line (what Ingest does).
pub fn note_title(note: &LegacyNote) -> String {
    let own = one_line(&note.title);
    if !own.is_empty() {
        return own;
    }
    one_line(&crate::commands::derive_title(note.text.trim()))
}

fn expected_hash(key: &str, body: &str) -> String {
    crate::index_only::pointer_content_hash(&Namespace::Legacy.source_id(key), body)
}

/// A note's document row: its source type, content hash and title.
fn stored_row(
    conn: &Connection,
    key: &str,
) -> rusqlite::Result<Option<(i64, String, String, String)>> {
    conn.query_row(
        "SELECT id, source_type, content_hash, title FROM documents WHERE source_id = ?1",
        [Namespace::Legacy.source_id(key)],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .optional()
}

/// The document that already holds exactly this note, if there is one: the same text under the same
/// title ([`note_is_current`], the rule `ingest_note_document` itself returns early on), or a vault
/// document the board records as made from this very text.
fn current_document(conn: &Connection, note: &LegacyNote) -> rusqlite::Result<Option<i64>> {
    let Some((id, source_type, hash, title)) = stored_row(conn, &note.key)? else {
        return Ok(None);
    };
    let same = note_is_current(
        &source_type,
        &hash,
        &title,
        &expected_hash(&note.key, note.text.trim()),
        &note_title(note),
    );
    let recorded = note.recorded_current && source_type == SOURCE_TYPE_VAULT;
    Ok((same || recorded).then_some(id))
}

/// Sorted notes: what still needs saving, and the counts of the rest.
#[derive(Debug, Default)]
pub struct Sorted {
    pub total: u64,
    pub to_save: Vec<LegacyNote>,
    pub already_saved: u64,
    pub unsaveable: u64,
}

/// Sort a board's notes against the store. One short read of the guard's connection; nothing is
/// saved here. A note with no text is not a note worth keeping and is not counted. A key already
/// seen is a second card with the same id: only the first is a document.
pub fn sort_notes(conn: &Connection, notes: Vec<LegacyNote>) -> crate::error::Result<Sorted> {
    let mut sorted = Sorted::default();
    let mut seen = HashSet::new();
    for note in notes {
        if note.text.trim().is_empty() {
            continue;
        }
        sorted.total += 1;
        if !can_be_document(&note.key) || !seen.insert(note.key.clone()) {
            sorted.unsaveable += 1;
        } else if current_document(conn, &note)?.is_some() {
            sorted.already_saved += 1;
        } else {
            sorted.to_save.push(note);
        }
    }
    Ok(sorted)
}

/// What the dialog offers, or `None` when `id` is not the first-party Pinboard. Takes the store's
/// guard for the reads and gives it back before it returns.
pub fn plan(state: &AppState, id: &ModId) -> crate::error::Result<Option<NotesPlan>> {
    let (sorted, board_bytes, source) = {
        let conn = state.conn()?;
        if !is_pinboard(&conn, id)? {
            return Ok(None);
        }
        let board = pinboard_legacy::stored_board(&conn)?;
        let board_bytes = board.as_ref().map_or(0, |b| b.bytes.len() as u64);
        let source = match board.as_ref().map(|b| b.source) {
            Some(BoardSource::Mod) => Source::Mod,
            _ => Source::Legacy,
        };
        let notes = board
            .map(|b| pinboard_legacy::pinboard_notes(&b.text()))
            .unwrap_or_default();
        (sort_notes(&conn, notes)?, board_bytes, source)
    };
    Ok(Some(NotesPlan {
        total: sorted.total,
        to_save: sorted
            .to_save
            .iter()
            .map(|n| NoteRef {
                key: n.key.clone(),
                title: note_title(n),
            })
            .collect(),
        already_saved: sorted.already_saved,
        unsaveable: sorted.unsaveable,
        engine_ready: state.sidecar.is_ready(),
        board_bytes,
        source,
    }))
}

// ---- Saving one note ----------------------------------------------------------------------------

/// How one note ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    /// A document was made or rewritten.
    Saved,
    /// The note was a document already, unchanged; nothing was embedded.
    Unchanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteSaved {
    pub outcome: Outcome,
    pub document_id: i64,
}

/// The embed and the vault write, the one step a test replaces. It takes the DB guard itself, briefly,
/// several times, and is called with none held.
pub trait Embed {
    /// Whether the document engine is set up (it is never set up from here).
    fn engine_ready(&self) -> bool;
    /// Save the note as its document and answer the document's id.
    fn save(&self, key: &str, title: &str, body: &str) -> crate::error::Result<i64>;
}

/// The real thing: `notes::save_as` in the legacy namespace, landing reviewed.
pub struct Engine<'a>(pub &'a AppState);

impl Embed for Engine<'_> {
    fn engine_ready(&self) -> bool {
        self.0.sidecar.is_ready()
    }

    fn save(&self, key: &str, title: &str, body: &str) -> crate::error::Result<i64> {
        notes::save_as(
            self.0,
            &Namespace::Legacy,
            Arrival::Reviewed,
            key,
            title,
            body,
        )
        .map(|document| document.id)
    }
}

/// Why a note was not saved. Every sentence is fixed: no title, no text, no key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NotThePinboard,
    NoSuchNote,
    NotADocument,
    EngineNotReady,
    Failed,
    NotConfirmed,
}

impl Refusal {
    pub fn words(self) -> &'static str {
        match self {
            Self::NotThePinboard => "Only the Pinboard has notes to keep.",
            Self::NoSuchNote => "That note is not on the board any more.",
            Self::NotADocument => "That note cannot be saved as a document.",
            Self::EngineNotReady => {
                "PM's document engine isn't set up, so notes can't be saved as documents."
            }
            Self::Failed => "PM couldn't save this note as a document.",
            Self::NotConfirmed => "PM couldn't confirm that this note was saved as a document.",
        }
    }
}

impl From<Refusal> for ModsError {
    fn from(r: Refusal) -> Self {
        let code = match r {
            Refusal::NotThePinboard | Refusal::NoSuchNote | Refusal::NotADocument => {
                Code::BadRequest
            }
            Refusal::EngineNotReady => Code::NotAllowed,
            Refusal::Failed | Refusal::NotConfirmed => Code::Internal,
        };
        ModsError::new(code, r.words())
    }
}

/// Whether the store holds the note's document, made from exactly `body` under `title`, and it is
/// the document `id` the save said it made. A save that answered ok must pass this: an uninstall that
/// goes ahead on a document that is not there deletes the only copy of the note.
fn reads_back(conn: &Connection, key: &str, id: i64, title: &str, body: &str) -> bool {
    matches!(
        stored_row(conn, key),
        Ok(Some((stored_id, source_type, hash, stored_title)))
            if stored_id == id
                && note_is_current(&source_type, &hash, &stored_title, &expected_hash(key, body), title)
    )
}

/// Save the note `key` of the stored board as its document, and read the document back.
///
/// The board is read afresh here (the page names only the key), so what is saved is what the board
/// holds now. A note already saved and unchanged answers [`Outcome::Unchanged`] without the engine
/// and without an embed. **No guard is held while the embed runs**, and none across notes: the guard
/// is taken for the read, dropped, taken again for the read-back.
pub fn save_one(
    state: &AppState,
    embed: &impl Embed,
    id: &ModId,
    key: &str,
) -> Result<NoteSaved, Refusal> {
    let note = {
        let conn = state.conn().map_err(|_| Refusal::Failed)?;
        if !is_pinboard(&conn, id).map_err(|_| Refusal::Failed)? {
            return Err(Refusal::NotThePinboard);
        }
        let board = pinboard_legacy::stored_board(&conn).map_err(|_| Refusal::Failed)?;
        let found = board
            .map(|b| pinboard_legacy::pinboard_notes(&b.text()))
            .unwrap_or_default()
            .into_iter()
            // The first card of that id WITH text: the one `sort_notes` counted (it skips a blank card
            // before it looks for a repeated id), so a plan and a save always name the same card.
            .find(|n| n.key == key && !n.text.trim().is_empty())
            .ok_or(Refusal::NoSuchNote)?;
        if !can_be_document(&found.key) {
            return Err(Refusal::NotADocument);
        }
        if let Some(document_id) = current_document(&conn, &found).map_err(|_| Refusal::Failed)? {
            return Ok(NoteSaved {
                outcome: Outcome::Unchanged,
                document_id,
            });
        }
        found
    };
    if !embed.engine_ready() {
        return Err(Refusal::EngineNotReady);
    }
    let (title, body) = (note_title(&note), note.text.trim());
    let document_id = embed.save(&note.key, &title, body).map_err(|_| {
        eprintln!("mods: a Pinboard note could not be saved as a document");
        Refusal::Failed
    })?;
    let conn = state.conn().map_err(|_| Refusal::NotConfirmed)?;
    if !reads_back(&conn, &note.key, document_id, &title, body) {
        return Err(Refusal::NotConfirmed);
    }
    Ok(NoteSaved {
        outcome: Outcome::Saved,
        document_id,
    })
}

// ---- Files in a folder --------------------------------------------------------------------------

/// What an export wrote, and where it stopped if it did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOutcome {
    /// The folder the person picked, for the dialog to say.
    pub folder: String,
    /// `.md` files written.
    pub notes_written: u64,
    /// The whole-board file's name, when it was asked for and written.
    pub board_file: Option<String>,
    /// The first thing that could not be written. Nothing after it was tried.
    pub failed: Option<ExportFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportFailure {
    /// The position of the note that could not be written among the notes with text (0 first), or
    /// `None` when it was the whole-board file.
    pub note_index: Option<u64>,
    pub message: String,
}

/// A piece of a file name: letters and digits of any script, spaces, `-` and `_`; every other
/// character (a dot, a slash, a colon, a control or direction character) is dropped, so there is no
/// `..`, no separator and no hidden name. Runs of spaces and dashes are one, the ends are trimmed, and
/// it is cut to `max_bytes` without splitting a character.
fn name_part(raw: &str, max_bytes: usize, max_chars: usize) -> String {
    let mut out = String::new();
    let mut chars = 0;
    for c in raw.chars() {
        let keep = if c.is_alphanumeric() || c == '-' || c == '_' {
            Some(c)
        } else if c.is_whitespace() {
            Some(' ')
        } else {
            None
        };
        let Some(c) = keep else { continue };
        let separator = !c.is_alphanumeric();
        if separator && out.chars().last().is_none_or(|l| !l.is_alphanumeric()) {
            continue;
        }
        if out.len() + c.len_utf8() > max_bytes || chars >= max_chars {
            break;
        }
        out.push(c);
        chars += 1;
    }
    out.trim_matches(|c: char| !c.is_alphanumeric()).to_owned()
}

/// Whether `stem` is a name Windows treats as a device whatever its extension: `NUL.md` opens the
/// null device, so a "file" written there goes nowhere and the note would be lost with the uninstall.
/// `CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9` and `LPT1`-`LPT9` (also with a superscript digit),
/// in any case. A stem with a title and key part (`NUL-ab12cd34`) is not one.
fn is_device_name(stem: &str) -> bool {
    let upper = stem.trim().to_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    let mut chars = upper.chars();
    let head: String = chars.by_ref().take(3).collect();
    matches!(head.as_str(), "COM" | "LPT")
        && matches!(
            (chars.next(), chars.next()),
            (Some(d), None) if d.is_ascii_digit() || matches!(d, '\u{B9}' | '\u{B2}' | '\u{B3}')
        )
}

/// `<title or Untitled>-<first 8 of the key>`, without the extension. A name that would be a Windows
/// device on its own gets an underscore in front, so it is always a file.
pub fn export_stem(title: &str, key: &str) -> String {
    let title = name_part(title, NAME_PART_MAX_BYTES, 60);
    let title = if title.is_empty() {
        "Untitled".to_owned()
    } else {
        title
    };
    let key = name_part(key, NAME_PART_MAX_BYTES, KEY_PART_CHARS);
    let stem = if key.is_empty() {
        title
    } else {
        format!("{title}-{key}")
    };
    if is_device_name(&stem) {
        format!("_{stem}")
    } else {
        stem
    }
}

/// Create `<stem>.<ext>` in `folder`, or `<stem> (2).<ext>` and on, whichever is free: a file that is
/// there is never opened for writing (`create_new` is the check and the creation in one step). Gives
/// the open file and its name.
fn create_unique(folder: &Path, stem: &str, ext: &str) -> std::io::Result<(std::fs::File, String)> {
    for n in 1..=999 {
        let name = if n == 1 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem} ({n}).{ext}")
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(folder.join(&name))
        {
            Ok(file) => return Ok((file, name)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "no free name",
    ))
}

/// Write `bytes` as a new file in `folder`. A file that fails part-way is removed, so no truncated
/// note is left looking like a whole one.
fn write_new(folder: &Path, stem: &str, ext: &str, bytes: &[u8]) -> std::io::Result<String> {
    let (mut file, name) = create_unique(folder, stem, ext)?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    if let Err(e) = written {
        drop(file);
        let _ = std::fs::remove_file(folder.join(&name));
        return Err(e);
    }
    Ok(name)
}

fn why(e: &std::io::Error) -> &'static str {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => "PM isn't allowed to write in that folder.",
        std::io::ErrorKind::StorageFull => "There is no room left there.",
        std::io::ErrorKind::NotFound => "That folder is not there any more.",
        _ => "PM couldn't write the file.",
    }
}

/// The notes with text, as the export writes them.
pub fn exportable(notes: Vec<LegacyNote>) -> Vec<LegacyNote> {
    notes
        .into_iter()
        .filter(|n| !n.text.trim().is_empty())
        .collect()
}

/// Write the notes (when `notes` is `Some`) and the whole board (when `board` is `Some`) into
/// `folder`: one `<title>-<key>.md` per note, holding the card's text exactly, then
/// `pinboard-board-<date>.json`, the board's exact bytes. Nothing already there is overwritten, and
/// the first failure stops the export. Blocking file work, no store.
pub fn export_into(
    folder: &Path,
    date: &str,
    notes: Option<&[LegacyNote]>,
    board: Option<&[u8]>,
) -> ExportOutcome {
    let mut out = ExportOutcome {
        folder: folder.to_string_lossy().into_owned(),
        notes_written: 0,
        board_file: None,
        failed: None,
    };
    for (i, note) in notes.unwrap_or_default().iter().enumerate() {
        let stem = export_stem(&one_line(&note.title), &note.key);
        match write_new(folder, &stem, "md", note.text.as_bytes()) {
            Ok(_) => out.notes_written += 1,
            Err(e) => {
                out.failed = Some(ExportFailure {
                    note_index: Some(i as u64),
                    message: why(&e).to_owned(),
                });
                return out;
            }
        }
    }
    if let Some(bytes) = board {
        let stem = format!("pinboard-board-{}", name_part(date, 10, 10));
        match write_new(folder, &stem, "json", bytes) {
            Ok(name) => out.board_file = Some(name),
            Err(e) => {
                out.failed = Some(ExportFailure {
                    note_index: None,
                    message: why(&e).to_owned(),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
