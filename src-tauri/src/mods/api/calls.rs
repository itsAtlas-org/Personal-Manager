// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The two shapes every data call of PR C (K2) is built from, so no handler writes its own copy of
//! the care a call takes.
//!
//! - [`read`]: the blocking step under the DB guard, after [`recheck`]; then [`still_current`] again,
//!   because the call may have outlived its session while it waited for the pool.
//! - [`write`]: ONE transaction. [`recheck`] is its first statement, so what the mod may do is what the
//!   store says at the moment it writes; the handler's `work` runs inside it (every second read, every
//!   lookup, the gift's ledger row beside the gift itself); a test's hook sees the write made and not
//!   yet committed ([`Point::BeforeCommit`]); then [`still_current`] immediately before the commit, so a
//!   call in flight across an uninstall, a pause, a lock or a vault switch commits nothing. What the
//!   write leaves to do after the guard is gone (the task views' announcement, the briefing's nudge)
//!   rides back as an [`Aftermath`] and is settled by the host once the transaction has ended.
//!
//! **The DB mutex is not re-entrant.** `work` is handed the transaction and must not call
//! `state.conn()` (or anything that does: `iso_now(&AppState)`) while it runs.

use rusqlite::Connection;
use serde_json::Value;

use super::grants::ModFacts;
use super::{blocking, recheck, still_current, Authorised, Host, ModError};
use crate::error::Error;
use crate::milestones::ops::Aftermath;

/// What a data call's `work` is handed, all of it under the one DB guard.
pub(super) struct Ctx<'a, H: Host> {
    pub host: &'a H,
    pub auth: &'a Authorised,
    /// The transaction (a write) or the plain connection (a read).
    pub conn: &'a Connection,
    /// The mod as [`recheck`] just read it.
    pub facts: &'a ModFacts,
}

/// What a write answers with, and what it leaves for after the guard is dropped.
pub(super) struct Wrote<T> {
    pub value: T,
    pub aftermath: Aftermath,
}

/// A write that leaves nothing to settle.
pub(super) fn quiet<T>(value: T) -> Wrote<T> {
    Wrote {
        value,
        aftermath: Aftermath::default(),
    }
}

/// A write that tells the open task views to re-read (a task, or a milestone a task may follow).
pub(super) fn tasks_changed<T>(value: T) -> Wrote<T> {
    Wrote {
        value,
        aftermath: Aftermath {
            briefing: false,
            tasks: true,
        },
    }
}

/// Run `work` as a read, under the guard, then ask once more whether the call is current.
pub(super) async fn read<H: Host, T: Send + 'static>(
    host: &H,
    auth: &Authorised,
    work: impl FnOnce(&Ctx<'_, H>) -> Result<T, ModError> + Send + 'static,
) -> Result<T, ModError> {
    let out = {
        let (host, auth) = (host.clone(), auth.clone());
        blocking(move || {
            #[cfg(test)]
            host.at(super::Point::BeforeData);
            let conn = host.store().conn().map_err(|_| ModError::locked())?;
            let facts = recheck(&host, &conn, &auth)?;
            work(&Ctx {
                host: &host,
                auth: &auth,
                conn: &conn,
                facts: &facts,
            })
        })
        .await?
    };
    still_current(host, auth)?;
    Ok(out)
}

/// Run `work` as one write, in one transaction (see the module doc).
pub(super) async fn write<H: Host, T: Send + 'static>(
    host: &H,
    auth: &Authorised,
    work: impl FnOnce(&Ctx<'_, H>) -> Result<Wrote<T>, ModError> + Send + 'static,
) -> Result<T, ModError> {
    let wrote = {
        let (host, auth) = (host.clone(), auth.clone());
        blocking(move || {
            #[cfg(test)]
            host.at(super::Point::BeforeData);
            let mut conn = host.store().conn().map_err(|_| ModError::locked())?;
            let tx = conn.transaction().map_err(ModError::internal)?;
            let facts = recheck(&host, &tx, &auth)?;
            let wrote = work(&Ctx {
                host: &host,
                auth: &auth,
                conn: &tx,
                facts: &facts,
            })?;
            #[cfg(test)]
            host.at(super::Point::BeforeCommit);
            still_current(&host, &auth)?;
            tx.commit().map_err(ModError::internal)?;
            Ok(wrote)
        })
        .await?
    };
    // The guard and the transaction are gone: the views may now be told.
    host.settle(wrote.aftermath);
    Ok(wrote.value)
}

/// How a list core fits its page into an answer: the bytes of JSON it may use, and, for a caller that
/// must hold a hostile row to a size (the MCP server's), the most characters one text value may keep.
/// The frame's own calls cut nothing: the page budget alone bounds them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fit {
    pub budget: usize,
    pub text_cap: Option<usize>,
}

impl Fit {
    /// A mod's frame: the list budget, no text cut.
    pub(crate) const FRAME: Self = Self {
        budget: super::limits::LIST_PAGE_BUDGET_BYTES,
        text_cap: None,
    };
}

/// `value` with every text in it cut to `max` characters, at a character boundary. A row's text can
/// be as long as whoever wrote it (a feed, a shared-vault member) chose; cutting it before a page is
/// measured is what lets the page keep its bound whatever the rows hold.
pub(crate) fn clip_texts(mut value: Value, max: usize) -> Value {
    fn walk(value: &mut Value, max: usize) {
        match value {
            Value::String(s) => {
                if let Some((at, _)) = s.char_indices().nth(max) {
                    s.truncate(at);
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|v| walk(v, max)),
            Value::Object(map) => map.values_mut().for_each(|v| walk(v, max)),
            _ => {}
        }
    }
    walk(&mut value, max);
    value
}

/// A core error as the mod is told it: PM's own failure, with its detail kept out of the answer.
pub(super) fn internal(e: Error) -> ModError {
    ModError::internal(e)
}

/// A core error from a validation that a handler has already made safe to say (no mod text in its
/// words: the closed sets are checked first, so the core never echoes a value): a sentence of PM's
/// own, as a bad argument. Anything that is not a validation sentence is PM's failure.
pub(super) fn refusal(e: Error) -> ModError {
    match e {
        Error::Other(words) => ModError::bad_args(words),
        other => ModError::internal(other),
    }
}

/// The project a mod named, as PM spells it, or `NotFound`: a mod never mints one through a write
/// that names a project (`tasks::resolve_existing_project`; the one exception, a milestone, asks the
/// person first). The words never echo the name.
pub(super) fn existing_project(conn: &Connection, typed: &str) -> Result<String, ModError> {
    match crate::tasks::resolve_existing_project(conn, typed) {
        Ok(name) => Ok(name),
        Err(Error::Other(words)) if words == crate::tasks::NO_SUCH_PROJECT => {
            Err(no_such_project())
        }
        Err(other) => Err(internal(other)),
    }
}

pub(super) fn no_such_project() -> ModError {
    ModError::new(super::Code::NotFound, "There is no such project.")
}
