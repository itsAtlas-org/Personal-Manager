// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A person's links to the places their record came from (`person_sources`): a vCard file, and in
//! the Google PR a Google contact. This module owns the one rule that keeps those links from
//! outliving the person's data: [`retire`] and the tombstone it leaves (spec 2.2).
//!
//! A purge deletes the person, but a Google link must not simply vanish: the next sync would list
//! that contact again and bring the person straight back. So a Google source is kept as a
//! **tombstone** holding ids only (Google's resource name, the contact's source id, and the random
//! `uid`s the person went by), and every file source is deleted outright. No row with a name, an
//! email or a note outlives its person.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};

use super::{require_transaction, PROVIDER_GOOGLE, STATE_IGNORED, STATE_PENDING_DELETE};
use crate::error::Result;

/// One link as the record shows it ("Imported from a file on 12-11-2026"). Never carries the
/// stored snapshot, which is a copy of the source's contact and so holds contact values.
#[derive(Clone, Serialize)]
pub struct SourceInfo {
    pub id: i64,
    pub provider: String,
    /// The account the link belongs to (a lowercased address; empty for a file).
    #[serde(rename = "account")]
    pub acct: String,
    pub state: String,
    pub synced_at: Option<String>,
    pub created_at: String,
    /// PM changed something since the last write to the source.
    pub dirty: bool,
    /// How many "same field changed" questions are open.
    pub open_conflicts: usize,
}

impl std::fmt::Debug for SourceInfo {
    /// Ids and a count only, like every type that sits near contact values.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceInfo")
            .field("id", &self.id)
            .field("open_conflicts", &self.open_conflicts)
            .finish_non_exhaustive()
    }
}

/// Every id a person goes by: their own `uid` first, then each uid folded into them by a merge.
/// `None` when no such person exists.
pub fn idents_of(conn: &Connection, person_id: i64) -> Result<Option<Vec<String>>> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT uid, merged_uids FROM people WHERE id = ?1",
            params![person_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row.map(|(own, folded)| {
        let mut all = vec![own];
        all.extend(parse_ident_list(folded.as_deref()));
        all
    }))
}

/// The strings of a JSON array column (`people.merged_uids`), or none for NULL or damage: a
/// malformed value must cost a lookup, never fail a purge.
pub fn parse_ident_list(json_text: Option<&str>) -> Vec<String> {
    json_text
        .and_then(|t| serde_json::from_str::<Value>(t).ok())
        .and_then(|v| match v {
            Value::Array(items) => Some(
                items
                    .into_iter()
                    .filter_map(|i| i.as_str().map(str::to_string))
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

/// Whether a snapshot already has the tombstone shape: an object holding `pm_uids` (an array) and
/// nothing beyond that and `source_id`.
pub fn is_tombstone(snapshot: &Value) -> bool {
    match snapshot.as_object() {
        Some(o) => {
            o.get("pm_uids").is_some_and(Value::is_array)
                && o.keys().all(|k| k == "pm_uids" || k == "source_id")
        }
        None => false,
    }
}

/// The Google CONTACT source id held in a stored snapshot (the Google person's `metadata.sources`
/// entry of type CONTACT), or `None`. Google's id for the contact survives a change of resource
/// name, which is why a tombstone keeps it.
fn contact_source_id(snapshot: &Value) -> Option<String> {
    let sources = snapshot
        .pointer("/metadata/sources")
        .or_else(|| snapshot.get("sources"))?
        .as_array()?;
    sources
        .iter()
        .find(|s| s.get("type").and_then(Value::as_str) == Some("CONTACT"))
        .and_then(|s| s.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The snapshot a tombstone holds: `{"pm_uids": [...], "source_id": ...}`, random ids only.
///
/// Idempotent: handed a snapshot that already is a tombstone it keeps that `source_id` and takes
/// the union of its `pm_uids` with `idents`, so retiring twice (a purge after a merge, a retry after
/// a crash) never loses the id that lets a later listing recognise the contact.
pub fn tombstone_snapshot(old: Option<&str>, idents: &[String]) -> String {
    let parsed: Option<Value> = old.and_then(|s| serde_json::from_str(s).ok());
    let mut pm_idents: Vec<String> = Vec::new();
    let mut source_id: Option<String> = None;
    if let Some(v) = &parsed {
        if is_tombstone(v) {
            pm_idents = v["pm_uids"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|i| i.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            source_id = v
                .get("source_id")
                .and_then(Value::as_str)
                .map(str::to_string);
        } else {
            source_id = contact_source_id(v);
        }
    }
    for ident in idents {
        if !pm_idents.contains(ident) {
            pm_idents.push(ident.clone());
        }
    }
    json!({ "pm_uids": pm_idents, "source_id": source_id }).to_string()
}

/// Retire the sources of people about to be deleted (spec 2.2 step 2). Every file source is
/// deleted; every Google source, a pending create included, becomes a tombstone: `state` is
/// `ignored` (a `pending_delete` keeps its state, so the delete is still sent), `etag`, `conflicts`
/// and `dirty_at` are cleared, and the snapshot is replaced by [`tombstone_snapshot`]. The link row
/// keeps its provider, account, external id and dates, which are ids and not personal data.
///
/// Must run in the caller's transaction, before the people rows go (the FK then nulls `person_id`
/// on the tombstones). A person with no sources, or an unknown id, is a no-op.
pub fn retire(tx: &Connection, person_ids: &[i64]) -> Result<()> {
    require_transaction(tx)?;
    for &person_id in person_ids {
        let Some(idents) = idents_of(tx, person_id)? else {
            continue;
        };
        tx.execute(
            "DELETE FROM person_sources WHERE person_id = ?1 AND provider <> ?2",
            params![person_id, PROVIDER_GOOGLE],
        )?;
        let mut stmt = tx.prepare(
            "SELECT id, state, snapshot FROM person_sources \
              WHERE person_id = ?1 AND provider = ?2",
        )?;
        let rows = stmt
            .query_map(params![person_id, PROVIDER_GOOGLE], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);
        for (source_id, state, snapshot) in rows {
            let next_state = if state == STATE_PENDING_DELETE {
                state
            } else {
                STATE_IGNORED.to_string()
            };
            tx.execute(
                "UPDATE person_sources \
                    SET state = ?2, etag = NULL, conflicts = NULL, dirty_at = NULL, snapshot = ?3 \
                  WHERE id = ?1",
                params![
                    source_id,
                    next_state,
                    tombstone_snapshot(snapshot.as_deref(), &idents)
                ],
            )?;
        }
    }
    Ok(())
}

/// How many links "Forget this account's links" removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ForgottenLinks {
    /// Rows of `person_sources`: linked, copied, waiting and tombstoned alike.
    pub people: usize,
    /// Rows of `label_sources`.
    pub labels: usize,
}

/// Delete every Google link of one account: its `person_sources` (tombstones and waiting Google
/// deletes too) and its `label_sources`. `acct` is the lowercased address the rows are stored under.
///
/// Only the links go. No person, label, detail or timeline row is touched, and nothing is sent to
/// Google, so a waiting delete is dropped rather than carried out. The caller stops the account's
/// sync first, or the next pass would list every contact again and bring each one back as a new
/// person. Must run in the caller's transaction.
pub fn forget_account_links_in(tx: &Connection, acct: &str) -> Result<ForgottenLinks> {
    require_transaction(tx)?;
    let people = tx.execute(
        "DELETE FROM person_sources WHERE provider = ?1 AND account = ?2",
        params![PROVIDER_GOOGLE, acct],
    )?;
    let labels = tx.execute(
        "DELETE FROM label_sources WHERE provider = ?1 AND account = ?2",
        params![PROVIDER_GOOGLE, acct],
    )?;
    Ok(ForgottenLinks { people, labels })
}

/// [`forget_account_links_in`] in a transaction of its own.
pub fn forget_account_links(conn: &Connection, acct: &str) -> Result<ForgottenLinks> {
    super::in_tx(conn, |tx| forget_account_links_in(tx, acct))
}

/// A person's links, oldest first.
pub fn sources_of(conn: &Connection, person_id: i64) -> Result<Vec<SourceInfo>> {
    let mut stmt = conn.prepare(
        "SELECT id, provider, account, state, synced_at, created_at, dirty_at IS NOT NULL, conflicts \
           FROM person_sources WHERE person_id = ?1 ORDER BY id",
    )?;
    let rows = stmt
        .query_map(params![person_id], |r| {
            let conflicts: Option<String> = r.get(7)?;
            Ok(SourceInfo {
                id: r.get(0)?,
                provider: r.get(1)?,
                acct: r.get(2)?,
                state: r.get(3)?,
                synced_at: r.get(4)?,
                created_at: r.get(5)?,
                dirty: r.get(6)?,
                open_conflicts: parse_ident_list_len(conflicts.as_deref()),
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// The length of a JSON array column, 0 for NULL or damage.
fn parse_ident_list_len(json_text: Option<&str>) -> usize {
    json_text
        .and_then(|t| serde_json::from_str::<Value>(t).ok())
        .and_then(|v| v.as_array().map(Vec::len))
        .unwrap_or(0)
}

/// The person a provider's item is linked to: `(provider, account, external id)` is unique among
/// links that have an id. A purged tombstone (no person) answers `None`; a `state = 'ignored'`
/// tombstone of a delete Google confirmed keeps its `person_id` until the purge, so it still
/// answers `Some`. The sync (PR B) must read `state` before it re-links a contact through this,
/// and [`sources_of`] and the list's `google_state` likewise report such a row as the person's link.
pub fn person_for_source(
    conn: &Connection,
    provider: &str,
    acct: &str,
    external_id: &str,
) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT person_id FROM person_sources \
              WHERE provider = ?1 AND account = ?2 AND external_id = ?3 AND person_id IS NOT NULL",
            params![provider, acct, external_id],
            |r| r.get(0),
        )
        .optional()?)
}

/// The person an id belongs to, and whether they are in Trash: their own `uid`, or a uid folded
/// into them by a merge (so a file exported before the merge still finds the survivor). A person in
/// Trash is still found: restoring them is the import's job (E4). The import asks the two halves
/// apart (own uid, then folded uids); this is the one-call form the tests pin.
#[cfg(test)]
pub fn person_for_ident(conn: &Connection, ident: &str) -> Result<Option<(i64, bool)>> {
    if let Some(found) = person_for_own_ident(conn, ident)? {
        return Ok(Some(found));
    }
    Ok(conn
        .query_row(
            "SELECT id, trashed_at IS NOT NULL FROM people \
              WHERE merged_uids IS NOT NULL \
                AND EXISTS (SELECT 1 FROM json_each(merged_uids) WHERE value = ?1) LIMIT 1",
            params![ident],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// The person whose own `uid` this is (an index lookup), and whether they are in Trash. A bulk
/// caller asks this per card and takes the folded ids from [`merged_ident_owners`] once, because
/// the folded ids sit in a JSON column no index reaches.
pub fn person_for_own_ident(conn: &Connection, ident: &str) -> Result<Option<(i64, bool)>> {
    Ok(conn
        .query_row(
            "SELECT id, trashed_at IS NOT NULL FROM people WHERE uid = ?1",
            params![ident],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// Every uid a merge folded into someone, and the person it folded into.
pub fn merged_ident_owners(conn: &Connection) -> Result<std::collections::HashMap<String, i64>> {
    let mut stmt = conn.prepare(
        "SELECT j.value, p.id FROM people p, json_each(p.merged_uids) j \
          WHERE p.merged_uids IS NOT NULL ORDER BY p.id DESC",
    )?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_contact_source_id_is_read_from_the_google_shape() {
        let snap = json!({"metadata": {"sources": [
            {"type": "PROFILE", "id": "p1"},
            {"type": "CONTACT", "id": "c42", "etag": "e"}
        ]}, "names": [{"givenName": "Invented"}]})
        .to_string();
        let out: Value =
            serde_json::from_str(&tombstone_snapshot(Some(&snap), &["u1".to_string()])).unwrap();
        assert_eq!(out["source_id"], "c42");
        assert_eq!(out["pm_uids"], json!(["u1"]));
        assert_eq!(out.as_object().unwrap().len(), 2, "ids only");
    }

    #[test]
    fn a_tombstone_is_recognised_by_its_shape() {
        assert!(is_tombstone(&json!({"pm_uids": [], "source_id": null})));
        assert!(is_tombstone(&json!({"pm_uids": ["a"]})));
        assert!(!is_tombstone(&json!({"pm_uids": [], "names": []})));
        assert!(!is_tombstone(&json!([1])));
    }

    #[test]
    fn a_damaged_snapshot_still_leaves_a_clean_tombstone() {
        let out: Value =
            serde_json::from_str(&tombstone_snapshot(Some("{not json"), &["u1".to_string()]))
                .unwrap();
        assert_eq!(out, json!({"pm_uids": ["u1"], "source_id": null}));
    }
}
