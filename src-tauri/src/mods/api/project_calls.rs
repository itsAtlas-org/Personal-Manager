// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `projects.list` (PR C, K2): the names of the user's projects, for a picker (the Pinboard's
//! timeline binding, a task's project). With `projects.read`.
//!
//! **Names and nothing else**: never an id, an alias, whether a project is archived, or a count of
//! anything in it. At most [`limits::MAX_PROJECTS_LISTED`] come back, in the order PM's own pickers
//! use (case-insensitive), and `truncated` says when there were more.
//!
//! The question itself is [`names`], which takes no mod: the MCP server asks it with its own cap.

use rusqlite::Connection;
use serde_json::{json, Value};

use super::calls::{self, internal};
use super::handlers::no_args;
use super::{limits, Authorised, Host, ModError, Reply};

/// The names of the user's projects, at most `max` of them in PM's picker order, and whether there
/// were more.
pub(crate) fn names(conn: &Connection, max: usize) -> Result<(Vec<String>, bool), ModError> {
    let mut names = crate::tags::project_names(conn).map_err(internal)?;
    let truncated = names.len() > max;
    names.truncate(max);
    Ok((names, truncated))
}

/// `projects.list()` → `{projects: [name], truncated}`.
pub(super) async fn list<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    no_args(args)?;
    let value = calls::read(host, auth, |cx| {
        let (names, truncated) = names(cx.conn, limits::MAX_PROJECTS_LISTED)?;
        Ok(json!({"projects": names, "truncated": truncated}))
    })
    .await?;
    Ok(Reply::value(value))
}
