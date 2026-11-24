// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The two ends of a question PM asks the person for a mod's call (PR C, K1; the registry itself is
//! `mods::confirm`): [`request`], which a handler ends in when its call needs the yes, and
//! [`complete`], which `mod_confirm` runs with the answer.
//!
//! **A handler asks** under the DB guard it already holds: it validates everything, then calls
//! [`request`] with the [`Action`] (ids only). Rust reads the target ([`Action::read`]), so the
//! question's text comes from PM's data and the mod's call contributes only which target; the facts
//! are hashed into the token, and the call is answered `MOD-2003` with `{token, summary}`.
//!
//! **The answer** ([`complete`]) is the dispatcher's own care taken again, because time has passed
//! and a person is slow: the main window only; the app not locked; the token taken (single use);
//! then, on Accept, the family's cap on calls in flight, [`recheck`] under the guard (steps 3-5 again:
//! the session, the mod, its permission), the second reads of a write call, the target read again and
//! held to the hash, the mod's gift quota for what the action will add, and only then
//! [`Action::perform`], which asks [`still_current`](super::still_current) once more right before it
//! commits. The action is the one stored with the token: nothing a frame or the page says reaches it.
//! K2 adds a variant of [`Action`] and a branch to each of the four methods below (a variant that
//! makes gifts says how many in `gifts`).

use std::time::Instant;

use rusqlite::Connection;
use serde_json::Value;

use crate::mods::confirm::{snapshot_hash, Action, Ask, Refused};
use crate::mods::ids::ModId;
use crate::mods::MAIN_LABEL;

use super::grants::ModFacts;
use super::permissions::Class;
use super::{blocking, gifts, recheck, second_read, Authorised, Code, Host, ModError, Reply};

/// What a question is about, as PM reads it now: the facts its text and its snapshot are made from,
/// and the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Target {
    /// Everything the question's words depend on, in one string: if any of it changes, the question
    /// no longer describes what would be done.
    pub facts: String,
    /// The question, in PM's words. Built from PM's data, never from the call's arguments.
    pub summary: String,
}

impl Action {
    /// The action's stable name (the snapshot is bound to it).
    pub(super) fn name(&self) -> &'static str {
        // `*self`, so that with no variant but a test's (a production build, until K2 adds the first)
        // the match is over an empty enum and still compiles.
        match *self {
            #[cfg(test)]
            Self::Probe { .. } => "probe",
            #[cfg(test)]
            Self::Gifting { .. } => "gifting",
        }
    }

    /// How many tasks or milestones the action will add to the user's lists as this mod's gifts (each
    /// is a ledger row). The answer checks the mod's quota for them under the guard, right before
    /// [`Action::perform`], because the quota may have filled while the question was open: a variant
    /// that makes gifts MUST say so here. (`perform` still writes the ledger rows with the gift.)
    pub(super) fn gifts(&self) -> u64 {
        match *self {
            #[cfg(test)]
            Self::Probe { .. } => 0,
            #[cfg(test)]
            Self::Gifting { adding, .. } => adding,
        }
    }

    /// Read the target for `mod_id` from the store: `NotFound` when it is gone (or, for a handler's
    /// own rule, not this mod's to ask about).
    #[cfg_attr(not(test), allow(unused_variables))]
    pub(super) fn read(&self, conn: &Connection, mod_id: &ModId) -> Result<Target, ModError> {
        match *self {
            #[cfg(test)]
            Self::Probe { ref key } | Self::Gifting { ref key, .. } => {
                let value: Option<String> = rusqlite::OptionalExtension::optional(conn.query_row(
                    "SELECT value FROM mod_kv WHERE mod_id = ?1 AND key = ?2",
                    rusqlite::params![mod_id.as_str(), key],
                    |r| r.get(0),
                ))
                .map_err(ModError::internal)?;
                let value = value.ok_or_else(|| ModError::new(Code::NotFound, "Nothing there."))?;
                Ok(Target {
                    facts: format!("{key}={value}"),
                    summary: format!("Let the test use \"{value}\"?"),
                })
            }
        }
    }

    /// Do it, with the guard (`conn`) held and the call just re-checked. Commits only after
    /// [`super::still_current`], like every writer.
    #[cfg_attr(not(test), allow(unused_variables))]
    pub(super) fn perform<H: Host>(
        &self,
        host: &H,
        conn: &Connection,
        auth: &Authorised,
        _facts: &ModFacts,
    ) -> Result<Value, ModError> {
        match *self {
            #[cfg(test)]
            Self::Probe { ref key } | Self::Gifting { ref key, .. } => {
                let tx = conn.unchecked_transaction().map_err(ModError::internal)?;
                tx.execute(
                    "UPDATE mod_kv SET value = '\"confirmed\"' WHERE mod_id = ?1 AND key = ?2",
                    rusqlite::params![auth.ticket.mod_id.as_str(), key],
                )
                .map_err(ModError::internal)?;
                host.at(super::Point::BeforeCommit);
                super::still_current(host, auth)?;
                tx.commit().map_err(ModError::internal)?;
                Ok(serde_json::json!({"done": true}))
            }
        }
    }
}

/// End a handler whose call needs the person's yes: read the target, hold the question, and return the
/// error to answer with (`MOD-2003` carrying `{token, summary}`; or what refused it: the target is
/// gone, the session ended, or the session is already waiting on another question, `MOD-3001`).
/// Call it under the DB guard, after every check the call has, and answer `Err(request(..))`.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn request<H: Host>(
    host: &H,
    conn: &Connection,
    auth: &Authorised,
    action: Action,
) -> ModError {
    let target = match action.read(conn, &auth.ticket.mod_id) {
        Ok(target) => target,
        Err(e) => return e,
    };
    let ask = Ask {
        row: auth.row,
        snapshot: snapshot_hash(action.name(), &target.facts),
        action,
    };
    match host.runtime().confirm_mint(&auth.ticket, ask) {
        Ok(token) => ModError::needs_confirmation(token, target.summary),
        Err(Refused::Busy) => ModError::new(
            Code::RateLimited,
            "PM is already asking about another call from this mod.",
        ),
        Err(Refused::Ended) => ModError::session_ended(),
        Err(Refused::Random) => ModError::internal("no random bytes for a confirm token"),
    }
}

/// `mod_confirm(token, accept)`: the person's answer. Never fails: every refusal is a
/// [`Reply::Error`] with its code. See the module doc for what is checked, and in what order.
pub async fn complete<H: Host>(host: &H, window: &str, token: &str, accept: bool) -> Reply {
    match answer(host, window, token, accept).await {
        Ok(value) => Reply::value(value).capped(),
        Err(error) => Reply::Error { error },
    }
}

async fn answer<H: Host>(
    host: &H,
    window: &str,
    token: &str,
    accept: bool,
) -> Result<Value, ModError> {
    // The main window only, as every call.
    if window != MAIN_LABEL {
        return Err(ModError::new(
            Code::NotGranted,
            "Mods run only in PM's main window.",
        ));
    }
    // The token is taken first, and is spent whatever follows. The store's epoch is read now, so a
    // token minted over another store is dead.
    let redeemed = host
        .runtime()
        .confirm_take(token, host.store().store_epoch())
        .ok_or_else(ModError::confirmation_gone)?;
    if !accept {
        return Err(ModError::declined());
    }
    let auth = Authorised {
        ticket: redeemed.ticket,
        row: redeemed.ask.row,
    };
    let (action, snapshot) = (redeemed.ask.action, redeemed.ask.snapshot);
    let host = host.clone();
    blocking(move || {
        // The lock is read before the guard is taken (its own read takes it), as the dispatcher does.
        if !matches!(host.app_locked(), Ok(false)) {
            return Err(ModError::locked());
        }
        // The family's cap on what is in flight, held while it runs (the question paid the bucket).
        let _family = host.calls().admit_family_answer(
            &auth.ticket.mod_id,
            auth.row.family,
            Instant::now(),
        )?;
        let conn = host.store().conn().map_err(|_| ModError::locked())?;
        // Steps 3-5 again, under the guard the action will use.
        let facts = recheck(&host, &conn, &auth)?;
        // A write to a target it names needs that target's reads still held: they may have been
        // revoked while the question was open. A write row with no rule is refused (fail closed).
        if auth.row.class == Class::Write {
            second_read::require(&host, &conn, &facts, &auth)?;
        }
        // What the question was about, as it is now: anything else is a different question.
        let target = action.read(&conn, &auth.ticket.mod_id)?;
        if snapshot_hash(action.name(), &target.facts) != snapshot {
            return Err(ModError::changed_while_asking());
        }
        // The mod's quota of gifts may have filled while the person decided. The guard is held, so
        // nothing can make a gift between this count and the action's own transaction.
        let adding = action.gifts();
        if adding > 0 {
            gifts::ensure_room(&conn, &auth.ticket.mod_id, adding)?;
        }
        action.perform(&host, &conn, &auth, &facts)
    })
    .await
}
