// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What uninstalling the Pinboard says about its notes from before it was a mod (PR C, K5, D-4): the
//! notes it saved as `note:<widget id>` are the user's own and predate the install, so the dialog lists
//! them as kept and no uninstall, "also delete" or not, ever deletes one. Chat's saved answers
//! (`note:chat-<message id>`) are not the Pinboard's and are not counted at all.

use super::*;
use crate::mods::tab::uninstall;

fn pinboard(m: &mut Value) {
    m["id"] = "atlas.pinboard".into();
    m["name"] = "Pinboard".into();
}

fn install_pinboard_with_notes(w: &World) {
    w.install(&pkg("1.0.0", pinboard), Approval::Approved)
        .unwrap();
    // Every note predates the install (the default `ingested_at` is now, so push them back).
    w.exec(
        "INSERT INTO documents(vault_path, title, content_hash, source_id, ingested_at) VALUES \
            ('n1.md', 'N1', 'h1', 'note:w-1', '2020-01-01T00:00:00Z'), \
            ('n2.md', 'N2', 'h2', 'note:w-2', '2020-01-01T00:00:00Z'), \
            ('n3.md', 'N3', 'h3', 'note:100_percent%', '2020-01-01T00:00:00Z'), \
            ('c1.md', 'Chat', 'h4', 'note:chat-77', '2020-01-01T00:00:00Z'), \
            ('o1.md', 'Other', 'h5', 'gdrive:me:f1', '2020-01-01T00:00:00Z'), \
            ('m1.md', 'Made', 'h6', 'mod:atlas.pinboard:k1', '2999-01-01T00:00:00Z');",
    );
}

fn sources(w: &World) -> Vec<String> {
    let c = w.store.conn.lock().unwrap();
    let mut stmt = c
        .prepare("SELECT source_id FROM documents ORDER BY source_id")
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn the_pinboards_old_notes_are_listed_as_kept_and_chats_saved_answers_are_not_counted() {
    let w = world();
    install_pinboard_with_notes(&w);
    let p = uninstall::preview(&w.ledger(), &id("atlas.pinboard")).unwrap();
    // Three notes by their old name (a `%` and an `_` in an id are not wildcards); the one document
    // under the mod's own prefix is a gift, counted where it always was.
    assert_eq!(p.stays.kept_documents, 3);
    assert_eq!(p.stays.documents, 1);
}

#[test]
fn an_uninstall_that_deletes_what_the_mod_made_never_deletes_a_kept_note() {
    let w = world();
    install_pinboard_with_notes(&w);
    let done = uninstall::run(&w.ledger(), &id("atlas.pinboard"), true).unwrap();
    assert_eq!(done.gifts_deleted.unwrap().documents, 1);
    assert_eq!(
        sources(&w),
        [
            "gdrive:me:f1",
            "note:100_percent%",
            "note:chat-77",
            "note:w-1",
            "note:w-2"
        ],
        "only the document under the mod's own prefix went"
    );
}

#[test]
fn the_old_name_is_the_pinboards_alone_and_only_while_it_is_first_party_and_signed() {
    let w = world();
    // Another mod: `note:` documents are nobody's gifts, kept or otherwise.
    w.install(&pkg("1.0.0", |_| {}), Approval::Approved)
        .unwrap();
    w.exec(
        "INSERT INTO documents(vault_path, title, content_hash, source_id) \
         VALUES ('n1.md', 'N1', 'h1', 'note:w-1')",
    );
    assert_eq!(
        uninstall::preview(&w.ledger(), &id("atlas.map"))
            .unwrap()
            .stays
            .kept_documents,
        0
    );
    // A stranger using the Pinboard's name (not first party, or not signed) owns none of them.
    let w = world();
    install_pinboard_with_notes(&w);
    w.exec("UPDATE mods SET tier = 'third'");
    assert_eq!(
        uninstall::preview(&w.ledger(), &id("atlas.pinboard"))
            .unwrap()
            .stays
            .kept_documents,
        0
    );
    w.exec("UPDATE mods SET tier = 'first', signed = 0");
    assert_eq!(
        uninstall::preview(&w.ledger(), &id("atlas.pinboard"))
            .unwrap()
            .stays
            .kept_documents,
        0
    );
}
