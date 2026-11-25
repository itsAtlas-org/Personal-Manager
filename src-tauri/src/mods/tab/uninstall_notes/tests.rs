// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keeping the Pinboard's notes on uninstall (K5b): the plan, the one-note save and its read-back,
//! the `.md` files and the board file. The embed is the one thing not run here (no engine in a unit
//! test): a fake stands in for it, counts its calls (the spy on the embed step) and checks, each call,
//! that the store's guard is free.

use std::cell::{Cell, RefCell};

use rusqlite::params;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::*;
use crate::mods::api::test_support::{insert_mod, store, ModRow};
use crate::sidecar::{SidecarManager, SidecarPaths};

fn id() -> ModId {
    ModId::parse(PINBOARD_MOD_ID).unwrap()
}

struct World {
    _dir: tempfile::TempDir,
    state: AppState,
}

fn world() -> World {
    let (dir, conn) = store();
    insert_mod(
        &conn,
        &ModRow::signed(PINBOARD_MOD_ID, "first", &["storage"]),
    );
    let sidecar = SidecarManager::new(SidecarPaths {
        source_dir: dir.path().join("sidecar"),
        venv_dir: dir.path().join("runtime").join("venv"),
    });
    World {
        _dir: dir,
        state: AppState::new(Some(conn), None, sidecar, None, None),
    }
}

impl World {
    fn sql<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        f(&self.state.conn().unwrap())
    }

    fn board(&self, widgets: serde_json::Value) {
        let raw = json!({"version": 1, "widgets": widgets}).to_string();
        self.sql(|c| crate::db::set_setting(c, "pinboard", &raw).unwrap());
    }

    /// The documents table, as a note's document would sit in it.
    fn document(&self, key: &str, title: &str, body: &str, source_type: &str) -> i64 {
        self.sql(|c| {
            c.execute(
                "INSERT INTO documents(vault_path, title, content_hash, source_id, source_type, reviewed) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 1)",
                params![
                    format!("note-{key}.md"),
                    title,
                    expected_hash(key, body),
                    format!("note:{key}"),
                    source_type
                ],
            )
            .unwrap();
            c.last_insert_rowid()
        })
    }

    fn documents(&self) -> Vec<String> {
        self.sql(|c| {
            let mut stmt = c
                .prepare("SELECT source_id FROM documents ORDER BY source_id")
                .unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        })
    }
}

fn note_card(id: &str, text: &str) -> serde_json::Value {
    json!({"id": id, "kind": "note", "text": text})
}

/// An embed that writes the document row the real one would, counts its calls and checks the guard.
struct Fake<'a> {
    world: &'a World,
    ready: bool,
    calls: RefCell<Vec<String>>,
    /// The call (1-based) that fails.
    fail_on: Cell<Option<usize>>,
    /// Answers ok but writes no document: a save that did not happen.
    lies: bool,
    guard_was_held: Cell<bool>,
}

impl<'a> Fake<'a> {
    fn new(world: &'a World) -> Self {
        Self {
            world,
            ready: true,
            calls: RefCell::default(),
            fail_on: Cell::new(None),
            lies: false,
            guard_was_held: Cell::new(false),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}

impl Embed for Fake<'_> {
    fn engine_ready(&self) -> bool {
        self.ready
    }

    fn save(&self, key: &str, title: &str, body: &str) -> crate::error::Result<i64> {
        // The embed runs with no guard held: try to take it from here.
        if self.world.state.db.try_lock().is_err() {
            self.guard_was_held.set(true);
            // Stop here: going on to write the document would wait on a guard its own caller holds.
            return Err(crate::error::Error::Other("the guard was held".into()));
        }
        self.calls.borrow_mut().push(key.to_owned());
        if self.fail_on.get() == Some(self.calls.borrow().len()) {
            return Err(crate::error::Error::Other("the embed failed".into()));
        }
        if self.lies {
            return Ok(1);
        }
        self.world.sql(|c| {
            c.execute(
                "DELETE FROM documents WHERE source_id = ?1",
                [format!("note:{key}")],
            )
            .unwrap();
        });
        Ok(self.world.document(key, title, body, SOURCE_TYPE_VAULT))
    }
}

// ---- the plan ----

#[test]
fn the_plan_sorts_the_notes_into_what_to_save_what_is_saved_and_what_cannot_be() {
    let w = world();
    w.board(json!([
        note_card("n-new", "never saved"),
        note_card("n-saved", "saved already"),
        note_card("n-edited", "edited since"),
        note_card("n blank", "has a bad id"),
        note_card("chat-77", "a Chat answer's name"),
        note_card("n-empty", "   \n"),
        {"id": "f", "kind": "folder", "children": [note_card("n-inside", "in a folder")]},
        note_card("n-new", "a second card with the same id"),
        {"id": "t", "kind": "timeline", "items": []},
    ]));
    w.document(
        "n-saved",
        "saved already",
        "saved already",
        SOURCE_TYPE_VAULT,
    );
    w.document(
        "n-edited",
        "edited since",
        "an older text",
        SOURCE_TYPE_VAULT,
    );
    let plan = plan(&w.state, &id()).unwrap().unwrap();
    assert_eq!(
        plan.to_save
            .iter()
            .map(|n| n.key.as_str())
            .collect::<Vec<_>>(),
        ["n-new", "n-edited", "n-inside"],
        "in board order, the edited one included"
    );
    assert_eq!(plan.already_saved, 1);
    // A bad id, Chat's name and the repeated id; the blank card is not counted at all.
    assert_eq!(plan.unsaveable, 3);
    assert_eq!(plan.total, 7);
    assert!(!plan.engine_ready, "no engine in a fresh store");
    assert_eq!(plan.source, Source::Legacy);
    assert!(plan.board_bytes > 0);
}

#[test]
fn the_plan_is_for_the_first_party_pinboard_and_nothing_else() {
    let w = world();
    w.board(json!([note_card("n1", "text")]));
    assert!(plan(&w.state, &ModId::parse("atlas.map").unwrap())
        .unwrap()
        .is_none());
    // An unsigned folder mod that calls itself atlas.pinboard has no old notes of its own.
    w.sql(|c| c.execute("DELETE FROM mods", []).unwrap());
    w.sql(|c| insert_mod(c, &ModRow::unsigned(PINBOARD_MOD_ID, &["storage"])));
    assert!(plan(&w.state, &id()).unwrap().is_none());
    w.sql(|c| c.execute("DELETE FROM mods", []).unwrap());
    // Signed but not First is no Pinboard either.
    w.sql(|c| insert_mod(c, &ModRow::signed(PINBOARD_MOD_ID, "second", &["storage"])));
    assert!(plan(&w.state, &id()).unwrap().is_none());
}

#[test]
fn after_the_move_the_plan_reads_the_mods_board_not_the_old_row() {
    let w = world();
    w.board(json!([note_card("old-1", "the pre-move board")]));
    w.sql(|c| {
        crate::db::set_setting(c, "pinboard_moved_sha", &"a".repeat(64)).unwrap();
        c.execute(
            "INSERT INTO mod_kv(mod_id, key, class, value) VALUES (?1, 'board', 'content', ?2)",
            params![
                PINBOARD_MOD_ID,
                json!({"version": 1, "widgets": [note_card("new-1", "written in the mod")]})
                    .to_string()
            ],
        )
        .unwrap();
    });
    let plan = plan(&w.state, &id()).unwrap().unwrap();
    assert_eq!(plan.source, Source::Mod);
    assert_eq!(plan.to_save.len(), 1);
    assert_eq!(plan.to_save[0].key, "new-1");
}

#[test]
fn a_note_the_board_records_as_ingested_and_unchanged_is_saved_even_though_its_document_is_rendered(
) {
    // The document holds the Markdown the Pinboard rendered, not the card's text, so the two cannot
    // be compared; the board's own record (ingestedAt + the hash of the text) stands in for it.
    let w = world();
    w.board(json!([
        {"id": "n1", "kind": "note", "text": "hello", "ingestedAt": "2026-07-01T10:00:00.000Z", "ingestedHash": "2y0dev"},
        {"id": "n2", "kind": "note", "text": "hello", "ingestedAt": "2026-07-01T10:00:00.000Z", "ingestedHash": "stale"},
        {"id": "n3", "kind": "note", "text": "hello", "ingestedAt": "2026-07-01T10:00:00.000Z", "ingestedHash": "2y0dev"},
    ]));
    // n1: a rendered document; n2: a document and the board says the text changed; n3: no document.
    w.document("n1", "A rendered title", "hello  \n", SOURCE_TYPE_VAULT);
    w.document("n2", "t", "hello  \n", SOURCE_TYPE_VAULT);
    let plan = plan(&w.state, &id()).unwrap().unwrap();
    assert_eq!(plan.already_saved, 1, "n1");
    assert_eq!(
        plan.to_save
            .iter()
            .map(|n| n.key.as_str())
            .collect::<Vec<_>>(),
        ["n2", "n3"]
    );
    // A legacy index-only pointer is not a saved note, whatever the board says: saving promotes it.
    w.sql(|c| {
        c.execute("UPDATE documents SET source_type = 'index_only'", [])
            .unwrap()
    });
    assert_eq!(
        super::plan(&w.state, &id()).unwrap().unwrap().to_save.len(),
        3
    );
}

#[test]
fn a_key_of_200_characters_can_be_a_document_and_one_of_201_cannot() {
    let w = world();
    w.board(json!([
        note_card(&"k".repeat(200), "fits"),
        note_card(&"k".repeat(201), "too long"),
    ]));
    let plan = plan(&w.state, &id()).unwrap().unwrap();
    assert_eq!(plan.to_save.len(), 1);
    assert_eq!(plan.unsaveable, 1);
}

// ---- saving one note ----

#[test]
fn a_new_note_is_embedded_once_and_its_document_is_read_back() {
    let w = world();
    w.board(json!([note_card("n1", "  keep this  \n")]));
    let fake = Fake::new(&w);
    let saved = save_one(&w.state, &fake, &id(), "n1").unwrap();
    assert_eq!(saved.outcome, Outcome::Saved);
    assert_eq!(fake.calls(), ["n1"]);
    assert_eq!(w.documents(), ["note:n1"]);
    // The id the answer names is the document row's.
    let row = w.sql(|c| stored_row(c, "n1").unwrap().unwrap());
    assert_eq!(row.0, saved.document_id);
}

/// The spy on the embed step: a note whose document is already current is skipped, never embedded,
/// and needs no engine; an edited one is embedded again.
#[test]
fn an_unchanged_note_is_skipped_and_never_embedded_and_a_changed_one_is_embedded_again() {
    let w = world();
    w.board(json!([
        note_card("same", "the same text"),
        note_card("edited", "the new text"),
    ]));
    let doc = w.document("same", "the same text", "the same text", SOURCE_TYPE_VAULT);
    w.document("edited", "the new text", "the old text", SOURCE_TYPE_VAULT);
    let mut fake = Fake::new(&w);
    fake.ready = false;
    let skipped = save_one(&w.state, &fake, &id(), "same").unwrap();
    assert_eq!(skipped.outcome, Outcome::Unchanged);
    assert_eq!(skipped.document_id, doc);
    assert!(fake.calls().is_empty(), "no embed, and no engine needed");
    // The edited one does need the engine, and says so before it tries.
    assert_eq!(
        save_one(&w.state, &fake, &id(), "edited").unwrap_err(),
        Refusal::EngineNotReady
    );
    assert!(fake.calls().is_empty());
    fake.ready = true;
    assert_eq!(
        save_one(&w.state, &fake, &id(), "edited").unwrap().outcome,
        Outcome::Saved
    );
    assert_eq!(fake.calls(), ["edited"]);
}

#[test]
fn a_chats_saved_answer_is_never_touched() {
    let w = world();
    w.board(json!([note_card(
        "chat-77",
        "a board planted with Chat's name"
    )]));
    let chat_doc = w.document("chat-77", "An answer", "the real answer", SOURCE_TYPE_VAULT);
    let fake = Fake::new(&w);
    assert_eq!(
        save_one(&w.state, &fake, &id(), "chat-77").unwrap_err(),
        Refusal::NotADocument
    );
    assert!(fake.calls().is_empty());
    let row = w.sql(|c| stored_row(c, "chat-77").unwrap().unwrap());
    assert_eq!(
        (row.0, row.2),
        (chat_doc, expected_hash("chat-77", "the real answer"))
    );
    // Nor does the plan offer it, count it as saved, or list it.
    let plan = plan(&w.state, &id()).unwrap().unwrap();
    assert!(plan.to_save.is_empty());
    assert_eq!((plan.already_saved, plan.unsaveable), (0, 1));
}

#[test]
fn a_save_that_answered_ok_but_left_no_document_is_not_a_save() {
    // The read-back: an uninstall that went ahead on a document that is not there would delete the
    // only copy of the note.
    let w = world();
    w.board(json!([note_card("n1", "text")]));
    let mut fake = Fake::new(&w);
    fake.lies = true;
    assert_eq!(
        save_one(&w.state, &fake, &id(), "n1").unwrap_err(),
        Refusal::NotConfirmed
    );
    assert!(w.documents().is_empty());
}

#[test]
fn a_document_that_is_not_this_notes_text_does_not_read_back() {
    let w = world();
    w.board(json!([note_card("n1", "the text")]));
    // The embed answers with a document row holding other text.
    struct Wrong<'a>(&'a World);
    impl Embed for Wrong<'_> {
        fn engine_ready(&self) -> bool {
            true
        }
        fn save(&self, key: &str, title: &str, _body: &str) -> crate::error::Result<i64> {
            Ok(self.0.document(key, title, "other text", SOURCE_TYPE_VAULT))
        }
    }
    assert_eq!(
        save_one(&w.state, &Wrong(&w), &id(), "n1").unwrap_err(),
        Refusal::NotConfirmed
    );
}

#[test]
fn the_refusals_are_for_notes_that_are_not_there_or_not_the_pinboards() {
    let w = world();
    w.board(json!([note_card("n1", "text"), note_card("blank", " ")]));
    let fake = Fake::new(&w);
    for key in ["nope", "blank"] {
        assert_eq!(
            save_one(&w.state, &fake, &id(), key).unwrap_err(),
            Refusal::NoSuchNote,
            "{key}"
        );
    }
    assert_eq!(
        save_one(&w.state, &fake, &ModId::parse("atlas.map").unwrap(), "n1").unwrap_err(),
        Refusal::NotThePinboard
    );
    assert!(fake.calls().is_empty());
}

#[test]
fn a_failed_embed_is_a_refusal_with_a_fixed_sentence() {
    let w = world();
    w.board(json!([note_card("n1", "secret words")]));
    let fake = Fake::new(&w);
    fake.fail_on.set(Some(1));
    let refusal = save_one(&w.state, &fake, &id(), "n1").unwrap_err();
    assert_eq!(refusal, Refusal::Failed);
    let words = ModsError::from(refusal).message;
    assert!(
        !words.contains("secret") && !words.contains("n1"),
        "{words}"
    );
    assert!(w.documents().is_empty());
}

#[test]
fn a_run_stops_at_the_note_that_fails_keeps_the_earlier_saves_and_a_retry_saves_nothing_twice() {
    let w = world();
    w.board(json!([
        note_card("n1", "one"),
        note_card("n2", "two"),
        note_card("n3", "three"),
        note_card("n4", "four"),
        note_card("n5", "five"),
    ]));
    let fake = Fake::new(&w);
    fake.fail_on.set(Some(3));
    // The page's loop: in order, and the first failure ends it.
    let run = |fake: &Fake<'_>| -> Result<usize, (usize, Refusal)> {
        let plan = plan(&w.state, &id()).unwrap().unwrap();
        for (i, n) in plan.to_save.iter().enumerate() {
            save_one(&w.state, fake, &id(), &n.key).map_err(|e| (i, e))?;
        }
        Ok(plan.to_save.len())
    };
    assert_eq!(run(&fake), Err((2, Refusal::Failed)));
    assert_eq!(
        w.documents(),
        ["note:n1", "note:n2"],
        "the earlier saves remain"
    );
    // The retry: the plan no longer lists the two saved notes, so they are not embedded again.
    fake.fail_on.set(None);
    let before = fake.calls().len();
    assert_eq!(run(&fake), Ok(3));
    assert_eq!(fake.calls()[before..], ["n3", "n3", "n4", "n5"][1..]);
    assert_eq!(fake.calls().iter().filter(|k| *k == "n1").count(), 1);
    assert_eq!(fake.calls().iter().filter(|k| *k == "n2").count(), 1);
    assert_eq!(w.documents().len(), 5);
}

#[test]
fn a_600_note_board_saves_note_by_note_and_never_holds_the_store_across_a_note() {
    let w = world();
    let cards: Vec<_> = (0..600)
        .map(|i| note_card(&format!("n{i}"), &format!("note number {i}")))
        .collect();
    w.board(json!(cards));
    let fake = Fake::new(&w);
    let plan_before = plan(&w.state, &id()).unwrap().unwrap();
    assert_eq!(plan_before.to_save.len(), 600);
    let mut done = 0;
    for n in &plan_before.to_save {
        // Each note is one call, and between calls the store is free to anyone else.
        assert!(
            w.state.db.try_lock().is_ok(),
            "the guard is free before note {done}"
        );
        save_one(&w.state, &fake, &id(), &n.key).unwrap();
        assert!(w.state.db.try_lock().is_ok(), "and after it");
        done += 1;
    }
    assert_eq!(done, 600);
    assert!(
        !fake.guard_was_held.get(),
        "no guard held while a note was embedded"
    );
    let after = plan(&w.state, &id()).unwrap().unwrap();
    assert_eq!((after.to_save.len(), after.already_saved), (0, 600));
}

#[test]
fn a_new_note_lands_reviewed_for_an_uninstall_and_in_the_queue_for_an_ingest() {
    use crate::notes::Arrival;
    assert_eq!(
        crate::ingest::fresh_note_filing(Arrival::Reviewed),
        ("Unsorted", true)
    );
    assert_eq!(
        crate::ingest::fresh_note_filing(Arrival::Review),
        ("Unsorted", false)
    );
    // And the uninstall's own save asks for the reviewed arrival.
    let source = include_str!("../uninstall_notes.rs");
    let at = source.find("impl Embed for Engine").unwrap();
    assert!(source[at..].contains("Arrival::Reviewed"));
}

// ---- the files ----

fn md(key: &str, title: &str, text: &str) -> LegacyNote {
    LegacyNote {
        key: key.into(),
        title: title.into(),
        text: text.into(),
        recorded_current: false,
    }
}

fn names(folder: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn a_file_is_named_for_its_title_and_the_first_eight_of_its_key() {
    assert_eq!(
        export_stem("Meeting notes", "c0ffee11-aaaa-bbbb"),
        "Meeting notes-c0ffee11"
    );
    assert_eq!(export_stem("", "k1"), "Untitled-k1");
    assert_eq!(export_stem("   ", "abc"), "Untitled-abc");
    // A key that is no name at all leaves just the title.
    assert_eq!(export_stem("Plan", "../.."), "Plan");
}

/// `NUL.md` on Windows opens the null device: `create_new` succeeds, the bytes go nowhere and the note
/// is reported written, then the uninstall deletes the only copy. A note with a key that is no name
/// and a title that is a device name must still get a real file.
#[test]
fn a_title_that_is_a_windows_device_name_still_makes_a_real_file() {
    for title in [
        "NUL",
        "con",
        "Aux",
        "PRN",
        "COM1",
        "com9",
        "LPT3",
        "LPT\u{B9}",
        " nul ",
    ] {
        for key in ["-", "_", "..", ""] {
            let stem = export_stem(title, key);
            assert!(!is_device_name(&stem), "{title:?}/{key:?} -> {stem:?}");
            assert!(stem.starts_with('_'), "{stem:?}");
        }
    }
    // A name that only starts like one is left alone, and a title with its key part is not a device.
    assert_eq!(export_stem("NUL", "ab12cd34"), "NUL-ab12cd34");
    assert_eq!(export_stem("Console", "-"), "Console");
    assert_eq!(export_stem("COM10", "-"), "COM10");
    assert_eq!(export_stem("COM", "-"), "COM");
    // And the file really is written, under a name that is not the device's.
    let dir = tempfile::tempdir().unwrap();
    let notes = [md("-", "NUL", "keep me")];
    let out = export_into(dir.path(), "2026-11-25", Some(&notes), None);
    assert_eq!(out.notes_written, 1, "{out:?}");
    assert_eq!(names(dir.path()), ["_NUL.md"]);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("_NUL.md")).unwrap(),
        "keep me"
    );
}

/// A board with two cards sharing an id, the first one blank: the plan counts the card with text, so
/// saving that id must find the same card, not the blank one.
#[test]
fn a_blank_card_sharing_an_id_does_not_make_the_note_unsaveable() {
    let w = world();
    w.board(json!([
        {"id": "k", "kind": "note", "text": "   "},
        {"id": "k", "kind": "note", "text": "hello"},
    ]));
    let plan = plan(&w.state, &id()).unwrap().unwrap();
    assert_eq!(plan.to_save.len(), 1);
    let fake = Fake::new(&w);
    let saved = save_one(&w.state, &fake, &id(), "k").unwrap();
    assert_eq!(saved.outcome, Outcome::Saved);
    assert_eq!(fake.calls(), ["k"]);
    // Saved, so the plan now counts it as done.
    assert_eq!(
        super::plan(&w.state, &id()).unwrap().unwrap().already_saved,
        1
    );
}

#[test]
fn a_title_can_never_make_a_name_with_dots_or_separators() {
    for title in [
        "../../etc/passwd",
        "a..b/../c",
        "..",
        "a/b\\c:d",
        "COM1",
        "name.md",
        "tab\tand\nnewline",
        "\u{202e}rtl",
        "  - _ leading",
        "~/home",
    ] {
        let stem = export_stem(title, "key12345");
        // Not one dot: the only dot in a file's name is the one before `md` that PM adds itself.
        assert!(!stem.contains('.'), "{title:?} -> {stem:?}");
        for bad in ["..", "/", "\\", ":", "\n", "\t", "\u{202e}", "~"] {
            assert!(!stem.contains(bad), "{title:?} -> {stem:?}");
        }
        assert!(!stem.starts_with('.') && !stem.starts_with('-'), "{stem:?}");
    }
    // A very long title is cut well inside a file system's limit, without splitting a character.
    let stem = export_stem(&"\u{1F600}x".repeat(500), &"k".repeat(300));
    assert!(
        stem.len() + ".md".len() + " (999)".len() <= 255,
        "{}",
        stem.len()
    );
}

#[test]
fn the_notes_are_written_as_md_files_and_nothing_leaves_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("out");
    std::fs::create_dir(&folder).unwrap();
    let notes = vec![
        md("n1", "../../escape", "first\n---\nsecond"),
        md("n2", "", "untitled body"),
    ];
    let out = export_into(&folder, "2026-11-25", Some(&notes), None);
    assert_eq!(out.failed, None);
    assert_eq!(out.notes_written, 2);
    assert_eq!(names(&folder), ["Untitled-n2.md", "escape-n1.md"]);
    assert_eq!(
        std::fs::read_to_string(folder.join("escape-n1.md")).unwrap(),
        "first\n---\nsecond",
        "the card's text, exactly"
    );
    assert_eq!(
        names(dir.path()),
        ["out"],
        "nothing written beside the folder"
    );
}

#[test]
fn a_taken_name_gets_a_counter_and_nothing_already_there_is_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Plan-n1.md"), "the person's own file").unwrap();
    std::fs::write(
        dir.path().join("pinboard-board-2026-11-25.json"),
        "earlier copy",
    )
    .unwrap();
    let notes = vec![md("n1", "Plan", "a"), md("n1", "Plan", "b")];
    let out = export_into(dir.path(), "2026-11-25", Some(&notes), Some(b"{}"));
    assert_eq!(out.failed, None);
    assert_eq!(
        names(dir.path()),
        [
            "Plan-n1 (2).md",
            "Plan-n1 (3).md",
            "Plan-n1.md",
            "pinboard-board-2026-11-25 (2).json",
            "pinboard-board-2026-11-25.json"
        ]
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("Plan-n1.md")).unwrap(),
        "the person's own file"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("pinboard-board-2026-11-25.json")).unwrap(),
        "earlier copy"
    );
    assert_eq!(
        out.board_file.as_deref(),
        Some("pinboard-board-2026-11-25 (2).json")
    );
}

#[test]
fn the_board_file_is_the_stored_boards_exact_bytes() {
    let w = world();
    // Not valid UTF-8, with odd spacing: the bytes must cross untouched.
    let raw: Vec<u8> = b"{\"version\": 1,\r\n \"widgets\":[]}\xff\xfe  \n".to_vec();
    w.sql(|c| {
        c.execute(
            "INSERT INTO settings(key, value) VALUES ('pinboard', CAST(?1 AS TEXT))",
            [&raw],
        )
        .unwrap();
    });
    let stored = w.sql(|c| pinboard_legacy::stored_board(c).unwrap().unwrap());
    assert_eq!(stored.bytes, raw);
    let dir = tempfile::tempdir().unwrap();
    let out = export_into(dir.path(), "2026-11-25", None, Some(&stored.bytes));
    assert_eq!(
        out.board_file.as_deref(),
        Some("pinboard-board-2026-11-25.json")
    );
    let written = std::fs::read(dir.path().join("pinboard-board-2026-11-25.json")).unwrap();
    assert_eq!(
        Sha256::digest(&written),
        Sha256::digest(&raw),
        "sha256 equal"
    );
    assert_eq!(out.notes_written, 0);
}

#[test]
fn an_export_stops_at_the_first_file_it_cannot_write() {
    let dir = tempfile::tempdir().unwrap();
    let gone = dir.path().join("not-there");
    let notes = vec![md("n1", "A", "a"), md("n2", "B", "b")];
    let out = export_into(&gone, "2026-11-25", Some(&notes), Some(b"{}"));
    let failed = out.failed.unwrap();
    assert_eq!(failed.note_index, Some(0));
    assert_eq!(failed.message, "That folder is not there any more.");
    assert_eq!((out.notes_written, out.board_file), (0, None));
    // A failure on the board file comes after the notes, and says it was the board.
    let folder = dir.path().join("out");
    std::fs::create_dir(&folder).unwrap();
    // Every counter name for the board file is taken by a directory: no free name.
    std::fs::create_dir(folder.join("pinboard-board-2026-11-25.json")).unwrap();
    for n in 2..=999 {
        std::fs::create_dir(folder.join(format!("pinboard-board-2026-11-25 ({n}).json"))).unwrap();
    }
    let out = export_into(&folder, "2026-11-25", Some(&notes), Some(b"{}"));
    assert_eq!(out.notes_written, 2, "the notes went first");
    assert_eq!(out.failed.unwrap().note_index, None);
    assert_eq!(out.board_file, None);
}

#[test]
fn only_notes_with_text_are_exported() {
    let notes = exportable(vec![
        md("a", "", "text"),
        md("b", "t", "  \n"),
        md("c", "", ""),
    ]);
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].key, "a");
}

// ---- the commands ----

#[test]
fn the_three_commands_are_registered_wrapped_and_the_preview_carries_the_plan() {
    let lib = include_str!("../../../lib.rs");
    for c in [
        "uninstall_notes_plan",
        "uninstall_save_note",
        "uninstall_export_files",
    ] {
        assert!(lib.contains(&format!("mods::tab::commands::{c},")), "{c}");
    }
    let ipc = include_str!("../../../../../src/lib/ipc.ts");
    for c in [
        "uninstall_notes_plan",
        "uninstall_save_note",
        "uninstall_export_files",
    ] {
        assert!(ipc.contains(&format!("\"{c}\"")), "{c}");
    }
    let commands = include_str!("../commands.rs");
    assert!(commands.contains("preview.notes ="));
    assert!(commands.contains("uninstall_notes::plan(&app.state::<AppState>(), &id)"));
}
