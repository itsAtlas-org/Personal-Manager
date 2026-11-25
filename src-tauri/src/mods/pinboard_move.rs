// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Pinboard's one-time move from `settings['pinboard']` into the mod's own storage (PR C, K8):
//! the pilot handover of `atlas.pinboard`, run in the install's transaction through
//! `autoinstall::finish` (an auto-install) or `autoinstall::finish_manual` (a click in the Mods tab).
//!
//! **DORMANT in C1.** The move acts only when `pinboard_move_pending` is in the store, and nothing
//! in C1 writes it (C2's migration v62 does, from the board's content). The core Pinboard stays the
//! live one.
//!
//! **What must hold** (Bobby: "be very careful with the pinboard, live data, additive, don't remove
//! anything"; the board is a person's own words and no copy of it may be lost):
//!
//! - **Nothing here writes or deletes `settings['pinboard']`.** The old row is the only pre-mod copy
//!   and it stays exactly as it was (the later cleanup is its own change, on Bobby's word). The tests
//!   compare its bytes and its rowid before and after every path.
//! - **The bytes cross unchanged.** The normal path parses nothing and rewrites nothing: the legacy
//!   text is bound as it is, class `content`, revision 1. It is read back in the same transaction and
//!   its bytes and sha256 compared; a mismatch is an `Err`, the install rolls back with the marker,
//!   and the next pass tries again, so the mod never exists without its board. (The spec says copy,
//!   commit, re-read after commit. Inside one SQLite transaction the read-back is the same row, and
//!   a failure takes the install back with it, so a post-commit check adds nothing: decision D-2.)
//! - **A board in the mod is never overwritten, and the real board is never stranded.** A mod board
//!   that is only an empty default is moved over. One with content keeps both: the legacy row stays,
//!   `pinboard_move_conflict` carries its sha256, and a strip offers "Use my old board"
//!   ([`use_old_board`]: it first copies the mod's board to the declared key `board-before-move`,
//!   then writes the legacy bytes) and "Keep this one" ([`dismiss`]). Nothing is deleted either way.
//! - **A board the mod could not read is repaired as a COPY or refused, never half-moved.** `storage.get`
//!   parses the value, and JavaScript's `JSON.stringify` can write a lone surrogate escape that
//!   `serde_json` refuses. When that is the only problem the mod gets a repaired copy (the escape
//!   replaced by U+FFFD, [`repair_surrogates`]) and `pinboard_move_repaired` says so; the legacy bytes
//!   stay. Any other parse failure, and a board over the mod's save cap (a call's arguments hold at
//!   most 1 MiB, about 100 bytes of which are envelope, so the test is the cap minus 1 KiB), is an
//!   `Err` with the marker kept: the board is safe in the old row, and the strip says so
//!   ([`state`] works that out again from the rows; nothing can be written in a transaction that
//!   rolls back).
//! - **The overlay is derived data and can never roll the install back.** `mod_overlay` is seeded from
//!   the stored bytes through `clamp_overlay` inside a SAVEPOINT; an error there is logged (no text)
//!   and the move carries on, because the mod republishes its dots at every start.
//! - **Shared-vault members can plant anything** in `settings` and `mod_kv` (hostile JSON, huge
//!   values, BLOBs, bytes that are not UTF-8): every read here is by CAST to BLOB or by existence,
//!   so a planted type is never an error that blocks an install, and no untrusted text reaches an
//!   error message or a log line.
//!
//! The marker and pending key names are C2's migration's; the strips that tell the person are drawn
//! by PM (`PinboardMoveStrips`).

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::api::limits::MAX_ARGS_BYTES;
use super::autoinstall::{self, has, AdoptRefusal};
use super::ledger::LedgerError;
use super::store;
use crate::db;
use crate::pinboard_legacy::{self, BOARD_KEY as LEGACY_KEY, PINBOARD_MOD_ID};

/// "A non-blank board exists and its copy is owed." Written by C2's v62; deleted by the move.
pub const PENDING: &str = "pinboard_move_pending";
/// The sha256 (hex) of the legacy row's bytes at the move: the board has moved, and the legacy row is
/// a frozen snapshot from here (`projects::pinboard_move_recorded`, the Calendar and the notes read it).
pub const MOVED: &str = "pinboard_moved_sha";
/// The mod already held a board with content: both kept, the person chooses. The value is the legacy
/// row's sha256 when the strip was raised, so an action only ever moves the board it described.
pub const CONFLICT: &str = "pinboard_move_conflict";
/// The mod's copy had lone surrogate escapes replaced; the legacy bytes are unchanged.
pub const REPAIRED: &str = "pinboard_move_repaired";
/// "The Pinboard's three device prefs (`localStorage`) are still on the device that did the install":
/// the page hands them over ([`adopt_pinboard_prefs`]).
pub const ADOPT_FLAG: &str = "mods_adopt_pinboard_prefs";

/// The mod's board key, and the keys the manifest declares beside it (`mods/atlas.pinboard/manifest.json`).
const BOARD: &str = "board";
const BEFORE_MOVE: &str = "board-before-move";
const PREFS: &str = "prefs";

/// The largest board the move will copy: a mod saves at most [`MAX_ARGS_BYTES`] per call, which holds
/// the board plus about 100 bytes of envelope, so a board over this could be copied but never saved.
pub const MOVE_MAX_BYTES: usize = MAX_ARGS_BYTES - 1024;

// ---- Test-only fault hooks ----------------------------------------------------------------------

/// Faults the tests switch on to prove the guards fire. Each is per thread (a test runs on one), off
/// by default, and compiled out of the product.
#[cfg(test)]
pub(crate) mod fault {
    use std::cell::Cell;

    thread_local! {
        /// After the board is written and before it is read back, change what was stored.
        pub static TAMPER_AFTER_WRITE: Cell<bool> = const { Cell::new(false) };
        /// While the overlay is seeded, fail after the first row is in.
        pub static SEEDING_FAILS: Cell<bool> = const { Cell::new(false) };
        /// How many times a fault above actually fired.
        pub static FIRED: Cell<u32> = const { Cell::new(0) };
    }

    pub fn fire(flag: &'static std::thread::LocalKey<Cell<bool>>) -> bool {
        let on = flag.with(Cell::get);
        if on {
            FIRED.with(|n| n.set(n.get() + 1));
        }
        on
    }
}

// ---- What may move (pure) -----------------------------------------------------------------------

/// Why a board is not copied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// It is not text, or not JSON (and not JSON once lone surrogate escapes are repaired).
    Unreadable,
    /// It is over [`MOVE_MAX_BYTES`].
    TooBig,
}

impl Refusal {
    /// What the install says when it stops here. Fixed words, never the board's.
    fn words(self) -> &'static str {
        match self {
            Self::Unreadable => {
                "PM couldn't move your Pinboard into its mod yet: the saved board couldn't be \
                 read as it is. Your board is safe in PM's store, and nothing was changed."
            }
            Self::TooBig => {
                "PM couldn't move your Pinboard into its mod yet: the saved board is bigger than \
                 a mod can save. Your board is safe in PM's store, and nothing was changed."
            }
        }
    }
}

/// What gets stored for a board that may move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The text for the mod's `board` key: the legacy bytes exactly, or (`repaired`) a copy.
    pub text: String,
    /// Lone surrogate escapes were replaced in `text`.
    pub repaired: bool,
}

/// Whether `bytes` may move, and as what. The size is tested first (a cheap refusal, and the
/// repaired copy is never longer than the original); then the parse, as a value (`storage.get` reads
/// it that way, and only this parse refuses a lone surrogate escape: skipping a value does not). The
/// parsed value is dropped: the board is never re-serialised.
pub fn plan(bytes: &[u8]) -> Result<Plan, Refusal> {
    if bytes.len() > MOVE_MAX_BYTES {
        return Err(Refusal::TooBig);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| Refusal::Unreadable)?;
    if serde_json::from_str::<Value>(text).is_ok() {
        return Ok(Plan {
            text: text.to_owned(),
            repaired: false,
        });
    }
    // The one fault a board can be repaired for. Anything the repair leaves unreadable is refused.
    let repaired = repair_surrogates(text).ok_or(Refusal::Unreadable)?;
    serde_json::from_str::<Value>(&repaired).map_err(|_| Refusal::Unreadable)?;
    Ok(Plan {
        text: repaired,
        repaired: true,
    })
}

/// Four hex digits at the start of `b`, as a UTF-16 code unit.
fn hex4(b: &[u8]) -> Option<u32> {
    let digits = b.get(..4)?;
    let mut value = 0u32;
    for d in digits {
        value = value * 16 + (*d as char).to_digit(16)?;
    }
    Some(value)
}

/// `text` with every LONE surrogate escape (`\ud83d` with no partner, a stray `\udc00`) inside a JSON
/// string replaced by the escape `�`; `None` when there was none. A well-formed pair is kept, and
/// so is an escaped backslash followed by the letters of an escape (`\\ud83d` is a backslash and text).
/// The scan only tracks strings and escapes; it does not parse, so on text that is not JSON it changes
/// what it finds and the caller's parse refuses the result.
fn repair_surrogates(text: &str) -> Option<String> {
    let b = text.as_bytes();
    let mut out = String::new();
    let (mut at, mut kept_from) = (0usize, 0usize);
    let mut in_string = false;
    let mut changed = false;
    fn replace(text: &str, out: &mut String, from: usize, to: usize, kept_from: &mut usize) {
        out.push_str(&text[*kept_from..from]);
        out.push_str("\\ufffd");
        *kept_from = to;
    }
    while at < b.len() {
        match b[at] {
            b'"' => {
                in_string = !in_string;
                at += 1;
            }
            b'\\' if in_string => {
                let unit = if b.get(at + 1) == Some(&b'u') {
                    hex4(&b[at + 2..])
                } else {
                    None
                };
                let Some(unit) = unit else {
                    // Any other escape, or a malformed `\u`: two bytes, and the parse decides.
                    at += 2;
                    continue;
                };
                let end = at + 6;
                if (0xD800..0xDC00).contains(&unit) {
                    let paired = b.get(end) == Some(&b'\\')
                        && b.get(end + 1) == Some(&b'u')
                        && hex4(&b[(end + 2).min(b.len())..])
                            .is_some_and(|low| (0xDC00..0xE000).contains(&low));
                    if paired {
                        at = end + 6;
                        continue;
                    }
                    replace(text, &mut out, at, end, &mut kept_from);
                    changed = true;
                } else if (0xDC00..0xE000).contains(&unit) {
                    replace(text, &mut out, at, end, &mut kept_from);
                    changed = true;
                }
                at = end;
            }
            _ => at += 1,
        }
    }
    changed.then(|| {
        out.push_str(&text[kept_from..]);
        out
    })
}

/// Whether the mod's existing `board` value is only a default: nothing in it is worth keeping. Blank
/// text, or a JSON object with no keys but `version` and `widgets`, whose `widgets` (if present) is an
/// empty list. Anything else (a widget, a key this build does not know, text that is not JSON) has
/// content, and is never overwritten.
fn is_empty_default(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    if text.trim().is_empty() {
        return true;
    }
    let Ok(Value::Object(board)) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    board.keys().all(|k| k == "version" || k == "widgets")
        && board
            .get("widgets")
            .is_none_or(|w| w.as_array().is_some_and(Vec::is_empty))
}

fn sha_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Blank as the move reads it: no row, or text that Rust's `trim` empties. A value that is not UTF-8
/// is not blank (it is refused as unreadable).
fn blank(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|t| t.trim().is_empty())
}

// ---- The rows ---------------------------------------------------------------------------------

/// A `settings` value as text, whatever type it was planted as.
fn text_of(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT CAST(value AS TEXT) FROM settings WHERE key = ?1",
            [key],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

/// The legacy row's exact bytes, or `None` for no row.
fn legacy_bytes(conn: &Connection) -> rusqlite::Result<Option<Vec<u8>>> {
    Ok(conn
        .query_row(
            "SELECT CAST(value AS BLOB) FROM settings WHERE key = ?1",
            [LEGACY_KEY],
            |r| r.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()?
        .flatten())
}

/// A mod key's exact bytes and revision. The revision is `None` when the column holds anything but a
/// plain integer: the column is INTEGER with only a `>= 1` check, so a shared vault's member can plant
/// `'x'` or `1.5`, and a typed read of that would be an error that blocks every install. Such a row
/// has no revision this move can write against, so the callers treat it as content (the conflict
/// path keeps both boards) and never as a default to move over.
fn mod_value(conn: &Connection, key: &str) -> rusqlite::Result<Option<(Vec<u8>, Option<i64>)>> {
    conn.query_row(
        "SELECT CAST(value AS BLOB), revision FROM mod_kv WHERE mod_id = ?1 AND key = ?2",
        params![PINBOARD_MOD_ID, key],
        |r| {
            let revision = match r.get::<_, rusqlite::types::Value>(1)? {
                rusqlite::types::Value::Integer(n) => Some(n),
                _ => None,
            };
            Ok((
                r.get::<_, Option<Vec<u8>>>(0)?.unwrap_or_default(),
                revision,
            ))
        },
    )
    .optional()
}

fn store_failed() -> LedgerError {
    LedgerError::Store(
        "PM couldn't move your Pinboard into its mod, so nothing was changed. Your board is safe \
         in PM's store, and PM tries again."
            .to_owned(),
    )
}

/// Write `text` as the mod's `board`, class `content`: a new row at revision 1, or (`over`) the row at
/// that revision moved to the next one. It never replaces a row it did not read: an INSERT that finds
/// one, or an UPDATE whose revision moved, fails, and the caller's transaction goes with it. Then the
/// row is read back in this transaction and its bytes, sha256, class and revision compared with what
/// was written. Returns the new revision.
fn put_board(conn: &Connection, text: &str, over: Option<i64>) -> Result<i64, LedgerError> {
    let revision = over.map_or(1, |r| r + 1);
    let written = match over {
        None => conn.execute(
            "INSERT INTO mod_kv(mod_id, key, class, value, revision) \
             VALUES (?1, ?2, 'content', ?3, 1)",
            params![PINBOARD_MOD_ID, BOARD, text],
        )?,
        Some(was) => conn.execute(
            "UPDATE mod_kv SET value = ?3, class = 'content', revision = ?4, \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') \
             WHERE mod_id = ?1 AND key = ?2 AND revision = ?5",
            params![PINBOARD_MOD_ID, BOARD, text, revision, was],
        )?,
    };
    if written != 1 {
        return Err(store_failed());
    }
    #[cfg(test)]
    if fault::fire(&fault::TAMPER_AFTER_WRITE) {
        conn.execute(
            "UPDATE mod_kv SET value = value || ' ' WHERE mod_id = ?1 AND key = ?2",
            params![PINBOARD_MOD_ID, BOARD],
        )?;
    }
    let (class, back, back_revision) = conn.query_row(
        "SELECT class, CAST(value AS BLOB), revision FROM mod_kv WHERE mod_id = ?1 AND key = ?2",
        params![PINBOARD_MOD_ID, BOARD],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, i64>(2)?,
            ))
        },
    )?;
    if back != text.as_bytes()
        || sha_hex(&back) != sha_hex(text.as_bytes())
        || class != "content"
        || back_revision != revision
    {
        return Err(store_failed());
    }
    Ok(revision)
}

/// The mod's calendar rows, from the stored board, inside a SAVEPOINT: an error is logged (no text)
/// and rolled back to it, and the move carries on. The overlay is derived (the mod republishes it at
/// every start), so a seeding that fails costs a few dots until then and must never cost the install.
fn seed_overlay(conn: &Connection, text: &str) {
    let entries = pinboard_legacy::derive_overlay(text);
    if conn.execute_batch("SAVEPOINT pinboard_seed").is_err() {
        eprintln!("mods: the Pinboard's calendar entries were not seeded");
        return;
    }
    let seeded = (|| -> rusqlite::Result<()> {
        conn.execute(
            "DELETE FROM mod_overlay WHERE mod_id = ?1",
            [PINBOARD_MOD_ID],
        )?;
        let mut insert = conn
            .prepare("INSERT INTO mod_overlay(mod_id, id, date, label) VALUES (?1, ?2, ?3, ?4)")?;
        for e in &entries {
            insert.execute(params![PINBOARD_MOD_ID, e.id, e.date, e.label])?;
            #[cfg(test)]
            if fault::fire(&fault::SEEDING_FAILS) {
                return Err(rusqlite::Error::InvalidQuery);
            }
        }
        Ok(())
    })();
    let closed = match seeded {
        Ok(()) => conn.execute_batch("RELEASE pinboard_seed"),
        Err(_) => {
            eprintln!("mods: the Pinboard's calendar entries were not seeded");
            conn.execute_batch("ROLLBACK TO pinboard_seed; RELEASE pinboard_seed")
        }
    };
    if closed.is_err() {
        eprintln!("mods: the Pinboard's calendar entries were not settled");
    }
}

/// What a move that happened leaves in `settings`: the overlay seeded, the legacy sha recorded (the
/// board lives in the mod from here; the legacy row is a snapshot), the owed copy cleared, the repair
/// noted, and the device prefs flagged for the page.
fn record_move(conn: &Connection, plan: &Plan, legacy_sha: &str) -> Result<(), LedgerError> {
    seed_overlay(conn, &plan.text);
    db::set_setting(conn, MOVED, legacy_sha).map_err(|_| store_failed())?;
    db::delete_setting(conn, PENDING).map_err(|_| store_failed())?;
    if plan.repaired {
        db::set_setting(conn, REPAIRED, "1").map_err(|_| store_failed())?;
    }
    db::set_setting(conn, ADOPT_FLAG, "1").map_err(|_| store_failed())?;
    Ok(())
}

// ---- The handover ---------------------------------------------------------------------------------

/// The Pinboard's handover (plan K8, steps 1 to 9), in the install's transaction. `Err` rolls the
/// install back, leaving the marker and the pending key for the next pass.
pub(super) fn hand_over_pinboard(conn: &Connection, mod_id: &str) -> Result<(), LedgerError> {
    // The mod's id is the one the keys are declared for; any other id is somebody else's storage.
    if mod_id != PINBOARD_MOD_ID {
        return Err(store_failed());
    }
    // 1, 2. Nothing owed (a restore of a store that moved already): nothing happens.
    if !has(conn, PENDING)? {
        return Ok(());
    }
    // 3. Nothing to copy: no row, or blank.
    let Some(legacy) = legacy_bytes(conn)?.filter(|b| !blank(b)) else {
        db::delete_setting(conn, PENDING).map_err(|_| store_failed())?;
        return Ok(());
    };
    let legacy_sha = sha_hex(&legacy);
    // 4. A board in the mod already: never overwrite it, never leave the real one behind.
    let over = match mod_value(conn, BOARD)? {
        None => None,
        Some((bytes, Some(revision))) if is_empty_default(&bytes) => Some(revision),
        Some(_) => {
            // `MOVED` is written here too, which the plan's step 4 does not list: it says the board
            // lives in the mod, so a rename no longer rewrites the legacy row (it would change the
            // bytes `use_old_board` checks against `CONFLICT`, and the strip would answer "Changed").
            // The cost: the legacy row is a snapshot from here, so a project renamed or merged while
            // the strip is open is renamed in the mod's refs only, and "Use my old board" brings the
            // old board in with its timelines still bound to the earlier name. No bytes are lost (the
            // old row and `board-before-move` both stay); the strip says so (`PinboardMoveStrips`).
            db::set_setting(conn, CONFLICT, &legacy_sha).map_err(|_| store_failed())?;
            db::set_setting(conn, MOVED, &legacy_sha).map_err(|_| store_failed())?;
            db::delete_setting(conn, PENDING).map_err(|_| store_failed())?;
            return Ok(());
        }
    };
    // 5, 6. Readable and small enough, or refused with the marker kept.
    let plan = plan(&legacy).map_err(|r| LedgerError::Store(r.words().to_owned()))?;
    // 7, 8. The bytes in, read back.
    put_board(conn, &plan.text, over)?;
    // 9. The rest.
    record_move(conn, &plan, &legacy_sha)
}

// ---- The strips' half ------------------------------------------------------------------------

/// What the page's strips need to know (`pinboard_move_state`). Booleans and a word: never the
/// board's own text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveState {
    /// Both boards kept: "Use my old board" or "Keep this one".
    pub conflict: bool,
    /// The mod's copy had characters replaced.
    pub repaired: bool,
    /// The move cannot happen as the board is: `unreadable` or `tooBig`. Worked out from the rows
    /// (a refused move rolls its transaction back, so it can write nothing).
    pub refused: Option<&'static str>,
}

pub fn state(conn: &Connection) -> rusqlite::Result<MoveState> {
    let mut refused = None;
    let owed = has(conn, PENDING)? && !has(conn, MOVED)?;
    if owed {
        let pilot = autoinstall::PILOTS.iter().find(|p| p.id == PINBOARD_MOD_ID);
        let wanted = match pilot {
            Some(p) => {
                has(conn, p.marker)?
                    || conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM mods WHERE id = ?1)",
                        [PINBOARD_MOD_ID],
                        |r| r.get(0),
                    )?
            }
            None => false,
        };
        if wanted {
            if let Some(legacy) = legacy_bytes(conn)?.filter(|b| !blank(b)) {
                refused = match plan(&legacy) {
                    Ok(_) => None,
                    Err(Refusal::Unreadable) => Some("unreadable"),
                    Err(Refusal::TooBig) => Some("tooBig"),
                };
            }
        }
    }
    Ok(MoveState {
        conflict: has(conn, CONFLICT)?,
        repaired: has(conn, REPAIRED)?,
        refused,
    })
}

/// Why "Use my old board" did nothing.
#[derive(Debug)]
pub enum UseOldRefusal {
    /// There is no conflict to settle.
    NoConflict,
    /// The Pinboard mod isn't installed from the catalogue here.
    NotInstalled,
    /// The old row isn't the board the strip described (it is gone, blank or different now).
    Changed,
    Unreadable,
    TooBig,
    /// `board-before-move` already holds a different board: nothing is overwritten, so nothing moves.
    CopyInTheWay,
    /// A write or its read-back failed; nothing was changed.
    Failed,
    Db(rusqlite::Error),
}

impl From<rusqlite::Error> for UseOldRefusal {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}

/// "Use my old board": the mod's current board is copied to `board-before-move` (read back), THEN the
/// legacy bytes become the mod's `board`, one revision on (a frame holding the old copy meets the
/// revision check). In one transaction: nothing is deleted, the legacy row is untouched, and a failure
/// anywhere leaves both boards exactly as they were. The strip's note goes with the success.
pub fn use_old_board(conn: &mut Connection) -> Result<(), UseOldRefusal> {
    let tx = conn.transaction()?;
    let Some(carried) = text_of(&tx, CONFLICT)? else {
        return Err(UseOldRefusal::NoConflict);
    };
    let installed: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM mods WHERE id = ?1 AND installed_from = ?2 AND signed = 1)",
        params![PINBOARD_MOD_ID, store::FROM_CATALOGUE],
        |r| r.get(0),
    )?;
    if !installed {
        return Err(UseOldRefusal::NotInstalled);
    }
    let legacy = legacy_bytes(&tx)?
        .filter(|b| !blank(b))
        .ok_or(UseOldRefusal::Changed)?;
    let legacy_sha = sha_hex(&legacy);
    if !carried.trim().eq_ignore_ascii_case(&legacy_sha) {
        return Err(UseOldRefusal::Changed);
    }
    let plan = plan(&legacy).map_err(|r| match r {
        Refusal::Unreadable => UseOldRefusal::Unreadable,
        Refusal::TooBig => UseOldRefusal::TooBig,
    })?;
    let over = match mod_value(&tx, BOARD)? {
        None => None,
        Some((current, revision)) => {
            // A revision that is not a plain integer (planted) leaves nothing to write against: the
            // two boards stay as they are and "Keep this one" is still the way out.
            let Some(revision) = revision else {
                return Err(UseOldRefusal::Failed);
            };
            // Save the board that is there first. A copy already in place is fine when it is this
            // very board, and never replaced when it is another.
            match mod_value(&tx, BEFORE_MOVE)? {
                Some((kept, _)) if kept == current => {}
                Some(_) => return Err(UseOldRefusal::CopyInTheWay),
                None => {
                    tx.execute(
                        "INSERT INTO mod_kv(mod_id, key, class, value, revision) \
                         SELECT mod_id, ?2, 'content', value, 1 FROM mod_kv \
                         WHERE mod_id = ?1 AND key = ?3",
                        params![PINBOARD_MOD_ID, BEFORE_MOVE, BOARD],
                    )?;
                    match mod_value(&tx, BEFORE_MOVE)? {
                        Some((kept, _)) if kept == current => {}
                        _ => return Err(UseOldRefusal::Failed),
                    }
                }
            }
            Some(revision)
        }
    };
    put_board(&tx, &plan.text, over).map_err(|_| UseOldRefusal::Failed)?;
    record_move(&tx, &plan, &legacy_sha).map_err(|_| UseOldRefusal::Failed)?;
    db::delete_setting(&tx, CONFLICT).map_err(|_| UseOldRefusal::Failed)?;
    tx.commit()?;
    Ok(())
}

/// Clear a strip's note: `conflict` ("Keep this one": the mod's board stays and the old row stays as
/// a snapshot) or `repaired` ("OK"). `false` for a name that is neither.
pub fn dismiss(conn: &Connection, which: &str) -> rusqlite::Result<bool> {
    let key = match which {
        "conflict" => CONFLICT,
        "repaired" => REPAIRED,
        _ => return Ok(false),
    };
    conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
    Ok(true)
}

// ---- The page's device prefs ----------------------------------------------------------------------

/// Merge this device's three Pinboard nags (`localStorage`: `None` = this device never set it) into the
/// mod's `prefs` key, only into the fields the mod does not hold yet, then clear the flag. A frame has
/// no `localStorage`, so only the page can carry them. The same shape as the Map's
/// ([`autoinstall::adopt_map_prefs`]): `Ok(false)` when there was nothing to do; the page keeps its
/// keys; nothing is written without the flag the move sets (dormant until then).
///
/// A `prefs` value that is not a JSON object is not ours to replace: it is left as it is and the flag
/// cleared.
pub fn adopt_pinboard_prefs(
    conn: &mut Connection,
    confirm_delete: Option<bool>,
    show_past: Option<bool>,
    show_completed: Option<bool>,
) -> Result<bool, AdoptRefusal> {
    let device = [
        ("confirmDelete", confirm_delete),
        ("showPast", show_past),
        ("showCompleted", show_completed),
    ];
    if device.iter().all(|(_, v)| v.is_none()) {
        return Ok(false);
    }
    let tx = conn.transaction()?;
    if !has(&tx, ADOPT_FLAG)? {
        return Ok(false);
    }
    let from_catalogue: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM mods WHERE id = ?1 AND installed_from = ?2 AND signed = 1)",
        params![PINBOARD_MOD_ID, store::FROM_CATALOGUE],
        |r| r.get(0),
    )?;
    if !from_catalogue {
        return Err(AdoptRefusal::NotInstalled);
    }
    let existing = mod_value(&tx, PREFS)?;
    let object = match &existing {
        None => Some(serde_json::Map::new()),
        Some((bytes, _)) => match std::str::from_utf8(bytes)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(t).ok())
        {
            Some(Value::Object(o)) => Some(o),
            _ => None,
        },
    };
    if let Some(mut object) = object {
        let mut changed = false;
        for (key, value) in device {
            if let (Some(v), false) = (value, object.contains_key(key)) {
                object.insert(key.to_owned(), Value::Bool(v));
                changed = true;
            }
        }
        if changed {
            tx.execute(
                "INSERT INTO mod_kv(mod_id, key, class, value, revision) \
                 VALUES (?1, ?2, 'content', ?3, 1) \
                 ON CONFLICT(mod_id, key) DO UPDATE SET value = excluded.value, \
                   revision = mod_kv.revision + 1, \
                   updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')",
                params![PINBOARD_MOD_ID, PREFS, Value::Object(object).to_string()],
            )?;
        }
    }
    tx.execute("DELETE FROM settings WHERE key = ?1", [ADOPT_FLAG])?;
    tx.commit()?;
    Ok(true)
}

#[cfg(test)]
mod tests;
