// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `documents.search` (experimental, PR C3): PM's own hybrid retrieval, asked a question by a mod
//! holding `documents.text`, and answered with short excerpts and nothing that locates a file.
//!
//! **What may be found.** Never a chat (the user's conversations), never an archived document (the
//! shelf `documents.list` leaves out too), and never an index-only document with no summary (its
//! chunk text is a placeholder). The filter is the retrieval's own ([`retrieval::Filters::mod_visible`]),
//! applied to BOTH branches before the pool is cut, so a page backfills past what is hidden instead of
//! coming back short; this module never post-filters a shrunken pool. An index-only hit that survives
//! carries a model-written SUMMARY, not the file's text, and says so (`indexOnly`).
//!
//! **What a hit tells.** `{documentId, title, heading, snippet, indexOnly}` and, with `projects.read`,
//! `project`. No score, no path (`source_path`, `vault_path`), no `source_id`/external ref, no chunk,
//! conversation or turn id, and never a vector. Title and heading are cut at [`LABEL_MAX_CHARS`] and
//! the snippet at [`SNIPPET_MAX_CHARS`], so one hostile row can't make a page too large. A snippet is
//! the user's own text: a mod that forwards it to `ai.complete` forwards untrusted text.
//!
//! **The heavy step** embeds the query, so it goes through [`Host::search`] (the sidecar), after the
//! same engine check `documents.saveNote` makes (`MOD-5007`; PM never sets the engine up for a mod),
//! with no DB guard held, under the call timeout, and asks again whether the call is current after
//! it. The embed family's limits admit it from the row.
//!
//! The pieces below ([`Query::parse`], [`hits`], [`visible_pool`]) take a [`Projection`], not a mod,
//! so the MCP server builds the same answer from the scopes its client was given.

use std::collections::HashMap;

use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::Value;

use super::handlers::{parse_object, within_call_timeout};
use super::projection::{Dto, Needs, Projection};
use super::{blocking, recheck, still_current, Authorised, Host, ModError, Reply};
use crate::error::Error;
use crate::retrieval::{self, RetrievedChunk};

/// The longest a query is, in characters.
pub(crate) const QUERY_MAX_CHARS: usize = 2000;
/// How many hits a page has when `limit` is left out, and the most it may have.
pub(crate) const DEFAULT_LIMIT: usize = 10;
pub(crate) const MAX_LIMIT: usize = 20;
/// The longest a snippet is, in characters.
pub(crate) const SNIPPET_MAX_CHARS: usize = 1000;
/// The longest a hit's title or heading is, in characters: a document's title is whatever its file
/// or feed says, so the page is bounded by this, not by luck.
pub(crate) const LABEL_MAX_CHARS: usize = 200;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

/// A search's arguments, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Query {
    pub text: String,
    pub limit: usize,
}

impl Query {
    /// The arguments as a query: `query` a non-blank string of at most [`QUERY_MAX_CHARS`], `limit` an
    /// integer 1 to [`MAX_LIMIT`]. Nothing is trimmed: the text goes to the embedder as given.
    pub(crate) fn parse(args: &Value) -> Result<Self, ModError> {
        let SearchArgs { query, limit } = parse_object(args)?;
        if query.trim().is_empty() {
            return Err(ModError::bad_args("query needs some text."));
        }
        if query.chars().count() > QUERY_MAX_CHARS {
            return Err(ModError::bad_args(format!(
                "query is at most {QUERY_MAX_CHARS} characters."
            )));
        }
        let limit = limit.unwrap_or(DEFAULT_LIMIT);
        if limit == 0 || limit > MAX_LIMIT {
            return Err(ModError::bad_args(format!("limit is 1 to {MAX_LIMIT}.")));
        }
        Ok(Self { text: query, limit })
    }
}

/// The fused candidate pool for `text`, holding back what a mod may not see
/// ([`retrieval::Filters::mod_visible`]). Under the DB guard, over an embedding the caller already
/// made; the rerank and the final cut happen after the guard is dropped.
pub(crate) fn visible_pool(
    conn: &Connection,
    text: &str,
    embedding: &[f32],
    k: usize,
    multilingual: bool,
) -> Result<Vec<RetrievedChunk>, Error> {
    retrieval::retrieve_fused(
        conn,
        &retrieval::RetrieveQuery {
            text,
            embedding,
            k,
            filters: retrieval::Filters {
                mod_visible: true,
                ..Default::default()
            },
            strategy: retrieval::Strategy::HybridRrf,
            multilingual,
        },
    )
}

/// The real search, for [`Host::search`] on the app: embed the query, fuse under the DB guard, drop
/// the guard, rerank (when the user has reranking on) and cut to `limit`. Blocking; the engine is
/// already known to be ready.
pub(super) fn search_store(
    state: &crate::AppState,
    text: &str,
    limit: usize,
) -> Result<Vec<RetrievedChunk>, ModError> {
    // The models and the reranking switch in one short guard, so neither the embed nor the rerank
    // (a sidecar call that can wait on a model download) holds the DB (AGENTS rule 4).
    let (gateway, rerank_on) = {
        let conn = state.conn().map_err(|_| ModError::locked())?;
        (
            state.gateway(&conn).map_err(ModError::internal)?,
            crate::db::reranking_enabled(&conn).map_err(ModError::internal)?,
        )
    };
    let embedding = gateway
        .embed_query(&[text.to_owned()])
        .map_err(ModError::internal)?
        .into_iter()
        .next()
        .ok_or_else(|| ModError::internal("the embedder returned no vector"))?;
    let reranker = rerank_on.then_some(&gateway as &dyn retrieval::Reranker);
    finish_search(
        state,
        text,
        &embedding,
        limit,
        gateway.embedder().multilingual,
        reranker,
    )
}

/// Everything after the embed: fuse the visible pool under a short DB guard, drop the guard, then
/// rerank (when there is a reranker) and cut to `limit`. [`search_store`] and the tests' host both
/// end here, so what the tests run is the production path and only the embed is scripted.
pub(super) fn finish_search(
    state: &crate::AppState,
    text: &str,
    embedding: &[f32],
    limit: usize,
    multilingual: bool,
    reranker: Option<&dyn retrieval::Reranker>,
) -> Result<Vec<RetrievedChunk>, ModError> {
    let pool = {
        let conn = state.conn().map_err(|_| ModError::locked())?;
        visible_pool(&conn, text, embedding, limit, multilingual).map_err(ModError::internal)?
    };
    retrieval::rerank_and_select(reranker, text, pool, limit)
        .map(|(chunks, _top)| chunks)
        .map_err(ModError::internal)
}

/// A hit as a caller with this `projection` gets it, field by field from named values: never the
/// chunk as it is, so a field added to a chunk can never leak by being added. `project` is the
/// document's home project's name (`None` for the unsorted), shown only with `projects.read`.
pub(crate) fn hit_dto(
    chunk: &RetrievedChunk,
    project: Option<&str>,
    projection: &Projection,
) -> Value {
    let snippet: String = chunk.content.chars().take(SNIPPET_MAX_CHARS).collect();
    let label = |text: &str| -> String { text.chars().take(LABEL_MAX_CHARS).collect() };
    Dto::new(*projection)
        .field(Needs::Nothing, "documentId", chunk.document_id)
        .field(Needs::Nothing, "title", label(&chunk.title))
        .field(
            Needs::Nothing,
            "heading",
            chunk.heading.as_deref().map(label),
        )
        .field(Needs::Nothing, "snippet", snippet)
        // A summary a model wrote about a file PM does not hold, not text of the file itself.
        .field(
            Needs::Nothing,
            "indexOnly",
            chunk.source_type.as_deref() == Some(crate::ingest::SOURCE_TYPE_INDEX_ONLY),
        )
        // A project's name is whatever its owner typed, so it is cut like any other label.
        .field(Needs::Projects, "project", project.map(label))
        .build()
}

/// The hits for `chunks` (already the host's answer to the query), at most `limit`. The index-only
/// placeholder is dropped here as well, as a second line behind the retrieval filter: it is a stand-in,
/// never an excerpt.
pub(crate) fn hits(
    conn: &Connection,
    chunks: &[RetrievedChunk],
    limit: usize,
    projection: &Projection,
) -> Result<Vec<Value>, ModError> {
    let kept: Vec<&RetrievedChunk> = chunks
        .iter()
        .filter(|c| c.content != crate::ingest::INDEX_ONLY_BODY_PLACEHOLDER)
        .take(limit)
        .collect();
    let projects = if projection.projects {
        home_projects(conn, kept.iter().map(|c| c.document_id))?
    } else {
        HashMap::new()
    };
    Ok(kept
        .into_iter()
        .map(|c| {
            hit_dto(
                c,
                projects.get(&c.document_id).map(String::as_str),
                projection,
            )
        })
        .collect())
}

/// The home project's name of each of these documents that has one.
fn home_projects(
    conn: &Connection,
    ids: impl Iterator<Item = i64>,
) -> Result<HashMap<i64, String>, ModError> {
    let wanted = serde_json::to_string(&ids.collect::<Vec<_>>()).map_err(ModError::internal)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, project FROM documents \
             WHERE id IN (SELECT value FROM json_each(?1)) AND TRIM(project) <> ''",
        )
        .map_err(ModError::internal)?;
    let rows = stmt
        .query_map(params![wanted], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(ModError::internal)?;
    let mut out = HashMap::new();
    for row in rows {
        let (id, project) = row.map_err(ModError::internal)?;
        out.insert(id, project.trim().to_owned());
    }
    Ok(out)
}

/// The whole search for `query`, shared by the frame's call and the MCP server's tool: the engine
/// check, the heavy step, and the hits as the caller's projection allows. Blocking.
///
/// `check` is the caller's own "is this call still allowed" step, run under the DB guard it is handed,
/// and answering with the projection the caller has NOW (a frame: [`recheck`] then its grants; an MCP
/// client: the request checks, then its scopes). It runs before the engine probe, again right before
/// the heavy step (no guard is held across the embed, the fuse or the rerank: they take it
/// themselves, briefly), and once more after it under the guard the hits are built with, so a grant
/// revoked while the embed ran is not spent on the answer.
pub(crate) fn answer<H: Host>(
    host: &H,
    query: &Query,
    check: impl Fn(&Connection) -> Result<Projection, ModError>,
) -> Result<Value, ModError> {
    {
        let conn = host.store().conn().map_err(|_| ModError::locked())?;
        check(&conn)?;
    }
    // The engine must already be set up: PM provisions nothing for a call from outside, and the
    // check comes before any retrieval.
    if !host.engine_ready() {
        return Err(ModError::engine_not_ready());
    }
    {
        let conn = host.store().conn().map_err(|_| ModError::locked())?;
        check(&conn)?;
    }
    #[cfg(test)]
    host.at(super::Point::Embedding);
    let chunks = host.search(&query.text, query.limit)?;
    let conn = host.store().conn().map_err(|_| ModError::locked())?;
    let projection = check(&conn)?;
    let found = hits(&conn, &chunks, query.limit, &projection)?;
    Ok(serde_json::json!({ "hits": found }))
}

/// `documents.search({query, limit?})` → `{hits: [{documentId, title, heading, snippet, indexOnly,
/// project?}]}`.
pub(super) async fn search<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let query = Query::parse(args)?;
    let value = {
        let (host, auth) = (host.clone(), auth.clone());
        within_call_timeout(blocking(move || {
            #[cfg(test)]
            host.at(super::Point::BeforeData);
            let value = answer(&host, &query, |conn| {
                let facts = recheck(&host, conn, &auth)?;
                Projection::of_grants(conn, &facts, host.engine_sealed())
            })?;
            still_current(&host, &auth)?;
            Ok(value)
        }))
        .await?
    };
    Ok(Reply::value(value))
}
