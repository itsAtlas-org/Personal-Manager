// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a mod may know about documents (Mods spec §1.5), with the `documents.meta` permission:
//! `documents.list(fields)`, `ui.openDocument(id)`, and the one test of whether a document is the
//! caller's to touch ([`readable`]) that `storage.doc.*` shares.
//!
//! **A narrow DTO, built by construction.** A document's row has paths, an external reference, an
//! account-namespaced source id, folder names. None of those is a field here: [`Field`] is the whole
//! vocabulary, a field the DTO doesn't have is refused as a bad argument (never silently dropped, so a
//! typo can't mean something else), and each document is built field by field from the row's own
//! columns, never from the full `Document`. What a mod gets is `id`, `title`, `homeProject` (its id
//! and name), `importance`, `chunkCount`, `reviewed`, `sourceType`, `tags` and `ingestedAt`, each only
//! when asked for.
//!
//! **The same documents the core Map shows**: every document but an archived one (the user shelved
//! it; it stays keyword-findable but is off the Map). PM has no trash for documents: a deleted one is
//! gone. That set ([`LISTED`]) is also what is *readable*: an archived document is unreachable to a
//! mod by id as well, and, like one that was never there, is answered "not found" ([`not_found`]),
//! so a call can't be used to tell a shelved document from a missing one.
//!
//! **Paged.** A page is at most `limit` documents (default and maximum
//! [`limits::MAX_DOCS_PER_CALL`], the Map's largest node cap) and ends early, under
//! [`limits::LIST_PAGE_BUDGET_BYTES`], so no page is refused as too large. Pages are keyed by id
//! (`after` is the last id seen, `next` the cursor or `null` at the end): a document landing or going
//! between two pages can't make one repeat or skip another.

use std::collections::HashSet;

use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::handlers::parse;
use super::limits;
use super::{
    blocking, recheck, still_current, Authorised, Code, Host, ModError, PageAction, Reply,
};

/// The SQL test (on a `documents d` row) for the documents a mod may list and read.
pub(super) const LISTED: &str = "d.importance IS NOT 'archive'";

/// A field of the list DTO: all of it. Adding one is a decision about what mods may know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Field {
    Id,
    Title,
    HomeProject,
    Importance,
    ChunkCount,
    Reviewed,
    SourceType,
    Tags,
    IngestedAt,
}

impl Field {
    #[cfg(test)]
    const ALL: [Field; 9] = [
        Self::Id,
        Self::Title,
        Self::HomeProject,
        Self::Importance,
        Self::ChunkCount,
        Self::Reviewed,
        Self::SourceType,
        Self::Tags,
        Self::IngestedAt,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Title => "title",
            Self::HomeProject => "homeProject",
            Self::Importance => "importance",
            Self::ChunkCount => "chunkCount",
            Self::Reviewed => "reviewed",
            Self::SourceType => "sourceType",
            Self::Tags => "tags",
            Self::IngestedAt => "ingestedAt",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    fields: Vec<Field>,
    #[serde(default)]
    after: Option<i64>,
    #[serde(default)]
    limit: Option<usize>,
}

/// The columns of one listed document, as the query reads them: only what [`Field`] can name.
struct Doc {
    id: i64,
    title: String,
    project: String,
    project_id: Option<i64>,
    importance: Option<String>,
    chunk_count: i64,
    reviewed: bool,
    source_type: String,
    tags_json: String,
    ingested_at: String,
}

/// One document as a mod gets it: `fields` and nothing else.
fn dto(doc: &Doc, fields: &[Field]) -> Value {
    let mut out = Map::new();
    for field in fields {
        let value = match field {
            Field::Id => json!(doc.id),
            Field::Title => json!(doc.title),
            // The project's registry id and its name as the user typed it; `null` for a document
            // with no home. The id is the project tag's: `null` while the registry row is still
            // catching up with a just-filed document.
            Field::HomeProject => {
                let name = doc.project.trim();
                if name.is_empty() {
                    Value::Null
                } else {
                    json!({ "id": doc.project_id, "name": name })
                }
            }
            Field::Importance => json!(doc.importance),
            Field::ChunkCount => json!(doc.chunk_count),
            Field::Reviewed => json!(doc.reviewed),
            Field::SourceType => json!(doc.source_type),
            Field::Tags => {
                // As the core list reads them: a column that isn't a list of strings is no tags.
                let tags: Vec<String> = serde_json::from_str(&doc.tags_json).unwrap_or_default();
                json!(tags)
            }
            Field::IngestedAt => json!(doc.ingested_at),
        };
        out.insert(field.name().to_owned(), value);
    }
    Value::Object(out)
}

/// A page of listed documents after `after`, as DTOs of `fields`: at most `limit`, and ending early
/// under the byte budget. The cursor is the last id returned, `None` when nothing follows.
fn list_page(
    conn: &Connection,
    fields: &[Field],
    after: i64,
    limit: usize,
    budget: usize,
) -> Result<(Vec<Value>, Option<i64>), ModError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT d.id, d.title, d.project, \
                    (SELECT t.id FROM tags t \
                      WHERE t.kind = 'project' AND t.norm = lower(trim(d.project))), \
                    d.importance, \
                    (SELECT count(*) FROM chunks c WHERE c.document_id = d.id), \
                    d.reviewed, d.source_type, d.tags, d.ingested_at \
             FROM documents d WHERE d.id > ?1 AND {LISTED} ORDER BY d.id LIMIT ?2"
        ))
        .map_err(ModError::internal)?;
    // One past the page, to know whether anything follows it.
    let rows = stmt
        .query_map(params![after, limit as i64 + 1], |r| {
            Ok(Doc {
                id: r.get(0)?,
                title: r.get(1)?,
                project: r.get(2)?,
                project_id: r.get(3)?,
                importance: r.get(4)?,
                chunk_count: r.get(5)?,
                reviewed: r.get::<_, i64>(6)? != 0,
                source_type: r.get(7)?,
                tags_json: r.get(8)?,
                ingested_at: r.get(9)?,
            })
        })
        .map_err(ModError::internal)?;
    let mut page: Vec<Value> = Vec::new();
    let mut used = 0usize;
    let mut last = None;
    for doc in rows {
        let doc = doc.map_err(ModError::internal)?;
        if page.len() == limit {
            return Ok((page, last));
        }
        let value = dto(&doc, fields);
        let size = serde_json::to_string(&value)
            .map_err(ModError::internal)?
            .len()
            + 1;
        // A page ends before the document that would pass the budget; a first document always goes
        // in, so a page is never empty while documents remain.
        if !page.is_empty() && used + size > budget {
            return Ok((page, last));
        }
        used += size;
        last = Some(doc.id);
        page.push(value);
    }
    Ok((page, None))
}

/// `documents.list(fields, after?, limit?)` → `{documents, next}`.
pub(super) async fn list<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let ListArgs {
        fields,
        after,
        limit,
    } = parse(args)?;
    if fields.is_empty() {
        return Err(ModError::bad_args(
            "Name the fields to list: id, title, homeProject, importance, chunkCount, reviewed, \
             sourceType, tags, ingestedAt.",
        ));
    }
    let mut seen = HashSet::new();
    if !fields.iter().all(|f| seen.insert(f.name())) {
        return Err(ModError::bad_args("A field is named twice."));
    }
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
    let (documents, next) = {
        let host = host.clone();
        let auth = auth.clone();
        blocking(move || {
            #[cfg(test)]
            host.at(super::Point::BeforeData);
            let conn = host.store().conn().map_err(|_| ModError::locked())?;
            recheck(&host, &conn, &auth)?;
            list_page(&conn, &fields, after, limit, limits::LIST_PAGE_BUDGET_BYTES)
        })
        .await?
    };
    still_current(host, auth)?;
    Ok(Reply::value(
        json!({ "documents": documents, "next": next }),
    ))
}

/// The error for a document the caller can't have: missing, shelved and unreadable all read alike.
pub(super) fn not_found() -> ModError {
    ModError::new(Code::NotFound, "There is no such document.")
}

/// Which of `ids` are documents the caller may touch (the listed set). One query for any number.
pub(super) fn readable(conn: &Connection, ids: &[i64]) -> Result<HashSet<i64>, ModError> {
    let ids = serde_json::to_string(ids).map_err(ModError::internal)?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT d.id FROM documents d \
             WHERE {LISTED} AND d.id IN (SELECT value FROM json_each(?1))"
        ))
        .map_err(ModError::internal)?;
    let rows = stmt
        .query_map([ids], |r| r.get::<_, i64>(0))
        .map_err(ModError::internal)?;
    rows.collect::<rusqlite::Result<HashSet<_>>>()
        .map_err(ModError::internal)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenArgs {
    id: i64,
}

/// `ui.openDocument(id)`: Rust finds the document readable, then PM's page opens its own reader on it
/// (`ModFrame.tsx`). A document that is missing, shelved or otherwise unreadable is the same
/// [`not_found`], and nothing is opened.
pub(super) async fn open_document<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let OpenArgs { id } = parse(args)?;
    let found = {
        let host = host.clone();
        let auth = auth.clone();
        blocking(move || {
            #[cfg(test)]
            host.at(super::Point::BeforeData);
            let conn = host.store().conn().map_err(|_| ModError::locked())?;
            recheck(&host, &conn, &auth)?;
            Ok(readable(&conn, &[id])?.contains(&id))
        })
        .await?
    };
    still_current(host, auth)?;
    if !found {
        return Err(not_found());
    }
    Ok(Reply::Page {
        action: PageAction::UiOpenDocument,
        args: json!({ "id": id }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::api::test_support::store;

    /// Names a mod must never be able to read, whichever it asks for.
    const FORBIDDEN: [&str; 14] = [
        "source_path",
        "sourcePath",
        "external_ref",
        "externalRef",
        "source_id",
        "sourceId",
        "source_parent_folder_name",
        "sourceParentFolderName",
        "source_folder_path",
        "sourceFolderPath",
        "source_account",
        "sourceAccount",
        "vault_path",
        "vaultPath",
    ];

    fn add(conn: &Connection, id: i64, importance: Option<&str>, project: &str) {
        conn.execute(
            "INSERT INTO documents(id, vault_path, title, content_hash, project, importance, tags, \
                                   source_path, external_ref, source_id, source_parent_folder_name, \
                                   source_account, source_folder_path) \
             VALUES (?1, ?2, ?3, ?2, ?4, ?5, '[\"a\",\"b\"]', '/home/me/secret.md', \
                     'https://drive.example/x', 'gdrive:me@example.com:f' || ?1, 'Private folder', \
                     'me@example.com', '[\"Private folder\"]')",
            params![id, format!("d{id}.md"), format!("Doc {id}"), project, importance],
        )
        .unwrap();
    }

    fn fields(names: &[&str]) -> Vec<Field> {
        serde_json::from_value(json!(names)).unwrap()
    }

    #[test]
    fn a_dto_has_exactly_the_requested_fields_and_never_a_forbidden_one() {
        let (_dir, conn) = store();
        add(&conn, 7, Some("high"), "Atlas");
        conn.execute(
            "INSERT INTO tags(kind, name, norm) VALUES ('project', 'Atlas', 'atlas')",
            [],
        )
        .unwrap();
        let project_tag: i64 = conn
            .query_row("SELECT id FROM tags WHERE norm = 'atlas'", [], |r| r.get(0))
            .unwrap();
        // Every field at once.
        let all: Vec<&str> = Field::ALL.iter().map(|f| f.name()).collect();
        let (page, next) = list_page(&conn, &fields(&all), 0, 10, usize::MAX).unwrap();
        assert_eq!(next, None);
        assert_eq!(
            page,
            vec![json!({
                "id": 7, "title": "Doc 7",
                "homeProject": {"id": project_tag, "name": "Atlas"},
                "importance": "high", "chunkCount": 0, "reviewed": false,
                "sourceType": "vault", "tags": ["a", "b"],
                "ingestedAt": page[0]["ingestedAt"],
            })]
        );
        // Each field alone gives only itself.
        for field in Field::ALL {
            let (page, _) = list_page(&conn, &[field], 0, 10, usize::MAX).unwrap();
            let keys: Vec<&String> = page[0].as_object().unwrap().keys().collect();
            assert_eq!(keys, vec![field.name()]);
        }
        // Nothing of the secret columns appears anywhere in what a mod could be sent, whichever
        // fields it asked for.
        let text = serde_json::to_string(&page).unwrap();
        for leaked in [
            "secret.md",
            "drive.example",
            "me@example.com",
            "Private folder",
            "gdrive:",
        ] {
            assert!(!text.contains(leaked), "{leaked} reached the DTO");
        }
    }

    /// Asking for a forbidden field by name, in either spelling, is refused whole: no field is
    /// quietly dropped, and no document is read.
    #[test]
    fn a_forbidden_field_asked_for_by_name_is_refused() {
        for name in FORBIDDEN {
            for list in [vec![name], vec!["id", name]] {
                let args = json!({ "fields": list });
                let err = parse::<ListArgs>(&args).map(|_| ()).unwrap_err();
                assert_eq!(err.code, Code::BadArgs, "{name}");
            }
        }
        // Nor does a field by another case or a nested path reach one.
        for name in ["ID", "Title", "home_project", "homeProject.id", "", "*"] {
            let args = json!({ "fields": [name] });
            assert!(parse::<ListArgs>(&args).is_err(), "{name:?}");
        }
        // And no argument names a column, a table or a filter of its own.
        for args in [
            json!({"fields": ["id"], "where": "1=1"}),
            json!({"fields": ["id"], "orderBy": "source_path"}),
            json!({"fields": "id"}),
            json!({}),
        ] {
            assert!(parse::<ListArgs>(&args).is_err(), "{args}");
        }
    }

    #[test]
    fn an_archived_document_is_not_listed_and_not_readable_and_looks_like_a_missing_one() {
        let (_dir, conn) = store();
        add(&conn, 1, None, "Atlas");
        add(&conn, 2, Some("archive"), "Atlas");
        add(&conn, 3, Some("low"), "Atlas");
        let ids = |after, limit| {
            list_page(&conn, &fields(&["id"]), after, limit, usize::MAX)
                .unwrap()
                .0
                .iter()
                .map(|d| d["id"].as_i64().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(0, 10), vec![1, 3]);
        let got = readable(&conn, &[1, 2, 3, 4, -1]).unwrap();
        assert_eq!(got, HashSet::from([1, 3]));
        assert!(readable(&conn, &[]).unwrap().is_empty());
        // The one error for both: missing (4) and shelved (2) are told apart by nothing.
        assert_eq!(not_found().code, Code::NotFound);
    }

    #[test]
    fn pages_follow_by_id_without_repeating_or_skipping() {
        let (_dir, conn) = store();
        for id in 1..=7 {
            add(&conn, id, None, "Atlas");
        }
        let f = fields(&["id"]);
        let mut seen = Vec::new();
        let mut after = 0;
        let mut pages = 0;
        loop {
            let (page, next) = list_page(&conn, &f, after, 3, usize::MAX).unwrap();
            pages += 1;
            seen.extend(page.iter().map(|d| d["id"].as_i64().unwrap()));
            match next {
                Some(n) => after = n,
                None => break,
            }
        }
        assert_eq!((seen, pages), ((1..=7).collect::<Vec<_>>(), 3));
        // A page that ends exactly at the end has no cursor.
        assert_eq!(list_page(&conn, &f, 0, 7, usize::MAX).unwrap().1, None);
        assert_eq!(list_page(&conn, &f, 4, 3, usize::MAX).unwrap().1, None);
        assert_eq!(
            list_page(&conn, &f, 7, 3, usize::MAX).unwrap(),
            (vec![], None)
        );
    }

    #[test]
    fn a_page_ends_early_under_its_byte_budget_and_is_never_empty() {
        let (_dir, conn) = store();
        for id in 1..=5 {
            add(&conn, id, None, "Atlas");
        }
        let f = fields(&["id", "title"]);
        let one = serde_json::to_string(&json!({"id": 1, "title": "Doc 1"}))
            .unwrap()
            .len()
            + 1;
        // Room for two documents: the third waits for the next page.
        let (page, next) = list_page(&conn, &f, 0, 5, 2 * one).unwrap();
        assert_eq!((page.len(), next), (2, Some(2)));
        // A budget smaller than one document still lets one through, so the list always advances.
        let (page, next) = list_page(&conn, &f, 2, 5, 1).unwrap();
        assert_eq!((page.len(), next), (1, Some(3)));
    }

    #[test]
    fn a_home_project_is_null_when_blank_and_has_no_id_until_the_registry_has_one() {
        let (_dir, conn) = store();
        add(&conn, 1, None, "  ");
        add(&conn, 2, None, "Fresh");
        let (page, _) = list_page(&conn, &fields(&["homeProject"]), 0, 10, usize::MAX).unwrap();
        assert_eq!(page[0]["homeProject"], Value::Null);
        assert_eq!(page[1]["homeProject"], json!({"id": null, "name": "Fresh"}));
    }
}
