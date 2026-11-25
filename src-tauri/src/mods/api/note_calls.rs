// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The note calls (PR C, K2): `documents.saveNote` and `documents.statusOf`. A note is a Markdown
//! document in the vault; the Pinboard keeps its notes as documents so they are searched, filed and
//! cited like any other (`notes::save`, the body the Pinboard's own command shares).
//!
//! **A mod never names a `source_id`.** Rust derives where a note lives from WHO the caller is
//! ([`namespace_for`]), never from anything a call says:
//! - the first-party Pinboard (`atlas.pinboard`, signed, tier First) keeps the LEGACY names every
//!   existing note already has, `note:<key>` and `note-<key>.md`, so the move to a mod orphans
//!   nothing (the document ids, citations and filing on them stay where they are);
//! - every other mod, an unsigned folder mod that calls itself `atlas.pinboard` included, gets its
//!   own: `mod:<id>:<key>` and `mod-<id>.<key>.md`, attributed to it by prefix. The id alone is not
//!   enough: a developer build may load an unsigned `atlas.*` folder, and it must not write over, or
//!   read, the Pinboard's notes.
//!
//! **Keys.** ASCII letters, digits, `-` and `_` (`ingest::valid_note_id`), at most
//! [`limits::NOTE_KEY_MAX_OWN`] characters in a mod's own namespace (the file name stays under 255
//! with the longest mod id) and [`limits::NOTE_KEY_MAX_LEGACY`] through the legacy mapping (a board
//! whose widget ids are older or imported must still save after the move). The legacy mapping refuses
//! a key that starts `chat-`: Chat saves its answers as `note:chat-<message id>`, and a board planted
//! in a shared vault must not overwrite another person's saved answer.
//!
//! **The engine must already be set up**; a call that finds it is not answers `MOD-5007` and PM
//! provisions nothing (the page offers a "Set it up" button of its own).
//!
//! **The one deliberate exception to "a call in flight across an uninstall commits nothing":** the
//! embed and the vault write are not transactional with the mod's state. The call is re-checked right
//! before them; one already started when an uninstall lands finishes. The note is the user's own text
//! and stays labelled by its prefix.

use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::calls;
use super::documents::ARCHIVE;
use super::grants::{self, ModFacts};
use super::handlers::{parse, within_call_timeout};
use super::permissions::PolicyTier;
use super::projection::{Dto, Needs, Projection};
use super::{blocking, limits, recheck, Authorised, Host, ModError, Reply};
use crate::mods::package::user_line_fault;
use crate::notes::Namespace;

/// The first-party Pinboard's id: with tier First it is the one mod whose notes keep the legacy names.
pub(super) const PINBOARD_ID: &str = "atlas.pinboard";

/// Where `facts`' notes live. The id AND the tier: a signed First row is Atlas's own, and an unsigned
/// folder mod's manifest can claim any tier it likes (`PolicyTier::of` reads only a signed row's).
pub(super) fn namespace_for(facts: &ModFacts) -> Namespace {
    if facts.id.as_str() == PINBOARD_ID && facts.tier == PolicyTier::First {
        Namespace::Legacy
    } else {
        Namespace::Mod(facts.id.clone())
    }
}

/// Whether `key` may name a note in `namespace`. The sentences name the rule, never the key.
pub(super) fn check_key(namespace: &Namespace, key: &str) -> Result<(), ModError> {
    let cap = match namespace {
        Namespace::Legacy => limits::NOTE_KEY_MAX_LEGACY,
        Namespace::Mod(_) => limits::NOTE_KEY_MAX_OWN,
    };
    if !crate::ingest::valid_note_id(key) || key.len() > cap {
        return Err(ModError::bad_args(format!(
            "A note's key is letters, digits, - and _, at most {cap} of them."
        )));
    }
    if matches!(namespace, Namespace::Legacy) && key.starts_with("chat-") {
        return Err(ModError::bad_args(
            "That key belongs to a saved answer in Chat.",
        ));
    }
    Ok(())
}

/// A note's title: one plain line, at most [`limits::NOTE_TITLE_MAX_CHARS`] characters, with no
/// newline and no `---` (it lands in the vault file's front matter). A refusal, never a repair; the
/// spaces around it are dropped first.
pub(super) fn checked_title(raw: &str) -> Result<String, ModError> {
    let title = raw.trim_matches(' ');
    let words = || {
        ModError::bad_args(format!(
            "A note's title is one plain line of at most {} characters, without ---.",
            limits::NOTE_TITLE_MAX_CHARS
        ))
    };
    user_line_fault(title, limits::NOTE_TITLE_MAX_CHARS, false).map_err(|_| words())?;
    if title.contains("---") {
        return Err(words());
    }
    Ok(title.to_owned())
}

/// A note's body: not blank, at most [`limits::NOTE_BODY_MAX_BYTES`].
pub(super) fn checked_body(raw: &str) -> Result<&str, ModError> {
    if raw.len() > limits::NOTE_BODY_MAX_BYTES {
        return Err(ModError::new(
            super::Code::TooLarge,
            "A note's text is larger than PM allows.",
        ));
    }
    if raw.trim().is_empty() {
        return Err(ModError::bad_args("A note needs some text."));
    }
    Ok(raw)
}

// --- documents.saveNote -------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveArgs {
    key: String,
    title: String,
    body: String,
}

/// `documents.saveNote({key, title, body})` → `{saved: true, documentId?}`; `documentId` only with
/// `documents.meta`. Never the vault path, the project or the `source_id`.
pub(super) async fn save_note<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let SaveArgs { key, title, body } = parse(args)?;
    let title = checked_title(&title)?;
    checked_body(&body)?;
    let value = {
        let (host, auth) = (host.clone(), auth.clone());
        within_call_timeout(blocking(move || {
            #[cfg(test)]
            host.at(super::Point::BeforeData);
            // Who this is decides where the note goes, and whether the key is one it may use.
            let (namespace, mod_id) = {
                let conn = host.store().conn().map_err(|_| ModError::locked())?;
                let facts = recheck(&host, &conn, &auth)?;
                let namespace = namespace_for(&facts);
                check_key(&namespace, &key)?;
                (namespace, facts.id)
            };
            // The engine must already be set up: PM provisions nothing for a mod's call.
            if !host.engine_ready() {
                return Err(ModError::engine_not_ready());
            }
            // Asked again right before the heavy step, with no guard held across it (the embed and the
            // vault write take the guard themselves, briefly, several times).
            {
                let conn = host.store().conn().map_err(|_| ModError::locked())?;
                recheck(&host, &conn, &auth)?;
            }
            let document = host.save_note(&namespace, &key, &title, &body)?;
            // The note is saved whatever became of the mod meanwhile; what it is told about the
            // document depends on what it may still know.
            let meta = host.store().conn().ok().is_some_and(|conn| {
                grants::read_mod(&conn, &mod_id)
                    .ok()
                    .flatten()
                    .is_some_and(|facts| {
                        grants::check(&conn, &facts, "documents.meta", host.engine_sealed()).is_ok()
                    })
            });
            Ok(if meta {
                json!({"saved": true, "documentId": document})
            } else {
                json!({"saved": true})
            })
        }))
        .await?
    };
    Ok(Reply::value(value))
}

// --- documents.statusOf -------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusArgs {
    keys: Vec<String>,
}

/// What one note's document is: `{documentId, reviewed, archived}` and, with `projects.read`,
/// `project` (its home project's name, or null).
fn status_dto(
    id: i64,
    reviewed: bool,
    project: &str,
    importance: Option<&str>,
    projection: Projection,
) -> Value {
    let project = project.trim();
    Dto::new(projection)
        .field(Needs::Nothing, "documentId", id)
        .field(Needs::Nothing, "reviewed", reviewed)
        .field(Needs::Nothing, "archived", importance == Some(ARCHIVE))
        .field(
            Needs::Projects,
            "project",
            (!project.is_empty()).then(|| project.to_owned()),
        )
        .build()
}

/// `documents.statusOf({keys})` → `{<key>: null | {documentId, reviewed, archived, project?}}`. It
/// reads ONLY the caller's own namespace, so it cannot say whether anyone else's note exists, and a
/// document that was deleted is `null` ("not ingested"), not a stale answer.
pub(super) async fn status_of<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let StatusArgs { keys } = parse(args)?;
    if keys.len() > limits::MAX_NOTE_KEYS_PER_CALL {
        return Err(ModError::bad_args(format!(
            "Ask about at most {} notes at a time.",
            limits::MAX_NOTE_KEYS_PER_CALL
        )));
    }
    let value = calls::read(host, auth, move |cx| {
        let namespace = namespace_for(cx.facts);
        for key in &keys {
            check_key(&namespace, key)?;
        }
        let projection = Projection::of_grants(cx.conn, cx.facts, cx.host.engine_sealed())?;
        let sources: Vec<String> = keys.iter().map(|k| namespace.source_id(k)).collect();
        let wanted = serde_json::to_string(&sources).map_err(ModError::internal)?;
        let mut stmt = cx
            .conn
            .prepare(
                "SELECT source_id, id, reviewed, project, importance FROM documents \
                 WHERE source_id IN (SELECT value FROM json_each(?1))",
            )
            .map_err(ModError::internal)?;
        let rows = stmt
            .query_map(params![wanted], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)? != 0,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })
            .map_err(ModError::internal)?;
        let mut found: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
        for row in rows {
            let (source, id, reviewed, project, importance) = row.map_err(ModError::internal)?;
            found.insert(
                source,
                status_dto(id, reviewed, &project, importance.as_deref(), projection),
            );
        }
        let mut out = Map::new();
        for (key, source) in keys.iter().zip(&sources) {
            out.insert(
                key.clone(),
                found.get(source).cloned().unwrap_or(Value::Null),
            );
        }
        Ok(Value::Object(out))
    })
    .await?;
    Ok(Reply::value(value))
}
