// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Notes as vault documents: the one way a note's text becomes a real Markdown document, whoever
//! asks (the Pinboard's `ingest_note` command today, a mod's `documents.saveNote` next).
//!
//! A note is named by a **namespace** and a **key**. The namespace says whose it is and so how it is
//! attributed: the Pinboard's notes keep the legacy `note:<key>` `source_id` and `note-<key>.md`
//! file (every existing note, and the document ids and citations on them, stay where they are),
//! and a mod's are `mod:<id>:<key>` and `mod-<id>.<key>.md`, so a mod's documents are attributed to
//! it by prefix and survive a Rebuild (spec 1.3). The namespace is chosen by the caller's
//! authority, never by the text of a request. A mod never names a `source_id` itself.
//!
//! [`save`] is the shared body of the command and the handler: check the text, make sure the
//! document engine is ready, and hand the rest to [`crate::ingest::ingest_note_document`], which
//! embeds, writes the vault file (refusing one that isn't this note's own) and indexes.

use crate::error::{Error, Result};
use crate::ingest::{self, Document};
use crate::mods::ids::ModId;
use crate::AppState;

/// Whose a note is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Namespace {
    /// The Pinboard's notes before it became a mod, and Chat's saved answers: `note:<key>`,
    /// `note-<key>.md`.
    Legacy,
    /// A mod's own: `mod:<id>:<key>`, `mod-<id>.<key>.md`. The key and the id are told apart by the
    /// last `.`: a note key never holds one and a mod id always does, whereas both may hold `-`, so a
    /// `-` between them let two mods' notes share a file name (`acme.a-b` + `c` and `acme.a` + `b-c`),
    /// and one mod could then keep another's note from ever saving.
    Mod(ModId),
}

impl Namespace {
    /// The `documents.source_id` (and front-matter claim) of the note `key`.
    pub fn source_id(&self, key: &str) -> String {
        match self {
            Self::Legacy => format!("note:{key}"),
            Self::Mod(id) => format!("mod:{}:{key}", id.as_str()),
        }
    }

    /// The logical vault file name of the note `key` (before any `.pmenc` suffix).
    pub fn file_name(&self, key: &str) -> String {
        match self {
            Self::Legacy => format!("note-{key}.md"),
            Self::Mod(id) => format!("mod-{}.{key}.md", id.as_str()),
        }
    }
}

/// Save a note as a vault document, creating it or updating it in place (an unchanged note is a
/// no-op, an edited one keeps its project, tags and importance). `title` is the line the note is
/// known by; the caller has already chosen it. Blocking: it embeds the text, so call it off the
/// main thread, and with no DB guard held (it takes the guard itself, briefly, several times).
pub fn save(
    state: &AppState,
    namespace: &Namespace,
    key: &str,
    title: &str,
    body: &str,
) -> Result<Document> {
    let body = body.trim();
    if body.is_empty() {
        return Err(Error::Other(
            "this note is empty — nothing to ingest".into(),
        ));
    }
    state.sidecar.ensure_installed()?;
    let gateway = {
        let conn = state.conn()?;
        state.gateway_for_write(&conn)?
    };
    let (vault, cipher) = state.markdown_io()?;
    let (vault_root, manifest_cipher) = state.manifest_io()?;
    ingest::ingest_note_document(
        state,
        &gateway,
        &vault,
        &cipher,
        &vault_root,
        &manifest_cipher,
        namespace,
        key,
        title,
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atlas() -> Namespace {
        Namespace::Mod(ModId::parse("atlas.pinboard").unwrap())
    }

    /// The legacy names are what every existing note already is on disk and in `documents`; a change
    /// here would orphan them, and the mod namespace must never be able to collide with them.
    #[test]
    fn the_legacy_namespace_keeps_the_names_notes_already_have() {
        assert_eq!(Namespace::Legacy.source_id("w-1-ab12"), "note:w-1-ab12");
        assert_eq!(Namespace::Legacy.file_name("w-1-ab12"), "note-w-1-ab12.md");
    }

    #[test]
    fn a_mod_note_is_attributed_by_its_prefix() {
        assert_eq!(atlas().source_id("w-1-ab12"), "mod:atlas.pinboard:w-1-ab12");
        assert_eq!(
            atlas().file_name("w-1-ab12"),
            "mod-atlas.pinboard.w-1-ab12.md"
        );
        // Neither name can be one the legacy namespace produces, whatever the key.
        for key in ["x", "chat-1", "mod", "atlas"] {
            assert_ne!(atlas().source_id(key), Namespace::Legacy.source_id(key));
            assert!(!atlas().source_id(key).starts_with("note:"));
            assert!(!atlas().file_name(key).starts_with("note-"));
        }
    }

    /// Two different (mod, key) pairs never share a file name, even where `-` could be read either
    /// way: the name is the same only for the same mod and key.
    #[test]
    fn two_mods_notes_never_share_a_file_name() {
        let ids = [
            "acme.a",
            "acme.a-b",
            "acme.a-b-c",
            "acme.a.b",
            "acme.a-b.c",
            "acme.b",
            "ab.c",
            "a-b.c",
            "atlas.pinboard",
        ];
        let keys = [
            "c", "b-c", "b", "b-c-d", "c-d", "a", "a-b", "a_b", "C", "md", "mod", "x-1",
        ];
        let mut seen = std::collections::HashMap::new();
        for id in ids {
            for key in keys {
                let ns = Namespace::Mod(ModId::parse(id).unwrap());
                let name = ns.file_name(key);
                if let Some(other) = seen.insert(name.clone(), (id, key)) {
                    panic!("{name} is both {other:?} and {:?}", (id, key));
                }
                // The source_id is injective too (the `:` is in neither part).
                assert_eq!(ns.source_id(key), format!("mod:{id}:{key}"));
            }
        }
        // The case the old `-` separator got wrong.
        let a = Namespace::Mod(ModId::parse("acme.a-b").unwrap());
        let b = Namespace::Mod(ModId::parse("acme.a").unwrap());
        assert_ne!(a.file_name("c"), b.file_name("b-c"));
    }
}
