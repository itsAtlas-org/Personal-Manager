// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A mod's per-document storage (Mods spec §1.5), with the `documents.meta` permission:
//! `storage.doc.get`, `.set`, `.list` and `.delete` on `mod_doc_data`. The Map keeps each document's
//! coordinates here.
//!
//! - **Cache class only.** These rows cascade from `documents` (a delete reaches them even while the
//!   mod is closed, and a Rebuild, which deletes and re-creates every document under new ids, wipes
//!   them), and a backup leaves them out (D4). So a key the manifest declares as `content` is refused:
//!   data a mod can't afford to lose belongs in `storage.set`. An undeclared key is fine; this table
//!   is cache whatever the manifest says.
//! - **Keys** follow the same grammar as `storage.*` (`package::is_data_key`), and a mod's per-document
//!   keys are its own: another mod's rows, and this mod's `mod_kv` keys, are separate namespaces.
//! - **No revisions.** `mod_kv` rows carry one, so two frames can't overwrite each other; a
//!   `mod_doc_data` row has no revision column (v60 is final, and slice 4 adds no migration), and a
//!   cache that can be rebuilt is last-writer-wins. A frame that wants compare-and-set keeps the
//!   document's value under a `storage.set` key.
//! - **Writes are batched** (`set` takes up to [`limits::MAX_DOCS_PER_CALL`] entries of one key in
//!   one transaction), because a layout of 5,000 documents one call at a time would spend the rate
//!   limit's burst fifty times over. All or nothing: a document that is missing or unreadable (the
//!   same answer, [`documents::not_found`]) refuses the whole call before anything is written.
//! - **The quota** is the one `storage.set` uses, over `mod_kv` and `mod_doc_data` together, checked in
//!   the same transaction as the write; a write that doesn't grow the mod's storage always fits.
//! - **Reads see only readable documents** (`documents::readable`): `get` of a missing, shelved or
//!   unreadable document is the same `NotFound`, and `list` lists only the readable ones. `delete` of a
//!   mod's own rows needs no readability: it can't read anything.

use std::collections::HashSet;

use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};

use super::documents::{not_found, readable, LISTED};
use super::handlers::parse;
use super::limits::{self, quota_allows, row_cost};
use super::storage::{checked_key, doc_usage, kv_usage};
use super::{blocking, recheck, still_current, Authorised, Host, ModError, Reply};
use crate::mods::ids::ModId;
use crate::mods::package::DataClass;

/// A page of `(document, value)` rows and the cursor after it.
type Page = (Vec<(i64, Value)>, Option<i64>);

/// Refuse a key the manifest declares as `content`.
fn cache_key(
    classes: &std::collections::BTreeMap<String, DataClass>,
    key: &str,
) -> Result<(), ModError> {
    checked_key(key)?;
    if classes.get(key) == Some(&DataClass::Content) {
        return Err(ModError::bad_args(
            "A per-document key is cache: declare it as a cache key, or keep it with storage.set.",
        ));
    }
    Ok(())
}

fn now() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct GetArgs {
    document_id: i64,
    key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Entry {
    document_id: i64,
    value: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetArgs {
    key: String,
    entries: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    key: String,
    #[serde(default)]
    after: Option<i64>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DeleteArgs {
    key: String,
    /// Only these documents' rows; every row of the key when omitted (clearing a cache).
    #[serde(default)]
    document_ids: Option<Vec<i64>>,
}

/// `storage.doc.get(documentId, key)` → `{found, value}`; `NotFound` for a document the caller can't
/// read.
pub(super) async fn get<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let GetArgs { document_id, key } = parse(args)?;
    checked_key(&key)?;
    let value = {
        let host = host.clone();
        let auth = auth.clone();
        blocking(move || {
            #[cfg(test)]
            host.at(super::Point::BeforeData);
            let conn = host.store().conn().map_err(|_| ModError::locked())?;
            recheck(&host, &conn, &auth)?;
            read_one(&conn, &auth.ticket.mod_id, &key, document_id)
        })
        .await?
    };
    still_current(host, auth)?;
    Ok(Reply::value(match value {
        Some(value) => json!({ "found": true, "value": value }),
        None => json!({ "found": false, "value": null }),
    }))
}

/// `storage.doc.set(key, entries)` → `{written}`.
pub(super) async fn set<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let SetArgs { key, entries } = parse(args)?;
    checked_key(&key)?;
    if entries.is_empty() || entries.len() > limits::MAX_DOCS_PER_CALL {
        return Err(ModError::bad_args(format!(
            "Write 1 to {} entries at a time.",
            limits::MAX_DOCS_PER_CALL
        )));
    }
    let mut seen = HashSet::new();
    if !entries.iter().all(|e| seen.insert(e.document_id)) {
        return Err(ModError::bad_args("A document is named twice."));
    }
    let texts = entries
        .iter()
        .map(|e| {
            serde_json::to_string(&e.value)
                .map(|text| (e.document_id, text))
                .map_err(ModError::internal)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let written = {
        let host = host.clone();
        let auth = auth.clone();
        blocking(move || {
            #[cfg(test)]
            host.at(super::Point::BeforeData);
            let mut conn = host.store().conn().map_err(|_| ModError::locked())?;
            let tx = conn.transaction().map_err(ModError::internal)?;
            // Under this transaction's guard: the mod as it is now, and nothing written unless every
            // check still holds.
            let facts = recheck(&host, &tx, &auth)?;
            cache_key(&facts.data_classes, &key)?;
            let written = write_entries(&tx, &auth.ticket.mod_id, &key, &texts)?;
            #[cfg(test)]
            host.at(super::Point::BeforeCommit);
            // Immediately before the commit: a bump since the recheck rolls the write back.
            still_current(&host, &auth)?;
            tx.commit().map_err(ModError::internal)?;
            Ok(written)
        })
        .await?
    };
    still_current(host, auth)?;
    Ok(Reply::value(json!({ "written": written })))
}

/// `storage.doc.list(key, after?, limit?)` → `{entries: [{documentId, value}], next}`, by document
/// id, readable documents only.
pub(super) async fn list<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let ListArgs { key, after, limit } = parse(args)?;
    checked_key(&key)?;
    let after = after.unwrap_or(0);
    if after < 0 {
        return Err(ModError::bad_args("after is a document id: 0 or more."));
    }
    let limit = limit.unwrap_or(limits::MAX_DOCS_PER_CALL);
    if limit == 0 || limit > limits::MAX_DOCS_PER_CALL {
        return Err(ModError::bad_args(format!(
            "limit is 1 to {}.",
            limits::MAX_DOCS_PER_CALL
        )));
    }
    let (entries, next) = {
        let host = host.clone();
        let auth = auth.clone();
        blocking(move || {
            #[cfg(test)]
            host.at(super::Point::BeforeData);
            let conn = host.store().conn().map_err(|_| ModError::locked())?;
            recheck(&host, &conn, &auth)?;
            list_page(
                &conn,
                &auth.ticket.mod_id,
                &key,
                after,
                limit,
                limits::LIST_PAGE_BUDGET_BYTES,
            )
        })
        .await?
    };
    still_current(host, auth)?;
    let entries: Vec<Value> = entries
        .into_iter()
        .map(|(document_id, value)| json!({ "documentId": document_id, "value": value }))
        .collect();
    Ok(Reply::value(json!({ "entries": entries, "next": next })))
}

/// `storage.doc.delete(key, documentIds?)` → `{deleted}`: this mod's rows of `key`.
pub(super) async fn delete<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let DeleteArgs { key, document_ids } = parse(args)?;
    checked_key(&key)?;
    if document_ids
        .as_ref()
        .is_some_and(|ids| ids.len() > limits::MAX_DOCS_PER_CALL)
    {
        return Err(ModError::bad_args(format!(
            "Name at most {} documents.",
            limits::MAX_DOCS_PER_CALL
        )));
    }
    let deleted = {
        let host = host.clone();
        let auth = auth.clone();
        blocking(move || {
            #[cfg(test)]
            host.at(super::Point::BeforeData);
            let mut conn = host.store().conn().map_err(|_| ModError::locked())?;
            let tx = conn.transaction().map_err(ModError::internal)?;
            recheck(&host, &tx, &auth)?;
            let deleted = delete_rows(&tx, &auth.ticket.mod_id, &key, document_ids.as_deref())?;
            #[cfg(test)]
            host.at(super::Point::BeforeCommit);
            still_current(&host, &auth)?;
            tx.commit().map_err(ModError::internal)?;
            Ok(deleted)
        })
        .await?
    };
    still_current(host, auth)?;
    Ok(Reply::value(json!({ "deleted": deleted })))
}

/// The value this mod stored under `key` for `document`: `None` for a readable document with none.
/// A document the caller can't read is [`not_found`].
fn read_one(
    conn: &Connection,
    id: &ModId,
    key: &str,
    document: i64,
) -> Result<Option<Value>, ModError> {
    if !readable(conn, &[document])?.contains(&document) {
        return Err(not_found());
    }
    let text: Option<String> = conn
        .query_row(
            "SELECT value FROM mod_doc_data WHERE mod_id = ?1 AND document_id = ?2 AND key = ?3",
            params![id.as_str(), document, key],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })
        .map_err(ModError::internal)?;
    text.map(|t| serde_json::from_str(&t).map_err(ModError::internal))
        .transpose()
}

/// Write `entries` (`(document, JSON text)`) under `key`, all or nothing, inside the caller's
/// transaction: every document readable, the quota allowing the growth. How many were written.
fn write_entries(
    conn: &Connection,
    id: &ModId,
    key: &str,
    entries: &[(i64, String)],
) -> Result<usize, ModError> {
    let ids: Vec<i64> = entries.iter().map(|(d, _)| *d).collect();
    let ok = readable(conn, &ids)?;
    if ids.iter().any(|d| !ok.contains(d)) {
        return Err(not_found());
    }
    // What the rows being replaced cost, and what the new ones will.
    let key_bytes = key.len() as u64;
    let old: u64 = {
        let mut stmt = conn
            .prepare(
                "SELECT length(CAST(value AS BLOB)) FROM mod_doc_data \
                 WHERE mod_id = ?1 AND key = ?2 \
                   AND document_id IN (SELECT value FROM json_each(?3))",
            )
            .map_err(ModError::internal)?;
        let ids_json = serde_json::to_string(&ids).map_err(ModError::internal)?;
        let sizes = stmt
            .query_map(params![id.as_str(), key, ids_json], |r| r.get::<_, i64>(0))
            .map_err(ModError::internal)?;
        let mut total = 0u64;
        for size in sizes {
            let size = size.map_err(ModError::internal)?;
            total = total.saturating_add(row_cost(key_bytes, size.max(0) as u64));
        }
        total
    };
    let new: u64 = entries.iter().fold(0u64, |total, (_, text)| {
        total.saturating_add(row_cost(key_bytes, text.len() as u64))
    });
    let everything = kv_usage(conn, id, None)?.saturating_add(doc_usage(conn, id, None)?);
    if !quota_allows(
        everything.saturating_sub(old),
        old,
        new,
        limits::STORAGE_QUOTA_BYTES,
    ) {
        return Err(ModError::new(
            super::Code::QuotaExceeded,
            "This mod's storage is full.",
        ));
    }
    let stamp = now();
    let mut put = conn
        .prepare(
            "INSERT INTO mod_doc_data(mod_id, document_id, key, value, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(mod_id, document_id, key) DO UPDATE SET \
               value = excluded.value, updated_at = excluded.updated_at",
        )
        .map_err(ModError::internal)?;
    for (document, text) in entries {
        put.execute(params![id.as_str(), document, key, text, stamp])
            .map_err(ModError::internal)?;
    }
    Ok(entries.len())
}

/// A page of this mod's `key` rows after `after`, of readable documents, by document id: at most
/// `limit`, ending early under `budget` bytes (the first row always goes in). The cursor is the last id
/// returned, `None` when nothing follows.
fn list_page(
    conn: &Connection,
    id: &ModId,
    key: &str,
    after: i64,
    limit: usize,
    budget: usize,
) -> Result<Page, ModError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT m.document_id, m.value FROM mod_doc_data m \
             JOIN documents d ON d.id = m.document_id \
             WHERE m.mod_id = ?1 AND m.key = ?2 AND m.document_id > ?3 AND {LISTED} \
             ORDER BY m.document_id LIMIT ?4"
        ))
        .map_err(ModError::internal)?;
    let rows = stmt
        .query_map(params![id.as_str(), key, after, limit as i64 + 1], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(ModError::internal)?;
    let mut page = Vec::new();
    let mut used = 0usize;
    let mut last = None;
    for row in rows {
        let (document, text) = row.map_err(ModError::internal)?;
        if page.len() == limit {
            return Ok((page, last));
        }
        // The text is already JSON, so its length is the value's, plus the entry's own framing.
        let size = text.len() + 40;
        if !page.is_empty() && used + size > budget {
            return Ok((page, last));
        }
        used += size;
        last = Some(document);
        page.push((
            document,
            serde_json::from_str(&text).map_err(ModError::internal)?,
        ));
    }
    Ok((page, None))
}

/// Delete this mod's `key` rows: those of `documents`, or all of them. How many went.
fn delete_rows(
    conn: &Connection,
    id: &ModId,
    key: &str,
    documents: Option<&[i64]>,
) -> Result<usize, ModError> {
    match documents {
        None => conn.execute(
            "DELETE FROM mod_doc_data WHERE mod_id = ?1 AND key = ?2",
            params![id.as_str(), key],
        ),
        Some(documents) => conn.execute(
            "DELETE FROM mod_doc_data WHERE mod_id = ?1 AND key = ?2 \
             AND document_id IN (SELECT value FROM json_each(?3))",
            params![
                id.as_str(),
                key,
                serde_json::to_string(documents).map_err(ModError::internal)?
            ],
        ),
    }
    .map_err(ModError::internal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::api::test_support::{insert_mod, store, ModRow};
    use crate::mods::api::Code;
    use std::collections::BTreeMap;

    fn id(s: &str) -> ModId {
        ModId::parse(s).unwrap()
    }

    /// A store with one mod and documents 1 (listed), 2 (archived) and 3 (listed).
    fn world() -> (tempfile::TempDir, Connection, ModId) {
        let (dir, conn) = store();
        insert_mod(
            &conn,
            &ModRow::signed("acme.tool", "second", &["documents.meta"]),
        );
        for (n, importance) in [(1, None), (2, Some("archive")), (3, Some("high"))] {
            conn.execute(
                "INSERT INTO documents(id, vault_path, title, content_hash, importance) \
                 VALUES (?1, ?2, 't', ?2, ?3)",
                params![n, format!("{n}.md"), importance],
            )
            .unwrap();
        }
        (dir, conn, id("acme.tool"))
    }

    fn entries(pairs: &[(i64, &str)]) -> Vec<(i64, String)> {
        pairs.iter().map(|(d, t)| (*d, (*t).to_owned())).collect()
    }

    fn rows(conn: &Connection, m: &ModId) -> Vec<(i64, String, String)> {
        let mut stmt = conn
            .prepare(
                "SELECT document_id, key, value FROM mod_doc_data WHERE mod_id = ?1 \
                 ORDER BY document_id, key",
            )
            .unwrap();
        stmt.query_map([m.as_str()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn a_batch_writes_replaces_and_reads_back_per_document() {
        let (_dir, conn, m) = world();
        assert_eq!(
            write_entries(&conn, &m, "coords", &entries(&[(1, "[1,2]"), (3, "[3,4]")])).unwrap(),
            2
        );
        assert_eq!(
            read_one(&conn, &m, "coords", 1).unwrap(),
            Some(json!([1, 2]))
        );
        assert_eq!(
            read_one(&conn, &m, "coords", 3).unwrap(),
            Some(json!([3, 4]))
        );
        // A readable document with nothing under that key reads as none, not as an error.
        assert_eq!(read_one(&conn, &m, "other", 1).unwrap(), None);
        // A write replaces in place: one row, the new value, the same key.
        write_entries(&conn, &m, "coords", &entries(&[(1, "{\"x\":9}")])).unwrap();
        assert_eq!(
            rows(&conn, &m),
            vec![
                (1, "coords".into(), "{\"x\":9}".into()),
                (3, "coords".into(), "[3,4]".into())
            ]
        );
        // Another mod's rows of the same key are its own.
        insert_mod(
            &conn,
            &ModRow::signed("acme.other", "second", &["documents.meta"]),
        );
        let other = id("acme.other");
        assert_eq!(read_one(&conn, &other, "coords", 1).unwrap(), None);
        write_entries(&conn, &other, "coords", &entries(&[(1, "0")])).unwrap();
        assert_eq!(rows(&conn, &m).len(), 2);
        assert_eq!(
            read_one(&conn, &m, "coords", 1).unwrap(),
            Some(json!({"x": 9}))
        );
    }

    /// A document that is missing and one that is archived are told apart by nothing, on every call,
    /// and a write with one in the batch writes none.
    #[test]
    fn a_missing_and_an_unreadable_document_are_the_same_not_found_and_a_batch_is_all_or_nothing() {
        let (_dir, conn, m) = world();
        let gone = read_one(&conn, &m, "k", 99).unwrap_err();
        let shelved = read_one(&conn, &m, "k", 2).unwrap_err();
        assert_eq!(gone, shelved);
        assert_eq!(gone.code, Code::NotFound);
        for bad in [99, 2, -5] {
            let err = write_entries(&conn, &m, "k", &entries(&[(1, "1"), (bad, "2"), (3, "3")]))
                .unwrap_err();
            assert_eq!(err, gone, "document {bad}");
            assert!(rows(&conn, &m).is_empty(), "nothing written for {bad}");
        }
        // A listing never shows a shelved document's row, even one stored before it was shelved.
        write_entries(&conn, &m, "k", &entries(&[(1, "1"), (3, "3")])).unwrap();
        conn.execute(
            "INSERT INTO mod_doc_data(mod_id, document_id, key, value) VALUES ('acme.tool', 2, 'k', '2')",
            [],
        )
        .unwrap();
        let (page, next) = list_page(&conn, &m, "k", 0, 10, usize::MAX).unwrap();
        assert_eq!(page, vec![(1, json!(1)), (3, json!(3))]);
        assert_eq!(next, None);
    }

    #[test]
    fn a_key_declared_as_content_is_refused_and_an_undeclared_or_cache_one_is_fine() {
        let classes: BTreeMap<String, DataClass> = [
            ("settings".to_owned(), DataClass::Content),
            ("coords".to_owned(), DataClass::Cache),
        ]
        .into();
        assert_eq!(
            cache_key(&classes, "settings").unwrap_err().code,
            Code::BadArgs
        );
        assert!(cache_key(&classes, "coords").is_ok());
        assert!(cache_key(&classes, "undeclared").is_ok());
        assert_eq!(
            cache_key(&classes, "Bad Key").unwrap_err().code,
            Code::BadArgs
        );
    }

    #[test]
    fn the_quota_counts_the_batch_against_both_tables_and_lets_a_full_mod_shrink() {
        let (_dir, conn, m) = world();
        conn.execute(
            "INSERT INTO mod_kv(mod_id, key, class, value) VALUES ('acme.tool', 'a', 'content', '0')",
            [],
        )
        .unwrap();
        // Fill the mod to ten bytes under its quota with one big cache row on document 1.
        let used = kv_usage(&conn, &m, None).unwrap();
        let filler =
            "x".repeat((limits::STORAGE_QUOTA_BYTES - used - row_cost(4, 0) - 10) as usize);
        conn.execute(
            "INSERT INTO mod_doc_data(mod_id, document_id, key, value) VALUES ('acme.tool', 1, 'fill', ?1)",
            [&filler],
        )
        .unwrap();
        // Two small new rows don't fit in ten bytes, and nothing is written; one 1-byte row doesn't
        // either (its cost includes the row overhead).
        for batch in [entries(&[(1, "1"), (3, "1")]), entries(&[(1, "1")])] {
            assert_eq!(
                write_entries(&conn, &m, "k", &batch).unwrap_err().code,
                Code::QuotaExceeded
            );
        }
        assert_eq!(rows(&conn, &m).len(), 1, "only the filler");
        // Replacing the filler with something smaller always fits, even when full.
        write_entries(&conn, &m, "fill", &entries(&[(1, "0")])).unwrap();
        // And with room again the batch goes in.
        assert_eq!(
            write_entries(&conn, &m, "k", &entries(&[(1, "1"), (3, "1")])).unwrap(),
            2
        );
    }

    #[test]
    fn a_listing_pages_by_document_id_and_a_delete_takes_only_the_named_rows() {
        let (_dir, conn, m) = world();
        for n in 4..=8 {
            conn.execute(
                "INSERT INTO documents(id, vault_path, title, content_hash) VALUES (?1, ?2, 't', ?2)",
                params![n, format!("{n}.md")],
            )
            .unwrap();
        }
        let all: Vec<(i64, String)> = [1, 3, 4, 5, 6, 7, 8]
            .iter()
            .map(|d| (*d, format!("{d}")))
            .collect();
        write_entries(&conn, &m, "k", &all).unwrap();
        write_entries(&conn, &m, "j", &entries(&[(1, "\"j\"")])).unwrap();
        let mut seen = Vec::new();
        let mut after = 0;
        loop {
            let (page, next) = list_page(&conn, &m, "k", after, 3, usize::MAX).unwrap();
            assert!(page.len() <= 3);
            seen.extend(page.iter().map(|(d, _)| *d));
            match next {
                Some(n) => after = n,
                None => break,
            }
        }
        assert_eq!(seen, vec![1, 3, 4, 5, 6, 7, 8]);
        // The byte budget ends a page early, and always lets one row through.
        let (page, next) = list_page(&conn, &m, "k", 0, 10, 1).unwrap();
        assert_eq!((page.len(), next), (1, Some(1)));
        // Delete by id: only those, only this key.
        assert_eq!(delete_rows(&conn, &m, "k", Some(&[3, 4, 99])).unwrap(), 2);
        assert_eq!(rows(&conn, &m).len(), 6);
        // Without ids it clears the key, and leaves the others.
        assert_eq!(delete_rows(&conn, &m, "k", None).unwrap(), 5);
        assert_eq!(rows(&conn, &m), vec![(1, "j".into(), "\"j\"".into())]);
        assert_eq!(delete_rows(&conn, &m, "k", None).unwrap(), 0);
    }

    /// The point of the cascade: a deleted document takes its rows with it, with no frame open, and
    /// uninstalling the mod takes all of its rows.
    #[test]
    fn rows_vanish_with_their_document_and_with_their_mod() {
        let (_dir, conn, m) = world();
        write_entries(&conn, &m, "k", &entries(&[(1, "1"), (3, "3")])).unwrap();
        conn.execute("DELETE FROM documents WHERE id = 1", [])
            .unwrap();
        assert_eq!(rows(&conn, &m), vec![(3, "k".into(), "3".into())]);
        conn.execute("DELETE FROM mods WHERE id = 'acme.tool'", [])
            .unwrap();
        assert!(rows(&conn, &m).is_empty());
    }
}
