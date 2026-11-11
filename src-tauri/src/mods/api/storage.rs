// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A mod's private storage (Mods spec §1.5): `storage.get(key)` and
//! `storage.set(key, value, expectedRevision)` on `mod_kv`.
//!
//! - **Keys** follow the manifest's data-key grammar (`package::is_data_key`). A key the manifest
//!   declares takes its class from there (`content` or `cache`, which decides backups, D4); any
//!   other key is `content`, the class that is kept and backed up, so an undeclared key is never
//!   silently dropped from a backup.
//! - **Values** are any JSON, stored as its text.
//! - **Revisions.** A key's revision starts at 1 and goes up by one with every write; a key with no
//!   row reads as revision 0. A write names the revision it was based on, and one based on a stale
//!   copy (two frames of one mod, say) is refused with [`Code::StaleRevision`] and the current
//!   revision, so the mod re-reads instead of overwriting.
//! - **The quota** (`limits::STORAGE_QUOTA_BYTES`, over `mod_kv` and `mod_doc_data` together) is
//!   checked in the same transaction as the write, after the revision.
//!
//! [`kv_usage`] and [`doc_usage`] are what `storage.doc.*` (part 3) checks the same quota with.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use super::handlers::parse;
use super::limits::{self, quota_allows, row_cost};
use super::{blocking, recheck, still_current, Authorised, Code, Host, ModError, Reply};
use crate::mods::ids::ModId;
use crate::mods::package::{self, DataClass};
use crate::mods::store;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetArgs {
    key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SetArgs {
    key: String,
    value: Value,
    expected_revision: i64,
}

fn checked_key(key: &str) -> Result<(), ModError> {
    if package::is_data_key(key) {
        Ok(())
    } else {
        Err(ModError::bad_args(
            "A storage key is a lowercase letter, then up to 63 lowercase letters, digits, '_', '.' \
             or '-'.",
        ))
    }
}

/// `storage.get(key)` → `{value, revision}`: `null` and 0 for a key with nothing stored.
pub(super) async fn get<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let GetArgs { key } = parse(args)?;
    checked_key(&key)?;
    let (value, revision) = {
        let host = host.clone();
        let auth = auth.clone();
        blocking(move || {
            #[cfg(test)]
            host.at(super::Point::BeforeData);
            let conn = host.store().conn().map_err(|_| ModError::locked())?;
            recheck(&host, &conn, &auth)?;
            read(&conn, &auth.ticket.mod_id, &key)
        })
        .await?
    };
    still_current(host, auth)?;
    Ok(Reply::value(
        json!({ "value": value, "revision": revision }),
    ))
}

/// `storage.set(key, value, expectedRevision)` → `{revision}`, the key's new revision.
pub(super) async fn set<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let SetArgs {
        key,
        value,
        expected_revision,
    } = parse(args)?;
    checked_key(&key)?;
    if expected_revision < 0 {
        return Err(ModError::bad_args(
            "expectedRevision is the revision the value was read at: 0 or more.",
        ));
    }
    let text = serde_json::to_string(&value).map_err(ModError::internal)?;
    let revision = {
        let host = host.clone();
        let auth = auth.clone();
        blocking(move || {
            #[cfg(test)]
            host.at(super::Point::BeforeData);
            let mut conn = host.store().conn().map_err(|_| ModError::locked())?;
            let tx = conn.transaction().map_err(ModError::internal)?;
            // Under this transaction's guard: the mod as it is now, its class for the key from
            // that, and nothing written unless every check still holds.
            let facts = recheck(&host, &tx, &auth)?;
            let class = facts
                .data_classes
                .get(&key)
                .copied()
                .unwrap_or(DataClass::Content);
            let next = write(
                &tx,
                &auth.ticket.mod_id,
                &key,
                &text,
                class,
                expected_revision,
            )?;
            #[cfg(test)]
            host.at(super::Point::BeforeCommit);
            // Immediately before the commit: a bump since the recheck (the app lock, a pause's
            // registry half) rolls the write back with the transaction.
            still_current(&host, &auth)?;
            tx.commit().map_err(ModError::internal)?;
            Ok(next)
        })
        .await?
    };
    still_current(host, auth)?;
    Ok(Reply::value(json!({ "revision": revision })))
}

/// The stored value and revision of `key`, or `null` and 0.
fn read(conn: &Connection, id: &ModId, key: &str) -> Result<(Value, i64), ModError> {
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT value, revision FROM mod_kv WHERE mod_id = ?1 AND key = ?2",
            params![id.as_str(), key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(ModError::internal)?;
    match row {
        None => Ok((Value::Null, 0)),
        Some((text, revision)) => {
            let value = serde_json::from_str(&text).map_err(ModError::internal)?;
            Ok((value, revision))
        }
    }
}

/// Write `text` under `key` if `expected` is its current revision and the quota allows it; the
/// new revision. Inside the caller's transaction.
fn write(
    conn: &Connection,
    id: &ModId,
    key: &str,
    text: &str,
    class: DataClass,
    expected: i64,
) -> Result<i64, ModError> {
    let current: Option<(i64, i64)> = conn
        .query_row(
            "SELECT revision, length(CAST(value AS BLOB)) FROM mod_kv WHERE mod_id = ?1 AND key = ?2",
            params![id.as_str(), key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(ModError::internal)?;
    let revision = current.map_or(0, |(revision, _)| revision);
    if expected != revision {
        return Err(ModError::new(
            Code::StaleRevision,
            "That value changed since it was read: read it again.",
        )
        .with_revision(revision));
    }
    let key_bytes = key.len() as u64;
    let old = current.map_or(0, |(_, bytes)| row_cost(key_bytes, bytes.max(0) as u64));
    let new = row_cost(key_bytes, text.len() as u64);
    let others = kv_usage(conn, id, Some(key))?.saturating_add(doc_usage(conn, id, None)?);
    if !quota_allows(others, old, new, limits::STORAGE_QUOTA_BYTES) {
        return Err(ModError::new(
            Code::QuotaExceeded,
            "This mod's storage is full.",
        ));
    }
    let next = revision + 1;
    let now = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();
    conn.execute(
        "INSERT INTO mod_kv(mod_id, key, class, value, revision, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(mod_id, key) DO UPDATE SET \
           class = excluded.class, value = excluded.value, revision = excluded.revision, \
           updated_at = excluded.updated_at",
        params![id.as_str(), key, store::kv_class(class), text, next, now],
    )
    .map_err(ModError::internal)?;
    Ok(next)
}

/// What `id`'s `mod_kv` rows cost against the quota, leaving out `except`'s row.
pub(crate) fn kv_usage(
    conn: &Connection,
    id: &ModId,
    except: Option<&str>,
) -> Result<u64, ModError> {
    usage(
        conn,
        "SELECT COALESCE(SUM(length(CAST(key AS BLOB)) + length(CAST(value AS BLOB)) + ?3), 0) \
         FROM mod_kv WHERE mod_id = ?1 AND key IS NOT ?2",
        params![id.as_str(), except, limits::ROW_OVERHEAD_BYTES as i64],
    )
}

/// What `id`'s `mod_doc_data` rows cost against the quota, leaving out `except`'s `(document,
/// key)` row.
pub(crate) fn doc_usage(
    conn: &Connection,
    id: &ModId,
    except: Option<(i64, &str)>,
) -> Result<u64, ModError> {
    let (document, key) = except.unzip();
    usage(
        conn,
        "SELECT COALESCE(SUM(length(CAST(key AS BLOB)) + length(CAST(value AS BLOB)) + ?4), 0) \
         FROM mod_doc_data WHERE mod_id = ?1 AND NOT (document_id IS ?2 AND key IS ?3)",
        params![
            id.as_str(),
            document,
            key,
            limits::ROW_OVERHEAD_BYTES as i64
        ],
    )
}

fn usage(conn: &Connection, sql: &str, args: impl rusqlite::Params) -> Result<u64, ModError> {
    let total: i64 = conn
        .query_row(sql, args, |r| r.get(0))
        .map_err(ModError::internal)?;
    Ok(total.max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::api::test_support::{insert_mod, store, ModRow};

    fn id(s: &str) -> ModId {
        ModId::parse(s).unwrap()
    }

    #[test]
    fn keys_follow_the_manifests_grammar() {
        for key in ["a", "settings", "map.layout-v2", "x_1", &"a".repeat(64)] {
            assert!(checked_key(key).is_ok(), "{key}");
        }
        for key in [
            "",
            "A",
            "1a",
            "_a",
            ".a",
            "a b",
            "a/b",
            "a\0",
            "é",
            "a:b",
            &"a".repeat(65),
        ] {
            assert_eq!(checked_key(key).unwrap_err().code, Code::BadArgs, "{key:?}");
        }
    }

    #[test]
    fn a_write_needs_the_current_revision_and_bumps_it() {
        let (_dir, conn) = store();
        insert_mod(&conn, &ModRow::signed("acme.tool", "second", &["storage"]));
        let m = id("acme.tool");
        assert_eq!(read(&conn, &m, "k").unwrap(), (Value::Null, 0));
        // A first write must say it saw nothing.
        let stale = write(&conn, &m, "k", "1", DataClass::Content, 1).unwrap_err();
        assert_eq!((stale.code, stale.revision), (Code::StaleRevision, Some(0)));
        assert_eq!(
            write(&conn, &m, "k", "1", DataClass::Content, 0).unwrap(),
            1
        );
        assert_eq!(
            write(&conn, &m, "k", "{\"a\":2}", DataClass::Cache, 1).unwrap(),
            2
        );
        let stale = write(&conn, &m, "k", "3", DataClass::Content, 1).unwrap_err();
        assert_eq!((stale.code, stale.revision), (Code::StaleRevision, Some(2)));
        assert_eq!(read(&conn, &m, "k").unwrap(), (json!({"a": 2}), 2));
        let class: String = conn
            .query_row("SELECT class FROM mod_kv WHERE key = 'k'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(class, "cache", "the class follows each write");
        // Another mod's key of the same name is its own.
        insert_mod(&conn, &ModRow::signed("acme.other", "second", &["storage"]));
        assert_eq!(
            read(&conn, &id("acme.other"), "k").unwrap(),
            (Value::Null, 0)
        );
    }

    #[test]
    fn the_quota_counts_both_tables_in_bytes_and_lets_a_full_mod_shrink() {
        let (_dir, conn) = store();
        insert_mod(&conn, &ModRow::signed("acme.tool", "second", &["storage"]));
        let m = id("acme.tool");
        write(&conn, &m, "a", "\"é\"", DataClass::Content, 0).unwrap();
        // "é" is two bytes, so the JSON text is four: 1 key byte + 4 + the row overhead.
        assert_eq!(
            kv_usage(&conn, &m, None).unwrap(),
            1 + 4 + limits::ROW_OVERHEAD_BYTES
        );
        assert_eq!(kv_usage(&conn, &m, Some("a")).unwrap(), 0);
        conn.execute_batch(
            "INSERT INTO documents(id, vault_path, title, content_hash)
               VALUES (41, 'a.md', 't', 'h1'), (42, 'b.md', 'u', 'h2');
             INSERT INTO mod_doc_data(mod_id, document_id, key, value) VALUES
               ('acme.tool', 41, 'xy', '[1,2]'), ('acme.tool', 42, 'xy', '0');",
        )
        .unwrap();
        let both = 2 * 2 + 5 + 1 + 2 * limits::ROW_OVERHEAD_BYTES;
        assert_eq!(doc_usage(&conn, &m, None).unwrap(), both);
        assert_eq!(
            doc_usage(&conn, &m, Some((41, "xy"))).unwrap(),
            2 + 1 + limits::ROW_OVERHEAD_BYTES
        );
        // Fill the mod to just under the quota with one big cache row in mod_doc_data.
        let used = kv_usage(&conn, &m, None).unwrap() + doc_usage(&conn, &m, None).unwrap();
        let room = limits::STORAGE_QUOTA_BYTES - used;
        let filler = "x".repeat((room - row_cost(3, 0) - 10) as usize);
        conn.execute(
            "INSERT INTO mod_doc_data(mod_id, document_id, key, value) VALUES ('acme.tool', 41, 'big', ?1)",
            [&filler],
        )
        .unwrap();
        // 10 bytes left: a new key "b" costs 1 + its text + overhead, which doesn't fit.
        let full = write(&conn, &m, "b", "1", DataClass::Content, 0).unwrap_err();
        assert_eq!(full.code, Code::QuotaExceeded);
        assert_eq!(
            read(&conn, &m, "b").unwrap(),
            (Value::Null, 0),
            "nothing written"
        );
        // Growing "a" by more than 10 bytes doesn't fit either; by 10 it does.
        assert_eq!(
            write(
                &conn,
                &m,
                "a",
                &format!("\"é{}\"", "y".repeat(11)),
                DataClass::Content,
                1
            )
            .unwrap_err()
            .code,
            Code::QuotaExceeded
        );
        assert_eq!(
            write(
                &conn,
                &m,
                "a",
                &format!("\"é{}\"", "y".repeat(10)),
                DataClass::Content,
                1
            )
            .unwrap(),
            2
        );
        // Full now; shrinking still works.
        assert_eq!(
            write(&conn, &m, "a", "0", DataClass::Content, 2).unwrap(),
            3
        );
        // Another mod's storage isn't this one's.
        insert_mod(&conn, &ModRow::signed("acme.other", "second", &["storage"]));
        assert_eq!(
            write(&conn, &id("acme.other"), "b", "1", DataClass::Content, 0).unwrap(),
            1
        );
    }
}
