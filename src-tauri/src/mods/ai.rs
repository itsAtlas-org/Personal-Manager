// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a mod's AI call costs, and the ledger that keeps it under the mod's monthly cap (Mods spec
//! §1.6, plan X2). Pure decisions plus the two small DB steps (reserve, settle). No network, no
//! model, no UI, and nothing here is a dispatch row yet: `ai.complete` (X5) is the first caller.
//!
//! **Reserve, then settle.** Before a call PM books the worst case it could cost against the cap,
//! and afterwards swaps the booking for what it did cost. The cap reads only the ledger in `mods`
//! and `ai_reservations`, never `usage_log`, whose insert is best-effort (a failed insert skips a
//! row). `reserve` and `settle` are each ONE transaction on the one connection (`state.conn()`), so
//! N parallel calls cannot each see the same headroom and overshoot together; neither is ever held
//! across an `.await`.
//!
//! The rules, each with the reason it errs the way it does:
//! - **Price the plan, not the habit.** A route with a cloud arm is priced at the worst case over
//!   EVERY model the cloud arm might fall through to (the auto-switch list is up to 50 ids), because
//!   OpenRouter may serve any of them. `LocalThenCloud` falls back inside the call and
//!   `CloudForPower` moves a local request to cloud on battery, so a mod could otherwise make a
//!   "local" plan spend. Only a loopback or private local endpoint is free: a public https
//!   "local" endpoint can be a paid hosted service whose price PM cannot know (Q8c), so it fails
//!   closed like an unpriced model.
//! - **An unknown price fails closed.** A model with no cached price, a negative or non-finite one
//!   (router models carry negative catalogue prices), is refused rather than guessed.
//! - **A crash leaves the booking in place.** A reservation a process kill leaves behind counts
//!   toward the cap until the month ends (plan decision 4): deleting it after N hours would turn a
//!   crash into a way to unspend. Only the month roll, or the call itself, removes one.
//! - **The month never rolls backwards.** A stored month later than today's (a fast laptop clock,
//!   then corrected) never resets a cap, so a clock set BACK cannot clear spend. A month that is
//!   missing or unreadable is adopted as the current one with the spend kept.
//! - **A clock that runs FAST does clear the month early, and the ledger stays on that month.** PM
//!   trusts its own clock: a forward jump (a laptop clock hours or a mistyped year ahead) rolls the
//!   month at once, erasing the spend of the month it skipped, and the stored month then pins the
//!   ledger to the future one until real time reaches it, carrying what was spent meanwhile. The
//!   loss is bounded (one cap of spend, or a mod blocked until the date arrives); telling a real
//!   month change from a skewed one needs a time source PM does not have, so this is the stated
//!   limit of the cap, not a promise that a wrong clock can never clear spend.
//! - **Settling never under-counts.** A local answer costs 0; else the cost the provider reported;
//!   else tokens times the price of the model that served; else the whole booking counts as spent.
//!   A call that may have reached the wire and ended without an answer (a timeout, a dropped
//!   future) is settled the last way, never released.
//! - **A settle after the roll still charges.** The roll deletes the mod's bookings, so a call that
//!   straddles midnight on the last day settles against the new month: slightly early, never lost.
//!
//! Money is REAL dollars, like every other money column. The cap check is `settled + reserved + new
//! <= cap` in f64, so rounding can only refuse a call that exactly fits, never admit one that does
//! not; any non-finite number refuses.

// No caller until `ai.complete` (X5) takes the dispatch row; X5 deletes this line, so a function it
// leaves unused is a warning again.
#![allow(dead_code)]

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::llm_gateway::{CallMeta, Provider, RoutePlan};
use crate::local_slot::EndpointClass;
use crate::openrouter::{Completion, Usage};

/// The `usage_log.kind` of a mod's call. Not a new kind: I-11's one writer and the CHECK set stay as
/// they are, a mod's row is told apart by `mod_id`.
pub const USAGE_KIND: &str = "background";

/// USD per token, as `model_pricing` keeps them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Price {
    pub prompt: f64,
    pub completion: f64,
}

impl Price {
    /// A price only when both rates are finite and not negative. A negative rate is how the
    /// catalogue marks a router model ("priced at whatever it routes to"), which can't be reserved.
    pub fn new(prompt: f64, completion: f64) -> Option<Self> {
        let ok = |p: f64| p.is_finite() && p >= 0.0;
        (ok(prompt) && ok(completion)).then_some(Self { prompt, completion })
    }
}

/// The cached price of `model`, `None` when it is not cached, a rate is missing, or a rate cannot
/// be used (see [`Price::new`]).
pub fn model_price(conn: &Connection, model: &str) -> Option<Price> {
    let row: Option<(Option<f64>, Option<f64>)> = conn
        .query_row(
            "SELECT prompt_price, completion_price FROM model_pricing WHERE model = ?1",
            params![model],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .ok()
        .flatten();
    let (prompt, completion) = row?;
    Price::new(prompt?, completion?)
}

// --- the route's cost shape -------------------------------------------------------------------

/// What PM can say about a route's cost before it runs.
#[derive(Clone, Debug, PartialEq)]
pub enum RouteCost {
    /// A local model on this machine or the user's own network. Costs nothing, takes no booking and
    /// never asks for a cap.
    Free,
    /// A route with a cloud arm: priced at the worst case over these models, and asks for a cap
    /// even when every one of them prices at 0.
    Cloud(Vec<String>),
    /// A route whose cost PM cannot know (a public "local" endpoint, or no arm at all). Refused.
    Unpriced,
}

impl RouteCost {
    /// The cost shape from the arms of a route. `cloud_models` is the cloud arm's whole model list,
    /// if the route has a cloud arm; `local` is the class of the local arm's resolved address, if
    /// it has a local arm. Pure, so the Q8c rule is tested without a route or a resolver.
    pub fn from_arms(cloud_models: Option<&[String]>, local: Option<EndpointClass>) -> Self {
        // A public local endpoint is unpriceable whatever else the plan holds: its leg may run first.
        if local == Some(EndpointClass::PublicRemote) {
            return RouteCost::Unpriced;
        }
        match cloud_models {
            Some([]) => RouteCost::Unpriced,
            Some(models) => RouteCost::Cloud(models.to_vec()),
            None => match local {
                Some(_) => RouteCost::Free,
                None => RouteCost::Unpriced,
            },
        }
    }

    /// The cost shape of a resolved plan. `local_class` is the class of the plan's local arm (the
    /// caller resolves the address, off the DB lock); a plan with a local arm and no class counts
    /// as public, so a caller that forgets it fails closed.
    pub fn of(plan: &RoutePlan, local_class: Option<EndpointClass>) -> Self {
        let public = Some(EndpointClass::PublicRemote);
        match plan {
            RoutePlan::Cloud(arm) | RoutePlan::CloudForPower { cloud: arm, .. } => {
                Self::from_arms(Some(&arm.models), None)
            }
            RoutePlan::LocalOnly(_) => Self::from_arms(None, local_class.or(public)),
            RoutePlan::LocalThenCloud { cloud, .. } => {
                Self::from_arms(Some(&cloud.models), local_class.or(public))
            }
        }
    }

    /// Whether the cap and the money prompt apply.
    pub fn is_cloud(&self) -> bool {
        matches!(self, RouteCost::Cloud(_))
    }
}

/// No price could be fixed for the plan: MOD-5005 at the handler.
#[derive(Debug, PartialEq, Eq)]
pub struct PriceUnknown;

/// The most the call could cost: for every model in the plan's list, `prompt_bytes x prompt price +
/// max_tokens x completion price`, and the largest of them. Bytes bound the prompt's tokens from
/// above (a token is at least one byte), so no tokenizer is needed; over-reserving fails safe.
/// `price_of` is asked for every model, so one unpriced model anywhere in the list refuses the call
/// (the fallback could land on it).
pub fn worst_case_usd(
    cost: &RouteCost,
    prompt_bytes: u64,
    max_tokens: u32,
    price_of: impl Fn(&str) -> Option<Price>,
) -> Result<f64, PriceUnknown> {
    let models = match cost {
        RouteCost::Free => return Ok(0.0),
        RouteCost::Unpriced => return Err(PriceUnknown),
        RouteCost::Cloud(models) => models,
    };
    let mut worst = 0.0_f64;
    for model in models {
        let price = price_of(model).ok_or(PriceUnknown)?;
        let usd = prompt_bytes as f64 * price.prompt + f64::from(max_tokens) * price.completion;
        if !usd.is_finite() {
            return Err(PriceUnknown);
        }
        worst = worst.max(usd);
    }
    Ok(worst)
}

// --- the month ----------------------------------------------------------------------------------

/// The user's civil month in `zone` (D-16), `YYYY-MM`.
pub fn civil_month(zone: Tz, now: DateTime<Utc>) -> String {
    crate::clock::today_in_at(zone, now)
        .format("%Y-%m")
        .to_string()
}

/// `YYYY-MM` with a month 01 to 12 and nothing else, so string order is calendar order.
fn is_month(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 7
        && b[4] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && matches!(
            &s[5..],
            "01" | "02" | "03" | "04" | "05" | "06" | "07" | "08" | "09" | "10" | "11" | "12"
        )
}

/// What a mod's stored month asks of the ledger at `current`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonthStep {
    /// The stored month is this one, or a LATER one (the clock went back): nothing changes.
    Keep,
    /// No month, or one that can't be read: it becomes the current month and the spend stays.
    Adopt,
    /// An earlier month: spend and bookings start again.
    Reset,
}

pub fn month_step(stored: Option<&str>, current: &str) -> MonthStep {
    // PM's own month is always well formed; if it is not, nothing may be cleared on its word.
    if !is_month(current) {
        return MonthStep::Keep;
    }
    match stored {
        None => MonthStep::Adopt,
        Some(s) if !is_month(s) => MonthStep::Adopt,
        Some(s) if s < current => MonthStep::Reset,
        Some(_) => MonthStep::Keep,
    }
}

// --- the cap ------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// Not a cloud route: nothing is booked and the cap does not apply.
    Free,
    /// A cloud route and no cap yet: the handler asks (MOD-2002, permission `ai`).
    NeedsCap,
    /// Booking it would pass the cap: MOD-3004.
    CapReached,
    Allow,
}

/// The cap check. Cap 0 is valid: a cloud route that costs 0 passes, any cost refuses. A cap
/// lowered below the spend refuses every priced call, and raising it is immediate (the next call
/// reads the new number).
pub fn gate(cloud: bool, cap: Option<f64>, settled: f64, reserved: f64, new: f64) -> Gate {
    if !cloud {
        return Gate::Free;
    }
    let Some(cap) = cap else {
        return Gate::NeedsCap;
    };
    let sane = |x: f64| x.is_finite() && x >= 0.0;
    if !(sane(cap) && sane(settled) && sane(reserved) && sane(new)) {
        return Gate::CapReached;
    }
    if settled + reserved + new <= cap {
        Gate::Allow
    } else {
        Gate::CapReached
    }
}

// --- reserve ------------------------------------------------------------------------------------

/// A booking against a mod's cap, or the stand-in for a free call. Not `Clone`, and `settle` takes
/// it by value: a booking is settled once.
#[derive(Debug)]
pub struct Reservation {
    /// `AppState::store_epoch` of the store the booking was made in. Booking ids are per-database
    /// counters and mod ids repeat across vaults, so only this ties a booking to its own store: a
    /// settle on any other store deletes and charges nothing.
    epoch: u64,
    /// `None` for a free call, which booked nothing.
    id: Option<i64>,
    mod_id: String,
    usd: f64,
}

impl Reservation {
    pub fn usd(&self) -> f64 {
        self.usd
    }
    pub fn is_booked(&self) -> bool {
        self.id.is_some()
    }
}

#[derive(Debug)]
pub enum ReserveError {
    /// No such mod (uninstalled since the call began): nothing booked.
    ModGone,
    /// Cloud route, no cap set.
    NeedsCap,
    CapReached,
    /// The amount to book was not a finite, non-negative number.
    BadAmount,
    Db(rusqlite::Error),
}

impl From<rusqlite::Error> for ReserveError {
    fn from(e: rusqlite::Error) -> Self {
        ReserveError::Db(e)
    }
}

/// Book `usd` against `mod_id`'s cap in its own IMMEDIATE transaction. A free call (`cloud` false)
/// books nothing and touches nothing. `epoch` is `AppState::store_epoch` read under the guard of
/// the connection passed (so it is the epoch of THIS store): the booking is settled only there.
pub fn reserve(
    conn: &mut Connection,
    epoch: u64,
    mod_id: &str,
    month: &str,
    usd: f64,
    cloud: bool,
) -> Result<Reservation, ReserveError> {
    if !cloud {
        return Ok(free(mod_id, epoch));
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let outcome = reserve_in(&tx, epoch, mod_id, month, usd, cloud);
    // Commit on every outcome but a database error: a refused call still keeps the month roll it
    // made, which must not be undone by the refusal.
    match outcome {
        Err(ReserveError::Db(e)) => Err(ReserveError::Db(e)),
        other => {
            tx.commit()?;
            other
        }
    }
}

fn free(mod_id: &str, epoch: u64) -> Reservation {
    Reservation {
        epoch,
        id: None,
        mod_id: mod_id.to_string(),
        usd: 0.0,
    }
}

/// [`reserve`] without opening a transaction, for a caller that already holds one (the handler
/// checks the mod, then books, in one). The caller owns the atomicity: a write transaction on the
/// one connection.
pub fn reserve_in(
    conn: &Connection,
    epoch: u64,
    mod_id: &str,
    month: &str,
    usd: f64,
    cloud: bool,
) -> Result<Reservation, ReserveError> {
    if !cloud {
        return Ok(free(mod_id, epoch));
    }
    if !(usd.is_finite() && usd >= 0.0) {
        return Err(ReserveError::BadAmount);
    }
    let row: Option<(Option<f64>, Option<String>, f64)> = conn
        .query_row(
            "SELECT ai_cap_usd, ai_month, ai_spent_usd FROM mods WHERE id = ?1",
            params![mod_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((cap, stored_month, mut settled)) = row else {
        return Err(ReserveError::ModGone);
    };
    match month_step(stored_month.as_deref(), month) {
        MonthStep::Keep => {}
        MonthStep::Adopt => {
            conn.execute(
                "UPDATE mods SET ai_month = ?2 WHERE id = ?1",
                params![mod_id, month],
            )?;
        }
        MonthStep::Reset => {
            conn.execute(
                "DELETE FROM ai_reservations WHERE mod_id = ?1",
                params![mod_id],
            )?;
            conn.execute(
                "UPDATE mods SET ai_spent_usd = 0, ai_month = ?2 WHERE id = ?1",
                params![mod_id, month],
            )?;
            settled = 0.0;
        }
    }
    let reserved = reserved_usd(conn, mod_id)?;
    match gate(true, cap, settled, reserved, usd) {
        Gate::NeedsCap => Err(ReserveError::NeedsCap),
        Gate::CapReached => Err(ReserveError::CapReached),
        Gate::Free | Gate::Allow => {
            conn.execute(
                "INSERT INTO ai_reservations(mod_id, reserved_usd) VALUES (?1, ?2)",
                params![mod_id, usd],
            )?;
            Ok(Reservation {
                epoch,
                id: Some(conn.last_insert_rowid()),
                mod_id: mod_id.to_string(),
                usd,
            })
        }
    }
}

// --- settle -------------------------------------------------------------------------------------

/// Which rule fixed what a call cost, for the tests and for a reader of the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettleBasis {
    /// A local model answered.
    Local,
    /// The cost the provider reported.
    Reported,
    /// Reported tokens times the served model's price.
    Estimated,
    /// Nothing to measure with: the whole booking counts as spent.
    Reservation,
}

/// What a finished call cost, never under what it could have. `served` is the price of the model
/// that answered (`None` when it is not known or not priced).
pub fn settle_amount(
    provider: Provider,
    usage: &Usage,
    served: Option<Price>,
    reserved: f64,
) -> (f64, SettleBasis) {
    if provider == Provider::Local {
        return (0.0, SettleBasis::Local);
    }
    let usable = |x: f64| x.is_finite() && x >= 0.0;
    if let Some(cost) = usage.cost.filter(|c| usable(*c)) {
        return (cost, SettleBasis::Reported);
    }
    if let (Some(prompt), Some(completion), Some(price)) =
        (usage.prompt_tokens, usage.completion_tokens, served)
    {
        if prompt >= 0 && completion >= 0 {
            let usd = prompt as f64 * price.prompt + completion as f64 * price.completion;
            if usable(usd) {
                return (usd, SettleBasis::Estimated);
            }
        }
    }
    (reserved, SettleBasis::Reservation)
}

/// What a settle did.
#[derive(Debug, PartialEq)]
pub struct Settled {
    pub charged_usd: f64,
    /// False when the mod was uninstalled during the call: no row took the charge (the history
    /// survives in `usage_log`).
    pub mod_present: bool,
    /// True when the booking was made in another store (a vault switch, a lock, a restore mid-call):
    /// nothing was deleted or charged here, because the ids in this store are not that booking's.
    /// The booking stays behind in its own store and counts there, so the cap errs high, never low.
    pub foreign_store: bool,
}

impl Settled {
    fn foreign() -> Self {
        Self {
            charged_usd: 0.0,
            mod_present: false,
            foreign_store: true,
        }
    }
}

/// Swap the booking for `usd` in one IMMEDIATE transaction. A non-finite or negative `usd` counts
/// the whole booking instead. The booking row may be gone (the month rolled, the mod went); the
/// charge is still added to the mod's row if it exists. `epoch` is `AppState::store_epoch` read
/// under the guard of `conn`; a booking from another store settles nothing ([`Settled::foreign_store`]).
pub fn settle(
    conn: &mut Connection,
    epoch: u64,
    res: Reservation,
    usd: f64,
) -> rusqlite::Result<Settled> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let out = settle_in(&tx, epoch, res, usd)?;
    tx.commit()?;
    Ok(out)
}

/// [`settle`] inside a transaction the caller holds.
pub fn settle_in(
    conn: &Connection,
    epoch: u64,
    res: Reservation,
    usd: f64,
) -> rusqlite::Result<Settled> {
    if res.epoch != epoch {
        return Ok(Settled::foreign());
    }
    let charge = if usd.is_finite() && usd >= 0.0 {
        usd
    } else {
        res.usd
    };
    if let Some(id) = res.id {
        conn.execute(
            "DELETE FROM ai_reservations WHERE id = ?1 AND mod_id = ?2",
            params![id, res.mod_id],
        )?;
    }
    // A free call booked nothing and costs nothing, so it never writes the mod's row.
    if res.id.is_none() && charge == 0.0 {
        return Ok(Settled {
            charged_usd: 0.0,
            mod_present: true,
            foreign_store: false,
        });
    }
    let rows = conn.execute(
        "UPDATE mods SET ai_spent_usd = ai_spent_usd + ?2 WHERE id = ?1",
        params![res.mod_id, charge],
    )?;
    Ok(Settled {
        charged_usd: charge,
        mod_present: rows == 1,
        foreign_store: false,
    })
}

/// A call that may have reached the wire and ended without an answer we can measure (a timeout, a
/// dropped future, a failure after the request left): the booking counts as spent.
pub fn settle_unknown(
    conn: &mut Connection,
    epoch: u64,
    res: Reservation,
) -> rusqlite::Result<Settled> {
    let usd = res.usd;
    settle(conn, epoch, res, usd)
}

/// A call that provably never reached the wire (refused before any request was made): the booking
/// is deleted and nothing is spent.
pub fn release(conn: &mut Connection, epoch: u64, res: Reservation) -> rusqlite::Result<()> {
    settle(conn, epoch, res, 0.0).map(|_| ())
}

/// A finished call: settle by the rules above, and write the call's `usage_log` row with the mod's
/// id, whether or not the settle worked (an uninstalled mod's spend is still history; a failed
/// settle leaves its booking, which counts, so the cap errs high). `model` is the served model, or
/// the plan's primary when the server did not say. One call under the one connection guard. A
/// booking from another store writes no row here: it would put another vault's call in this one's
/// history.
pub fn settle_and_log(
    conn: &mut Connection,
    epoch: u64,
    res: Reservation,
    completion: &Completion,
    meta: &CallMeta,
    model: Option<&str>,
) -> rusqlite::Result<Settled> {
    if res.epoch != epoch {
        return Ok(Settled::foreign());
    }
    let served = completion
        .model
        .as_deref()
        .and_then(|m| model_price(conn, m));
    let (usd, _basis) = settle_amount(meta.provider, &completion.usage, served, res.usd);
    let mod_id = res.mod_id.clone();
    let settled = settle(conn, epoch, res, usd);
    crate::commands::log_usage_as(
        conn,
        USAGE_KIND,
        model,
        &completion.usage,
        meta,
        Some(&mod_id),
    );
    settled
}

/// Every booking's total for a mod: what the cap counts beside the settled spend.
pub fn reserved_usd(conn: &Connection, mod_id: &str) -> rusqlite::Result<f64> {
    conn.query_row(
        "SELECT COALESCE(SUM(reserved_usd), 0) FROM ai_reservations WHERE mod_id = ?1",
        params![mod_id],
        |r| r.get(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_gateway::{CloudArm, LocalArm};
    use crate::mods::api::test_support::{insert_mod, store, ModRow, DB_KEY};
    use crate::secret::Secret;
    use chrono::TimeZone;
    use std::sync::{Arc, Barrier, Mutex};

    const NOV: &str = "2026-11";
    const DEC: &str = "2026-12";

    fn price(prompt: f64, completion: f64) -> Price {
        Price { prompt, completion }
    }

    fn models(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    // --- prices and the route's shape -----------------------------------------------------------

    /// A rate that is negative (a router model's catalogue marker), missing or not a number is no
    /// price, so it can never be reserved against or settled with.
    #[test]
    fn a_price_needs_two_finite_non_negative_rates() {
        assert_eq!(Price::new(1e-6, 2e-6), Some(price(1e-6, 2e-6)));
        assert_eq!(Price::new(0.0, 0.0), Some(price(0.0, 0.0)));
        for bad in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(Price::new(bad, 1e-6), None, "{bad}");
            assert_eq!(Price::new(1e-6, bad), None, "{bad}");
        }
    }

    #[test]
    fn model_price_reads_the_cache_and_refuses_what_it_cannot_use() {
        let (_dir, conn) = store();
        let put = |model: &str, p: Option<f64>, c: Option<f64>| {
            conn.execute(
                "INSERT INTO model_pricing(model, prompt_price, completion_price, fetched_at) \
                 VALUES (?1, ?2, ?3, '2026-11-17T00:00:00.000Z')",
                params![model, p, c],
            )
            .unwrap();
        };
        put("a/ok", Some(1e-6), Some(2e-6));
        put("a/free", Some(0.0), Some(0.0));
        put("a/router", Some(-1.0), Some(-1.0));
        put("a/half", Some(1e-6), None);
        assert_eq!(model_price(&conn, "a/ok"), Some(price(1e-6, 2e-6)));
        assert_eq!(model_price(&conn, "a/free"), Some(price(0.0, 0.0)));
        assert_eq!(model_price(&conn, "a/router"), None);
        assert_eq!(model_price(&conn, "a/half"), None);
        assert_eq!(model_price(&conn, "a/absent"), None);
    }

    /// The Q8c rule and the plan rule in one table: only a loopback or private local arm is free; a
    /// cloud arm anywhere makes the route a priced one; a public local arm is unpriceable even
    /// with a cloud arm beside it (its leg runs first).
    #[test]
    fn the_route_is_priced_by_its_arms() {
        use EndpointClass::{Loopback, PrivateRemote, PublicRemote};
        let cloud = models(&["a/one", "a/two"]);
        let from = RouteCost::from_arms;
        assert_eq!(from(None, Some(Loopback)), RouteCost::Free);
        assert_eq!(from(None, Some(PrivateRemote)), RouteCost::Free);
        assert_eq!(from(None, Some(PublicRemote)), RouteCost::Unpriced);
        assert_eq!(from(Some(&cloud), None), RouteCost::Cloud(cloud.clone()));
        assert_eq!(
            from(Some(&cloud), Some(Loopback)),
            RouteCost::Cloud(cloud.clone())
        );
        assert_eq!(
            from(Some(&cloud), Some(PrivateRemote)),
            RouteCost::Cloud(cloud.clone())
        );
        assert_eq!(from(Some(&cloud), Some(PublicRemote)), RouteCost::Unpriced);
        assert_eq!(from(Some(&[]), None), RouteCost::Unpriced);
        assert_eq!(from(None, None), RouteCost::Unpriced);
        assert!(from(Some(&cloud), None).is_cloud());
        assert!(!RouteCost::Free.is_cloud());
        assert!(!RouteCost::Unpriced.is_cloud());
    }

    fn cloud_arm(ids: &[&str]) -> CloudArm {
        CloudArm {
            key: Secret::from("k".to_string()),
            models: models(ids),
        }
    }

    fn local_arm() -> LocalArm {
        LocalArm {
            base_url: "http://127.0.0.1:11434".into(),
            model: "llama".into(),
            token: None,
        }
    }

    /// Every plan shape: LocalThenCloud and CloudForPower are priced at the cloud arm, LocalOnly is
    /// free, and a plan with a local arm whose address class was never given counts as public.
    #[test]
    fn every_plan_shape_is_priced_at_its_worst_arm() {
        use EndpointClass::{Loopback, PublicRemote};
        let cloud = RouteCost::Cloud(models(&["a/one", "a/two"]));
        let plan = RoutePlan::Cloud(cloud_arm(&["a/one", "a/two"]));
        assert_eq!(RouteCost::of(&plan, None), cloud);
        let power = RoutePlan::CloudForPower {
            cloud: cloud_arm(&["a/one", "a/two"]),
            displaced_local_model: "llama".into(),
        };
        assert_eq!(RouteCost::of(&power, None), cloud);
        let then = RoutePlan::LocalThenCloud {
            local: local_arm(),
            cloud: cloud_arm(&["a/one", "a/two"]),
        };
        assert_eq!(RouteCost::of(&then, Some(Loopback)), cloud);
        assert_eq!(
            RouteCost::of(&then, Some(PublicRemote)),
            RouteCost::Unpriced
        );
        assert_eq!(RouteCost::of(&then, None), RouteCost::Unpriced);
        let only = RoutePlan::LocalOnly(local_arm());
        assert_eq!(RouteCost::of(&only, Some(Loopback)), RouteCost::Free);
        assert_eq!(
            RouteCost::of(&only, Some(PublicRemote)),
            RouteCost::Unpriced
        );
        assert_eq!(RouteCost::of(&only, None), RouteCost::Unpriced);
    }

    #[test]
    fn the_worst_case_is_the_dearest_model_in_the_whole_list() {
        let prices = |m: &str| match m {
            "a/cheap" => Some(price(1e-6, 2e-6)),
            // Dear on completion, cheap on prompt: the max is taken over the SUM per model.
            "a/dear" => Some(price(0.5e-6, 10e-6)),
            "a/last" => Some(price(3e-6, 1e-6)),
            _ => None,
        };
        let cost = RouteCost::Cloud(models(&["a/cheap", "a/dear", "a/last"]));
        // 1000 bytes, 100 tokens: cheap 0.0012, dear 0.0015, last 0.0031.
        let got = worst_case_usd(&cost, 1000, 100, prices).unwrap();
        assert!((got - 0.0031).abs() < 1e-12, "{got}");
        // The dearest model sits in the MIDDLE for a completion-heavy call.
        let got = worst_case_usd(&cost, 10, 1000, prices).unwrap();
        assert!(
            (got - (10.0 * 0.5e-6 + 1000.0 * 10e-6)).abs() < 1e-12,
            "{got}"
        );
        // First, middle and last positions all count: reorder the list.
        let reordered = RouteCost::Cloud(models(&["a/last", "a/dear", "a/cheap"]));
        assert_eq!(
            worst_case_usd(&reordered, 10, 1000, prices),
            worst_case_usd(&cost, 10, 1000, prices)
        );
    }

    #[test]
    fn one_unpriced_model_anywhere_in_the_list_refuses_the_call() {
        let prices = |m: &str| (m != "a/unknown").then(|| price(1e-6, 1e-6));
        for list in [
            ["a/unknown", "a/ok", "a/ok2"],
            ["a/ok", "a/unknown", "a/ok2"],
            ["a/ok", "a/ok2", "a/unknown"],
        ] {
            let cost = RouteCost::Cloud(models(&list));
            assert_eq!(worst_case_usd(&cost, 10, 10, prices), Err(PriceUnknown));
        }
        assert_eq!(
            worst_case_usd(&RouteCost::Unpriced, 10, 10, prices),
            Err(PriceUnknown)
        );
    }

    #[test]
    fn a_free_route_costs_nothing_and_a_free_cloud_model_costs_nothing_but_still_asks() {
        let never = |_: &str| -> Option<Price> { panic!("a free route asks no price") };
        assert_eq!(
            worst_case_usd(&RouteCost::Free, 1 << 20, 4096, never),
            Ok(0.0)
        );
        let free_cloud = RouteCost::Cloud(models(&["a/free"]));
        let got = worst_case_usd(&free_cloud, 1 << 20, 4096, |_| Some(price(0.0, 0.0)));
        assert_eq!(got, Ok(0.0));
        assert!(free_cloud.is_cloud());
    }

    /// A number that leaves the f64 range is no number: it refuses rather than reserving infinity
    /// (which the CHECK would reject, or `0 * inf` would turn into a NaN that passes a comparison).
    #[test]
    fn a_worst_case_that_overflows_is_refused() {
        let cost = RouteCost::Cloud(models(&["a/huge"]));
        let huge = |_: &str| Some(price(1e308, 1e308));
        assert_eq!(
            worst_case_usd(&cost, u64::MAX, u32::MAX, huge),
            Err(PriceUnknown)
        );
    }

    // --- the month ------------------------------------------------------------------------------

    #[test]
    fn the_month_steps_by_calendar_order() {
        use MonthStep::{Adopt, Keep, Reset};
        let step = month_step;
        assert_eq!(step(Some("2026-11"), "2026-11"), Keep);
        assert_eq!(step(Some("2026-10"), "2026-11"), Reset);
        assert_eq!(step(Some("2025-12"), "2026-01"), Reset);
        // A later stored month never resets: a clock that ran fast cannot clear spend.
        assert_eq!(step(Some("2026-12"), "2026-11"), Keep);
        assert_eq!(step(Some("2027-01"), "2026-12"), Keep);
        assert_eq!(step(Some("9999-12"), "2026-11"), Keep);
        // No month yet, or one nobody could have written: adopted, spend kept.
        assert_eq!(step(None, "2026-11"), Adopt);
        for junk in [
            "",
            "2026",
            "2026-13",
            "2026-00",
            "26-11",
            "2026/11",
            "2026-1",
            "abcd-ef",
            "2026-11-01",
        ] {
            assert_eq!(step(Some(junk), "2026-11"), Adopt, "{junk:?}");
        }
        // PM's own month is never junk; if it were, nothing is cleared on its word.
        assert_eq!(step(Some("2026-10"), "garbage"), Keep);
        assert_eq!(step(None, "garbage"), Keep);
    }

    /// The month is the user's civil month in their zone (D-16), so the boundary is their midnight.
    #[test]
    fn the_civil_month_follows_the_users_zone() {
        let at = chrono::Utc
            .with_ymd_and_hms(2026, 11, 30, 23, 30, 0)
            .unwrap();
        assert_eq!(civil_month(chrono_tz::UTC, at), "2026-11");
        assert_eq!(civil_month(chrono_tz::Pacific::Auckland, at), "2026-12");
        assert_eq!(civil_month(chrono_tz::America::Los_Angeles, at), "2026-11");
        let jan = chrono::Utc
            .with_ymd_and_hms(2026, 12, 31, 20, 0, 0)
            .unwrap();
        assert_eq!(civil_month(chrono_tz::Asia::Tokyo, jan), "2027-01");
    }

    // --- the cap --------------------------------------------------------------------------------

    #[test]
    fn the_gate_table() {
        use Gate::{Allow, CapReached, Free, NeedsCap};
        // Not cloud: free, whatever the cap says, even a cap lowered below the spend.
        assert_eq!(gate(false, None, 0.0, 0.0, 0.0), Free);
        assert_eq!(gate(false, Some(0.0), 5.0, 5.0, 0.0), Free);
        // Cloud without a cap asks.
        assert_eq!(gate(true, None, 0.0, 0.0, 0.5), NeedsCap);
        // Fits, exactly fits, passes by a hair.
        assert_eq!(gate(true, Some(1.0), 0.25, 0.25, 0.25), Allow);
        assert_eq!(gate(true, Some(1.0), 0.5, 0.25, 0.25), Allow);
        assert_eq!(gate(true, Some(1.0), 0.5, 0.25, 0.250001), CapReached);
        // Reservations count beside settled spend.
        assert_eq!(gate(true, Some(1.0), 0.0, 0.9, 0.2), CapReached);
        assert_eq!(gate(true, Some(1.0), 0.9, 0.0, 0.2), CapReached);
        // Cap 0: a cloud route that costs 0 passes, any cost refuses.
        assert_eq!(gate(true, Some(0.0), 0.0, 0.0, 0.0), Allow);
        assert_eq!(gate(true, Some(0.0), 0.0, 0.0, 1e-9), CapReached);
        // Lowered below what is spent: even a free model's cloud route (cost 0) is refused,
        // because spent > cap.
        assert_eq!(gate(true, Some(1.0), 2.0, 0.0, 0.0), CapReached);
        // Anything that is not a sane number refuses.
        for bad in [f64::NAN, f64::INFINITY, -1.0] {
            assert_eq!(
                gate(true, Some(bad), 0.0, 0.0, 0.0),
                CapReached,
                "cap {bad}"
            );
            assert_eq!(
                gate(true, Some(1.0), bad, 0.0, 0.0),
                CapReached,
                "settled {bad}"
            );
            assert_eq!(
                gate(true, Some(1.0), 0.0, bad, 0.0),
                CapReached,
                "reserved {bad}"
            );
            assert_eq!(
                gate(true, Some(1.0), 0.0, 0.0, bad),
                CapReached,
                "new {bad}"
            );
        }
    }

    // --- settling -------------------------------------------------------------------------------

    fn usage(prompt: Option<i64>, completion: Option<i64>, cost: Option<f64>) -> Usage {
        Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            cost,
        }
    }

    #[test]
    fn a_call_settles_by_the_first_rule_that_can_measure_it() {
        use SettleBasis::{Estimated, Local, Reported, Reservation};
        let p = Some(price(1e-6, 2e-6));
        let cloud = Provider::Cloud;
        // A local answer is free, even when a bill is attached to it and a price is known.
        assert_eq!(
            settle_amount(Provider::Local, &usage(Some(9), Some(9), Some(5.0)), p, 1.0),
            (0.0, Local)
        );
        // The reported cost beats an estimate.
        assert_eq!(
            settle_amount(cloud, &usage(Some(1000), Some(500), Some(0.0042)), p, 1.0),
            (0.0042, Reported)
        );
        // A reported 0 is a real 0 (a free model), not "absent".
        assert_eq!(
            settle_amount(cloud, &usage(Some(1000), Some(500), Some(0.0)), p, 1.0),
            (0.0, Reported)
        );
        // Else tokens x the served model's price.
        let (usd, basis) = settle_amount(cloud, &usage(Some(1000), Some(500), None), p, 1.0);
        assert_eq!(basis, Estimated);
        assert!((usd - (1000.0 * 1e-6 + 500.0 * 2e-6)).abs() < 1e-12);
        // Else the whole booking counts: no price, a missing token count, or a negative one.
        assert_eq!(
            settle_amount(cloud, &usage(Some(1000), Some(500), None), None, 0.75),
            (0.75, Reservation)
        );
        assert_eq!(
            settle_amount(cloud, &usage(None, Some(500), None), p, 0.75),
            (0.75, Reservation)
        );
        assert_eq!(
            settle_amount(cloud, &usage(Some(1000), None, None), p, 0.75),
            (0.75, Reservation)
        );
        assert_eq!(
            settle_amount(cloud, &usage(Some(-5), Some(5), None), p, 0.75),
            (0.75, Reservation)
        );
        assert_eq!(
            settle_amount(cloud, &usage(None, None, None), None, 0.75),
            (0.75, Reservation)
        );
    }

    /// A cost nobody could have paid (negative, NaN, infinite) is not a report: the next rule runs.
    #[test]
    fn an_unusable_reported_cost_falls_through_to_the_next_rule() {
        let p = Some(price(1e-6, 1e-6));
        for bad in [-0.01, f64::NAN, f64::INFINITY] {
            let (usd, basis) = settle_amount(
                Provider::Cloud,
                &usage(Some(1000), Some(1000), Some(bad)),
                p,
                9.0,
            );
            assert_eq!(basis, SettleBasis::Estimated, "{bad}");
            assert!((usd - 0.002).abs() < 1e-12);
            assert_eq!(
                settle_amount(Provider::Cloud, &usage(None, None, Some(bad)), None, 9.0),
                (9.0, SettleBasis::Reservation)
            );
        }
    }

    // --- the ledger -----------------------------------------------------------------------------

    /// Every store in these tests has this epoch unless a test says otherwise. These wrappers shadow
    /// the real functions so the many one-store tests read as they did; the cross-store test calls
    /// `super::` with two epochs.
    const EPOCH: u64 = 7;

    fn reserve(
        conn: &mut Connection,
        mod_id: &str,
        month: &str,
        usd: f64,
        cloud: bool,
    ) -> Result<Reservation, ReserveError> {
        super::reserve(conn, EPOCH, mod_id, month, usd, cloud)
    }

    fn settle(conn: &mut Connection, res: Reservation, usd: f64) -> rusqlite::Result<Settled> {
        super::settle(conn, EPOCH, res, usd)
    }

    fn settle_unknown(conn: &mut Connection, res: Reservation) -> rusqlite::Result<Settled> {
        super::settle_unknown(conn, EPOCH, res)
    }

    fn release(conn: &mut Connection, res: Reservation) -> rusqlite::Result<()> {
        super::release(conn, EPOCH, res)
    }

    fn settle_and_log(
        conn: &mut Connection,
        res: Reservation,
        completion: &Completion,
        meta: &CallMeta,
        model: Option<&str>,
    ) -> rusqlite::Result<Settled> {
        super::settle_and_log(conn, EPOCH, res, completion, meta, model)
    }

    fn with_mod(cap: Option<f64>) -> (tempfile::TempDir, Connection) {
        let (dir, conn) = store();
        insert_mod(&conn, &ModRow::signed("acme.tool", "second", &["ai"]));
        conn.execute("UPDATE mods SET ai_cap_usd = ?1", params![cap])
            .unwrap();
        (dir, conn)
    }

    fn spent(conn: &Connection, id: &str) -> f64 {
        conn.query_row(
            "SELECT ai_spent_usd FROM mods WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn month_of(conn: &Connection, id: &str) -> Option<String> {
        conn.query_row(
            "SELECT ai_month FROM mods WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn book(conn: &mut Connection, usd: f64) -> Result<Reservation, ReserveError> {
        reserve(conn, "acme.tool", NOV, usd, true)
    }

    fn meta(provider: Provider) -> CallMeta {
        CallMeta {
            provider,
            latency_ms: 12,
            fallback: None,
            displaced_local_model: None,
        }
    }

    fn completion(model: Option<&str>, usage: Usage) -> Completion {
        Completion {
            text: "hello".into(),
            model: model.map(str::to_string),
            usage,
            truncated: false,
        }
    }

    #[test]
    fn reservations_fill_the_cap_and_the_next_one_is_refused() {
        let (_dir, mut conn) = with_mod(Some(1.0));
        let a = book(&mut conn, 0.4).unwrap();
        let b = book(&mut conn, 0.4).unwrap();
        assert!(a.is_booked() && b.is_booked());
        assert_eq!(reserved_usd(&conn, "acme.tool").unwrap(), 0.8);
        assert!(matches!(
            book(&mut conn, 0.4),
            Err(ReserveError::CapReached)
        ));
        // A refusal books nothing.
        assert_eq!(reserved_usd(&conn, "acme.tool").unwrap(), 0.8);
        // What still fits does.
        assert!(book(&mut conn, 0.2).is_ok());
    }

    #[test]
    fn a_cloud_call_with_no_cap_asks_and_a_free_one_runs_without_touching_the_ledger() {
        let (_dir, mut conn) = with_mod(None);
        assert!(matches!(book(&mut conn, 0.01), Err(ReserveError::NeedsCap)));
        // The refused cloud call adopted the month (a refusal keeps what it did) and booked nothing.
        assert_eq!(month_of(&conn, "acme.tool").as_deref(), Some(NOV));
        // A free call in another month changes nothing at all: it never reads the mod's row.
        let free = reserve(&mut conn, "acme.tool", DEC, 0.0, false).unwrap();
        assert!(!free.is_booked());
        assert_eq!(reserved_usd(&conn, "acme.tool").unwrap(), 0.0);
        assert_eq!(month_of(&conn, "acme.tool").as_deref(), Some(NOV));
        // Settling a free call writes nothing either.
        let out = settle(&mut conn, free, 0.0).unwrap();
        assert_eq!(
            out,
            Settled {
                charged_usd: 0.0,
                mod_present: true,
                foreign_store: false
            }
        );
        assert_eq!(spent(&conn, "acme.tool"), 0.0);
    }

    /// A free call needs no mod row at all: it can't have been the thing a cap was set for.
    #[test]
    fn a_free_call_for_a_mod_that_is_gone_is_not_an_error_here() {
        let (_dir, mut conn) = store();
        assert!(reserve(&mut conn, "ghost.mod", NOV, 0.0, false).is_ok());
    }

    #[test]
    fn cap_zero_means_no_cloud_spend() {
        let (_dir, mut conn) = with_mod(Some(0.0));
        assert!(matches!(
            book(&mut conn, 0.000001),
            Err(ReserveError::CapReached)
        ));
        // A cloud model that prices at 0 fits under a cap of 0.
        assert!(book(&mut conn, 0.0).is_ok());
    }

    #[test]
    fn a_call_that_exactly_fills_the_cap_is_admitted() {
        let (_dir, mut conn) = with_mod(Some(1.0));
        assert!(book(&mut conn, 0.5).is_ok());
        assert!(book(&mut conn, 0.5).is_ok());
        assert!(matches!(
            book(&mut conn, 0.0001),
            Err(ReserveError::CapReached)
        ));
    }

    #[test]
    fn a_bad_amount_is_refused_before_anything_is_written() {
        let (_dir, mut conn) = with_mod(Some(1.0));
        for bad in [f64::NAN, f64::INFINITY, -0.5] {
            assert!(
                matches!(book(&mut conn, bad), Err(ReserveError::BadAmount)),
                "{bad}"
            );
        }
        assert_eq!(reserved_usd(&conn, "acme.tool").unwrap(), 0.0);
    }

    #[test]
    fn a_mod_that_is_not_installed_cannot_book() {
        let (_dir, mut conn) = store();
        assert!(matches!(
            reserve(&mut conn, "ghost.mod", NOV, 0.1, true),
            Err(ReserveError::ModGone)
        ));
    }

    #[test]
    fn each_mod_has_its_own_cap_and_its_own_bookings() {
        let (_dir, mut conn) = with_mod(Some(1.0));
        insert_mod(&conn, &ModRow::signed("acme.other", "second", &["ai"]));
        conn.execute(
            "UPDATE mods SET ai_cap_usd = 0.5 WHERE id = 'acme.other'",
            [],
        )
        .unwrap();
        assert!(book(&mut conn, 1.0).is_ok());
        // The first mod being full says nothing about the second.
        let other = reserve(&mut conn, "acme.other", NOV, 0.5, true);
        assert!(other.is_ok());
        assert!(matches!(
            reserve(&mut conn, "acme.other", NOV, 0.01, true),
            Err(ReserveError::CapReached)
        ));
        assert_eq!(reserved_usd(&conn, "acme.tool").unwrap(), 1.0);
        assert_eq!(reserved_usd(&conn, "acme.other").unwrap(), 0.5);
    }

    #[test]
    fn settling_swaps_the_booking_for_the_real_cost_and_frees_the_rest() {
        let (_dir, mut conn) = with_mod(Some(1.0));
        let res = book(&mut conn, 0.9).unwrap();
        assert!(matches!(
            book(&mut conn, 0.2),
            Err(ReserveError::CapReached)
        ));
        let out = settle(&mut conn, res, 0.1).unwrap();
        assert_eq!(
            out,
            Settled {
                charged_usd: 0.1,
                mod_present: true,
                foreign_store: false
            }
        );
        assert_eq!(reserved_usd(&conn, "acme.tool").unwrap(), 0.0);
        assert_eq!(spent(&conn, "acme.tool"), 0.1);
        // The 0.8 that was held is free again: 0.1 settled + 0.9 fits, 0.95 does not.
        assert!(matches!(
            book(&mut conn, 0.95),
            Err(ReserveError::CapReached)
        ));
        assert!(book(&mut conn, 0.9).is_ok());
    }

    /// Booking ids are per-database counters and mod ids repeat across vaults. A call that began in
    /// one store and ends after a vault switch must not delete another store's booking (which would
    /// free headroom that is in use) nor charge another store's mod.
    #[test]
    fn a_booking_settles_only_in_the_store_it_was_made_in() {
        let (_dir_a, mut a) = with_mod(Some(1.0));
        let (_dir_b, mut b) = with_mod(Some(1.0));
        let done = completion(Some("a/m"), usage(Some(10), Some(5), Some(0.02)));
        let foreign = Settled {
            charged_usd: 0.0,
            mod_present: false,
            foreign_store: true,
        };
        // Same mod id, and the same booking id (1) in each store; B's is another call in flight.
        let held_b = super::reserve(&mut b, 2, "acme.tool", NOV, 0.3, true).unwrap();
        for how in 0..4 {
            // AUTOINCREMENT does not reuse ids; start A's count again so each round collides.
            a.execute(
                "DELETE FROM sqlite_sequence WHERE name = 'ai_reservations'",
                [],
            )
            .unwrap();
            let res_a = super::reserve(&mut a, 1, "acme.tool", NOV, 0.5, true).unwrap();
            assert_eq!(res_a.id, held_b.id, "the ids collide, which is the hazard");
            let out = match how {
                0 => super::settle(&mut b, 2, res_a, 0.5),
                1 => super::settle_unknown(&mut b, 2, res_a),
                2 => super::release(&mut b, 2, res_a).map(|()| foreign_marker()),
                _ => super::settle_and_log(
                    &mut b,
                    2,
                    res_a,
                    &done,
                    &meta(Provider::Cloud),
                    Some("a/m"),
                ),
            }
            .unwrap();
            assert!(out.foreign_store, "{how}");
            assert_eq!(out.charged_usd, 0.0, "{how}");
            // B is exactly as it was: its booking stands, nothing was charged, nothing was logged.
            assert_eq!(reserved_usd(&b, "acme.tool").unwrap(), 0.3, "{how}");
            assert_eq!(spent(&b, "acme.tool"), 0.0, "{how}");
            assert_eq!(
                b.query_row("SELECT COUNT(*) FROM usage_log", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                0,
                "{how}"
            );
            // A's own booking stays where it was made (it counts there: errs high).
            assert_eq!(reserved_usd(&a, "acme.tool").unwrap(), 0.5, "{how}");
            a.execute("DELETE FROM ai_reservations", []).unwrap();
        }
        // A free call from another store is refused the same way, even with a charge attached.
        let free_a = super::reserve(&mut a, 1, "acme.tool", NOV, 0.0, false).unwrap();
        let out = super::settle(&mut b, 2, free_a, 0.4).unwrap();
        assert_eq!(out, foreign);
        assert_eq!(spent(&b, "acme.tool"), 0.0);
        // In its own store it settles as always.
        let own = super::reserve(&mut a, 1, "acme.tool", NOV, 0.5, true).unwrap();
        let out = super::settle(&mut a, 1, own, 0.2).unwrap();
        assert!(!out.foreign_store);
        assert_eq!(spent(&a, "acme.tool"), 0.2);
        assert_eq!(reserved_usd(&a, "acme.tool").unwrap(), 0.0);
        let out = super::settle(&mut b, 2, held_b, 0.1).unwrap();
        assert!(!out.foreign_store);
        assert_eq!(spent(&b, "acme.tool"), 0.1);
    }

    /// `release` returns nothing; this stands in for its outcome in the loop above.
    fn foreign_marker() -> Settled {
        Settled {
            charged_usd: 0.0,
            mod_present: false,
            foreign_store: true,
        }
    }

    /// A call may cost more than its booking (a provider that ignores `max_tokens`, a price that
    /// moved). The real cost is charged in full, so spend can pass the cap by that excess, and the
    /// next call is refused: never under-counted.
    #[test]
    fn a_settle_above_the_booking_is_charged_in_full() {
        let (_dir, mut conn) = with_mod(Some(1.0));
        let res = book(&mut conn, 0.1).unwrap();
        settle(&mut conn, res, 0.7).unwrap();
        assert_eq!(spent(&conn, "acme.tool"), 0.7);
        assert!(matches!(
            book(&mut conn, 0.4),
            Err(ReserveError::CapReached)
        ));
    }

    #[test]
    fn a_settle_with_a_number_nobody_can_charge_counts_the_whole_booking() {
        for bad in [f64::NAN, f64::INFINITY, -1.0] {
            let (_dir, mut conn) = with_mod(Some(1.0));
            let res = book(&mut conn, 0.6).unwrap();
            let out = settle(&mut conn, res, bad).unwrap();
            assert_eq!(out.charged_usd, 0.6, "{bad}");
            assert_eq!(spent(&conn, "acme.tool"), 0.6);
            assert_eq!(reserved_usd(&conn, "acme.tool").unwrap(), 0.0);
        }
    }

    #[test]
    fn an_unknown_outcome_counts_as_spent_and_a_release_counts_as_nothing() {
        let (_dir, mut conn) = with_mod(Some(1.0));
        let timed_out = book(&mut conn, 0.3).unwrap();
        let out = settle_unknown(&mut conn, timed_out).unwrap();
        assert_eq!(
            out,
            Settled {
                charged_usd: 0.3,
                mod_present: true,
                foreign_store: false
            }
        );
        assert_eq!(spent(&conn, "acme.tool"), 0.3);
        let never_sent = book(&mut conn, 0.3).unwrap();
        release(&mut conn, never_sent).unwrap();
        assert_eq!(spent(&conn, "acme.tool"), 0.3);
        assert_eq!(reserved_usd(&conn, "acme.tool").unwrap(), 0.0);
    }

    /// A process kill leaves the booking row. It keeps counting against the cap for the rest of the
    /// month (plan decision 4), however long ago it was made, and goes only when the month rolls.
    #[test]
    fn a_booking_a_crash_left_counts_until_the_month_ends() {
        let (_dir, mut conn) = with_mod(Some(1.0));
        let leaked = book(&mut conn, 0.8).unwrap();
        // The process dies: no settle, no release. The row's age makes no difference.
        std::mem::forget(leaked);
        conn.execute(
            "UPDATE ai_reservations SET created_at = '2026-11-01T00:00:00.000Z'",
            [],
        )
        .unwrap();
        assert!(matches!(
            book(&mut conn, 0.3),
            Err(ReserveError::CapReached)
        ));
        assert!(book(&mut conn, 0.2).is_ok());
        // Later the same month, still counted.
        assert!(matches!(
            reserve(&mut conn, "acme.tool", NOV, 0.3, true),
            Err(ReserveError::CapReached)
        ));
        // The month rolls: the leftover and the spend are gone, and the new month starts clean.
        let fresh = reserve(&mut conn, "acme.tool", DEC, 1.0, true).unwrap();
        assert_eq!(fresh.usd(), 1.0);
        assert_eq!(reserved_usd(&conn, "acme.tool").unwrap(), 1.0);
        assert_eq!(month_of(&conn, "acme.tool").as_deref(), Some(DEC));
    }

    #[test]
    fn the_month_roll_clears_settled_spend_and_only_this_mods_bookings() {
        let (_dir, mut conn) = with_mod(Some(1.0));
        insert_mod(&conn, &ModRow::signed("acme.other", "second", &["ai"]));
        conn.execute(
            "UPDATE mods SET ai_cap_usd = 1.0 WHERE id = 'acme.other'",
            [],
        )
        .unwrap();
        let a = book(&mut conn, 0.6).unwrap();
        settle(&mut conn, a, 0.6).unwrap();
        let _other = reserve(&mut conn, "acme.other", NOV, 0.4, true).unwrap();
        assert!(matches!(
            book(&mut conn, 0.6),
            Err(ReserveError::CapReached)
        ));
        // December: this mod starts again, the other mod's November booking is not touched.
        assert!(reserve(&mut conn, "acme.tool", DEC, 0.9, true).is_ok());
        assert_eq!(spent(&conn, "acme.tool"), 0.0);
        assert_eq!(reserved_usd(&conn, "acme.other").unwrap(), 0.4);
        assert_eq!(month_of(&conn, "acme.other").as_deref(), Some(NOV));
    }

    /// The roll is kept even when the call that triggered it is then refused.
    #[test]
    fn a_refused_call_still_keeps_the_roll_it_made() {
        let (_dir, mut conn) = with_mod(Some(0.5));
        let a = book(&mut conn, 0.5).unwrap();
        settle(&mut conn, a, 0.5).unwrap();
        assert!(matches!(
            reserve(&mut conn, "acme.tool", DEC, 5.0, true),
            Err(ReserveError::CapReached)
        ));
        assert_eq!(spent(&conn, "acme.tool"), 0.0);
        assert_eq!(month_of(&conn, "acme.tool").as_deref(), Some(DEC));
    }

    /// A stored month later than today's (a clock that ran fast, then was corrected, or one set
    /// back on purpose) never resets: the spend and the stored month stay.
    #[test]
    fn a_clock_set_back_cannot_clear_spend() {
        let (_dir, mut conn) = with_mod(Some(1.0));
        let a = reserve(&mut conn, "acme.tool", "2027-03", 0.9, true).unwrap();
        settle(&mut conn, a, 0.9).unwrap();
        // Back in November 2026: still 0.9 spent, still 2027-03.
        assert!(matches!(
            book(&mut conn, 0.2),
            Err(ReserveError::CapReached)
        ));
        assert_eq!(spent(&conn, "acme.tool"), 0.9);
        assert_eq!(month_of(&conn, "acme.tool").as_deref(), Some("2027-03"));
        assert!(book(&mut conn, 0.1).is_ok());
    }

    /// The documented limit, pinned so it is deliberate: a clock that jumps FORWARD clears the
    /// month's spend, and after the clock is corrected the ledger stays on the false month with the
    /// spend made meanwhile (never more than one cap lost, never an unbounded charge).
    #[test]
    fn a_clock_that_ran_fast_clears_the_month_early_and_the_ledger_stays_on_it() {
        let (_dir, mut conn) = with_mod(Some(1.0));
        let a = book(&mut conn, 0.9).unwrap();
        settle(&mut conn, a, 0.9).unwrap();
        // December on a fast clock: November's 0.9 is gone.
        let skewed = reserve(&mut conn, "acme.tool", DEC, 0.9, true).unwrap();
        assert_eq!(spent(&conn, "acme.tool"), 0.0);
        settle(&mut conn, skewed, 0.9).unwrap();
        // The clock is corrected: still blocked, still December, the spend carries.
        assert!(matches!(
            book(&mut conn, 0.2),
            Err(ReserveError::CapReached)
        ));
        assert_eq!(month_of(&conn, "acme.tool").as_deref(), Some(DEC));
        assert_eq!(spent(&conn, "acme.tool"), 0.9);
        // Real December arrives: no second reset, the carried spend still counts.
        assert!(matches!(
            reserve(&mut conn, "acme.tool", DEC, 0.2, true),
            Err(ReserveError::CapReached)
        ));
        assert_eq!(spent(&conn, "acme.tool"), 0.9);
    }

    #[test]
    fn a_missing_or_unreadable_month_is_adopted_with_the_spend_kept() {
        for stored in [None, Some("garbage")] {
            let (_dir, mut conn) = with_mod(Some(1.0));
            conn.execute(
                "UPDATE mods SET ai_spent_usd = 0.7, ai_month = ?1",
                params![stored],
            )
            .unwrap();
            assert!(matches!(
                book(&mut conn, 0.4),
                Err(ReserveError::CapReached)
            ));
            assert_eq!(spent(&conn, "acme.tool"), 0.7);
            assert_eq!(month_of(&conn, "acme.tool").as_deref(), Some(NOV));
        }
    }

    /// Lowering the cap below what is spent refuses every priced call from then on; raising it
    /// lets the next call through.
    #[test]
    fn lowering_the_cap_stops_calls_and_raising_it_resumes_them() {
        let (_dir, mut conn) = with_mod(Some(1.0));
        let a = book(&mut conn, 0.8).unwrap();
        settle(&mut conn, a, 0.8).unwrap();
        conn.execute("UPDATE mods SET ai_cap_usd = 0.5", [])
            .unwrap();
        assert!(matches!(
            book(&mut conn, 0.0),
            Err(ReserveError::CapReached)
        ));
        conn.execute("UPDATE mods SET ai_cap_usd = 2.0", [])
            .unwrap();
        assert!(book(&mut conn, 1.0).is_ok());
    }

    /// The mod goes (uninstall cascades its bookings) while a call is out: the settle hits no row,
    /// says so, and does not fail; `settle_and_log` still writes the usage row, with the mod's id.
    #[test]
    fn an_uninstall_mid_call_still_logs_the_spend_and_charges_no_row() {
        let (_dir, mut conn) = with_mod(Some(1.0));
        let res = book(&mut conn, 0.5).unwrap();
        conn.execute("DELETE FROM mods WHERE id = 'acme.tool'", [])
            .unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM ai_reservations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        let done = completion(Some("a/m"), usage(Some(10), Some(5), Some(0.02)));
        let out =
            settle_and_log(&mut conn, res, &done, &meta(Provider::Cloud), Some("a/m")).unwrap();
        assert_eq!(
            out,
            Settled {
                charged_usd: 0.02,
                mod_present: false,
                foreign_store: false
            }
        );
        let (mod_id, kind, cost): (String, String, f64) = conn
            .query_row("SELECT mod_id, kind, cost_usd FROM usage_log", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(
            (mod_id.as_str(), kind.as_str(), cost),
            ("acme.tool", "background", 0.02)
        );
    }

    #[test]
    fn a_finished_call_settles_and_logs_by_the_rules() {
        let (_dir, mut conn) = with_mod(Some(1.0));
        conn.execute(
            "INSERT INTO model_pricing(model, prompt_price, completion_price, fetched_at) \
             VALUES ('a/served', 1e-6, 2e-6, '2026-11-17T00:00:00.000Z')",
            [],
        )
        .unwrap();
        // Tokens only: priced from the SERVED model (not the plan's primary, a/primary).
        let res = book(&mut conn, 0.5).unwrap();
        let done = completion(Some("a/served"), usage(Some(1000), Some(500), None));
        let out = settle_and_log(
            &mut conn,
            res,
            &done,
            &meta(Provider::Cloud),
            Some("a/served"),
        )
        .unwrap();
        assert!((out.charged_usd - 0.002).abs() < 1e-12);
        assert!((spent(&conn, "acme.tool") - 0.002).abs() < 1e-12);
        // The served model is unknown: the booking counts.
        let res = book(&mut conn, 0.25).unwrap();
        let done = completion(None, usage(Some(1000), Some(500), None));
        let out = settle_and_log(
            &mut conn,
            res,
            &done,
            &meta(Provider::Cloud),
            Some("a/primary"),
        )
        .unwrap();
        assert_eq!(out.charged_usd, 0.25);
        // A local answer: nothing charged, the booking is gone, the row is still logged as local.
        let res = book(&mut conn, 0.25).unwrap();
        let done = completion(Some("llama"), usage(Some(10), Some(10), None));
        let out =
            settle_and_log(&mut conn, res, &done, &meta(Provider::Local), Some("llama")).unwrap();
        assert_eq!(out.charged_usd, 0.0);
        assert_eq!(reserved_usd(&conn, "acme.tool").unwrap(), 0.0);
        let rows: Vec<(String, Option<String>, String)> = conn
            .prepare("SELECT mod_id, model, provider FROM usage_log ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r.0 == "acme.tool"));
        assert_eq!(rows[1].1.as_deref(), Some("a/primary"));
        assert_eq!(rows[2].2, "local");
    }

    // --- concurrency ----------------------------------------------------------------------------

    /// Sixteen calls race for a cap that holds eight of them: exactly eight are booked, and the
    /// total never passes the cap. Every call is one reserve under the one connection guard,
    /// the way a handler takes `state.conn()`. 0.125 is exact in binary, so the sum is exact too.
    #[test]
    fn parallel_calls_cannot_overshoot_the_cap() {
        let (_dir, conn) = with_mod(Some(1.0));
        let conn = Arc::new(Mutex::new(conn));
        let start = Arc::new(Barrier::new(16));
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let (conn, start) = (conn.clone(), start.clone());
                std::thread::spawn(move || {
                    start.wait();
                    let mut guard = conn.lock().unwrap();
                    reserve(&mut guard, "acme.tool", NOV, 0.125, true)
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let booked = results.iter().filter(|r| r.is_ok()).count();
        let refused = results
            .iter()
            .filter(|r| matches!(r, Err(ReserveError::CapReached)))
            .count();
        assert_eq!((booked, refused), (8, 8));
        assert_eq!(
            reserved_usd(&conn.lock().unwrap(), "acme.tool").unwrap(),
            1.0
        );
    }

    /// The same race over four SEPARATE connections to one file: the IMMEDIATE transaction is what
    /// keeps them from reading the same headroom. (In PM there is one connection; this proves the
    /// transaction itself, not only the mutex around it.)
    #[test]
    fn parallel_connections_cannot_overshoot_either() {
        let (dir, conn) = with_mod(Some(1.0));
        let path = dir.path().join("pm.sqlite");
        drop(conn);
        let start = Arc::new(Barrier::new(4));
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let (path, start) = (path.clone(), start.clone());
                std::thread::spawn(move || {
                    let mut conn = crate::db::open(&path, DB_KEY).unwrap();
                    conn.busy_timeout(std::time::Duration::from_secs(30))
                        .unwrap();
                    start.wait();
                    (0..4)
                        .map(|_| reserve(&mut conn, "acme.tool", NOV, 0.125, true))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();
        assert!(
            results
                .iter()
                .all(|r| r.is_ok() || matches!(r, Err(ReserveError::CapReached))),
            "{results:?}"
        );
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 8);
        let conn = crate::db::open(&path, DB_KEY).unwrap();
        assert_eq!(reserved_usd(&conn, "acme.tool").unwrap(), 1.0);
    }
}
