// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! How fast one connected app may ask, and the words it is told when it is too fast.
//!
//! The limiter is the mods' own ([`Calls`]: a bucket, a concurrency cap and the families' own
//! buckets), a second instance held by the MCP server and keyed by a made-up mod id, `mcp.c<id>`
//! (the app's row id), so one app's flood never spends another's allowance and nothing here touches
//! the mods' limiter or its types. The numbers are the mods' defaults: 50 calls a second, bursts of
//! 100, 16 in flight, and the embed family's own (one search at a time, six a minute) for `search`.
//!
//! An app is not a mod, so a refusal is told in the app's words: the mods' sentences ("this mod's
//! calls") would read wrong, and a fixed sentence also gives nothing about the limiter's state away.

use std::time::Instant;

use crate::mods::api::limits::{Admission, Calls, Family, FamilyAdmission};
use crate::mods::api::{Code, ModError};
use crate::mods::ids::ModId;

/// The one sentence a limiter refusal is told in.
pub const TOO_FAST: &str = "Too many requests from this app. Wait a moment and try again.";

/// What an admitted request holds until it is dropped: its slot in flight and, once its tool is
/// known, its family's.
#[derive(Debug)]
pub struct Held {
    _call: Admission,
    _family: Option<FamilyAdmission>,
}

/// The per-app limiter.
#[derive(Debug, Default)]
pub struct McpLimits {
    calls: Calls,
}

/// The made-up mod id an app's allowance is kept under. `mcp.c<id>` always parses.
fn key(client: i64) -> Result<ModId, ModError> {
    ModId::parse(&format!("mcp.c{client}")).map_err(ModError::internal)
}

/// Whatever the mods' limiter said, the app is told this.
fn too_fast(_: ModError) -> ModError {
    ModError::new(Code::RateLimited, TOO_FAST)
}

impl McpLimits {
    /// Admit one request of `client` under the general allowance alone, or refuse it in the app's own
    /// words. Every request is asked this FIRST, before any check that reads the store or the device's
    /// file, so a request PM will turn away anyway (an unknown tool, a listing) still costs the app a
    /// token and a slot, and an app that loops on one cannot hold the DB guard.
    pub fn admit_call(&self, client: i64) -> Result<Held, ModError> {
        self.admit_call_at(client, Instant::now())
    }

    /// [`admit_call`](Self::admit_call) at `now` (a test's clock).
    pub fn admit_call_at(&self, client: i64, now: Instant) -> Result<Held, ModError> {
        let id = key(client)?;
        let call = self.calls.admit(&id, now).map_err(too_fast)?;
        Ok(Held {
            _call: call,
            _family: None,
        })
    }

    /// `held` (an admitted request of `client`) also admitted in its tool's `family`. A refusal gives
    /// the request's slot back.
    pub fn in_family(&self, held: Held, client: i64, family: Family) -> Result<Held, ModError> {
        self.in_family_at(held, client, family, Instant::now())
    }

    /// [`in_family`](Self::in_family) at `now` (a test's clock).
    pub fn in_family_at(
        &self,
        mut held: Held,
        client: i64,
        family: Family,
        now: Instant,
    ) -> Result<Held, ModError> {
        let id = key(client)?;
        held._family = Some(
            self.calls
                .admit_family(&id, family, now)
                .map_err(too_fast)?,
        );
        Ok(held)
    }

    /// A request of `client` in `family` admitted in both steps at `now`.
    // Only the tests drive the clock.
    #[cfg(test)]
    pub fn admit_at(&self, client: i64, family: Family, now: Instant) -> Result<Held, ModError> {
        let held = self.admit_call_at(client, now)?;
        self.in_family_at(held, client, family, now)
    }
}
