// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The daily spend limit: a brake the person sets, per provider, that pauses that provider for the
//! rest of their day once PM has logged that much spent on it. Off by default.
//!
//! It is a brake and not a gate on price. The check is `spent_today >= limit` BEFORE a call, never
//! "spent plus what this call could cost": a predictive gate would refuse every Fable reply under a
//! $5 limit (one worst-case reply is $5.12) with nothing spent. The price is that the call that
//! crosses the line can overshoot by one reply, and parallel background calls already in flight by
//! that many; the field's help says so.
//!
//! What it counts is what PM logged ([`crate::commands::spent_since`], the code the Usage tab prices
//! with). A reply that fails after billing logs no row, a key used outside PM is invisible, and
//! another computer on the same key is not counted (`usage_log` is a device table). The provider's
//! own spend limit is the hard stop, and the help names it.
//!
//! The one place it is asked is the top of the two cloud dispatch functions in `llm_gateway`
//! ([`gate`]), so chat, Review, organise, canon, a mod's call and the On battery route all meet it
//! for all four providers. A refusal leaves the mod wire mark at "never left" (nothing was sent), so
//! a mod's booking is given back.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, OnceLock};

use chrono::{DateTime, Days, LocalResult, TimeZone, Utc};
use chrono_tz::Tz;
use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::error::{Billing, Error, FailureClass, ProviderFailure, Result};
use crate::llm_gateway::CloudProvider;
use crate::{clock, db, AppState};

/// The limit the field starts from when the person turns it on. A suggestion only: no provider has a
/// limit until the person has written one.
pub const SUGGESTED_DAILY_LIMIT_USD: f64 = 5.0;

/// The settings key a provider's limit is stored under: a decimal dollar amount, absent when the
/// limit is off. Exhaustive, so a fifth provider fails to compile here. Device class, never in the
/// webview's allow-list: only [`write`] (through `set_daily_limit`) sets it.
pub(crate) fn limit_key(p: CloudProvider) -> &'static str {
    match p {
        CloudProvider::OpenRouter => "daily_limit_openrouter_usd",
        CloudProvider::OpenAi => "daily_limit_openai_usd",
        CloudProvider::Gemini => "daily_limit_gemini_usd",
        CloudProvider::Anthropic => "daily_limit_anthropic_usd",
    }
}

/// Whether `x` can be a daily limit: above zero, finite, and no more than the same ceiling the mods'
/// monthly limit uses. "Off" is `None`, never 0: a limit of nothing would be a pause that never
/// lifts, and a person who means that turns the provider off.
pub fn valid_limit(x: f64) -> bool {
    x.is_finite() && x > 0.0 && x <= crate::mods::ai::MAX_CAP_USD
}

/// What the store holds for a provider's limit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Setting {
    Off,
    On(f64),
    /// A row is there but is not a limit PM could have written (a hand-edited store, a restore from
    /// something else). It reads as "can't tell", never as off: a brake that quietly lets go is worse
    /// than one that asks the person to look.
    Unreadable,
}

/// The stored limit, or the reason it can't be told.
pub(crate) fn read_setting(conn: &Connection, p: CloudProvider) -> Result<Setting> {
    Ok(match db::get_setting(conn, limit_key(p))? {
        None => Setting::Off,
        Some(raw) => match raw.trim().parse::<f64>() {
            Ok(x) if valid_limit(x) => Setting::On(x),
            _ => Setting::Unreadable,
        },
    })
}

/// Set or clear a provider's limit. `None` clears it; anything else must pass [`valid_limit`].
/// Drops this provider's remembered pause, so raising the limit lifts it at once.
pub(crate) fn write(conn: &Connection, p: CloudProvider, usd: Option<f64>) -> Result<()> {
    match usd {
        None => db::delete_setting(conn, limit_key(p))?,
        Some(x) if valid_limit(x) => db::set_setting(conn, limit_key(p), &format!("{x}"))?,
        Some(_) => {
            return Err(Error::Other(
                "Enter an amount above 0, or turn the limit off.".into(),
            ))
        }
    }
    forget(conn, p);
    Ok(())
}

// --- the person's day ---------------------------------------------------------------------------

/// Where "today" starts and when it ends, for one zone at one instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Day {
    /// The zone's most recent midnight as UTC in SQLite's `YYYY-MM-DD HH:MM:SS` form. Not comparable
    /// with `usage_log.created_at` as it stands: [`crate::commands::spent_since`] normalises it.
    pub since: String,
    /// The next midnight in the zone, as a UTC instant.
    pub resumes_at: DateTime<Utc>,
}

/// The day `now` falls in, in `zone`: the same civil day the Calendar, the briefing and the mods'
/// monthly limit use, so the boundary matches what the person sees. On a day the clocks change it is
/// 23 or 25 hours long, and a zone with no 00:00 that day starts at its first valid instant.
pub(crate) fn day_bounds(zone: Tz, now: DateTime<Utc>) -> Day {
    let since = clock::day_start_utc_in_at(zone, now);
    let next = clock::today_in_at(zone, now)
        .checked_add_days(Days::new(1))
        .unwrap_or_else(|| clock::today_in_at(zone, now));
    let midnight = next.and_hms_opt(0, 0, 0).expect("00:00:00 is valid");
    let resumes_at = match zone.from_local_datetime(&midnight) {
        LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => t.with_timezone(&Utc),
        // The clock skips this midnight: the day begins at the first instant that exists.
        LocalResult::None => zone
            .from_local_datetime(&(midnight + chrono::Duration::hours(1)))
            .earliest()
            .map_or_else(
                || now + chrono::Duration::hours(24),
                |t| t.with_timezone(&Utc),
            ),
    };
    Day { since, resumes_at }
}

/// The zone PM keeps days in: the stored one, UTC until one is set. Unlike
/// [`crate::commands::shared::resolve_zone`], which falls back to UTC on any trouble, a failed READ
/// is an error here: a limit that cannot place its day fails closed rather than count the wrong one.
fn zone_of(conn: &Connection) -> Result<Tz> {
    crate::commands::try_resolve_zone(conn)
}

// --- the decision -------------------------------------------------------------------------------

/// Whether `spent` has reached `limit`. `>=`, not `>`: "once PM has logged this much" means the
/// provider is paused AT the limit, and a $5.00 limit stops at exactly $5.00.
pub(crate) fn is_reached(spent: f64, limit: f64) -> bool {
    spent >= limit
}

/// A dollar figure as the limit's sentences print it: cents, and four places under a cent so a
/// $0.01 limit never reads "$0.00 of $0.01".
pub fn usd_text(x: f64) -> String {
    if x == 0.0 || x >= 0.01 {
        format!("{x:.2}")
    } else {
        format!("{x:.4}")
    }
}

/// What a person is told when a request is refused for the limit (chat error, background log, a
/// mod's error). Never moves the request on by itself: the next company is the person's choice.
pub(crate) fn refusal_sentence(p: CloudProvider, spent: f64, limit: f64) -> String {
    let (name, where_to_raise) = match p {
        CloudProvider::OpenRouter => ("OpenRouter", "under your OpenRouter key in AI & Models"),
        other => (other.display_name(), "in Your own keys"),
    };
    format!(
        "{name} is paused: PM has logged at least ${spent} today, over your daily limit of ${limit}. \
         It starts again at 00:00 (your day), or sooner if you raise the limit {where_to_raise}. \
         PM won't send this reply to another company on its own; you can choose to.",
        spent = usd_text(spent),
        limit = usd_text(limit),
    )
}

/// The refusal when a limit is on and PM cannot say what has been spent. Fails closed.
pub(crate) fn unreadable_sentence(p: CloudProvider) -> String {
    format!(
        "PM couldn't read today's usage, so it paused {name} to be safe. Check Usage, or turn the limit off.",
        name = p.display_name()
    )
}

fn limit_failure(p: CloudProvider, message: String) -> Error {
    Error::Provider(Box::new(ProviderFailure {
        class: FailureClass::LimitReached,
        billing: Billing::NeverLeft,
        provider: p,
        model: None,
        message,
    }))
}

// --- what has been remembered once a provider is paused -----------------------------------------

/// A pause already decided for today, so a tripped provider's later calls do not each sum the log.
#[derive(Clone, Copy)]
struct Tripped {
    until: DateTime<Utc>,
    limit: f64,
    spent: f64,
}

/// Which pause: a provider's, in one store. The store's own path is part of the key, so a pause
/// decided in one vault is never read as another vault's after a switch. A restore can land on a
/// path this map already knows (a pristine home is re-homed onto the default path), so a stale
/// pause may outlive one; that is harmless and fail-safe: a pristine store holds no usage to trip
/// on, the pause ends at midnight, it is ignored when the limit differs, and any write to the
/// limit or a zone change clears it.
type TrippedKey = (String, &'static str);

static TRIPPED: LazyLock<Mutex<HashMap<TrippedKey, Tripped>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn tripped_key(conn: &Connection, p: CloudProvider) -> TrippedKey {
    let store = conn.path().map(str::to_owned).unwrap_or_default();
    (store, p.as_str())
}

fn remembered(
    conn: &Connection,
    p: CloudProvider,
    limit: f64,
    now: DateTime<Utc>,
) -> Option<Tripped> {
    let key = tripped_key(conn, p);
    let mut map = TRIPPED.lock().ok()?;
    match map.get(&key).copied() {
        // Still today, and the limit is the one it tripped on: a raised or lowered limit is a new
        // question, answered from the log.
        Some(t) if now < t.until && t.limit == limit => Some(t),
        Some(_) => {
            map.remove(&key);
            None
        }
        None => None,
    }
}

fn remember(conn: &Connection, p: CloudProvider, t: Tripped) {
    if let Ok(mut map) = TRIPPED.lock() {
        map.insert(tripped_key(conn, p), t);
    }
}

/// Forget a provider's pause (its limit changed or was cleared).
pub(crate) fn forget(conn: &Connection, p: CloudProvider) {
    if let Ok(mut map) = TRIPPED.lock() {
        map.remove(&tripped_key(conn, p));
    }
}

/// Forget every pause: the time zone changed, so "today" is a different span.
pub(crate) fn forget_all() {
    if let Ok(mut map) = TRIPPED.lock() {
        map.clear();
    }
}

// --- reading today ------------------------------------------------------------------------------

struct Today {
    spent: f64,
    unpriced: i64,
    resumes_at: DateTime<Utc>,
}

fn read_today(conn: &Connection, p: CloudProvider, now: DateTime<Utc>) -> Result<Today> {
    let day = day_bounds(zone_of(conn)?, now);
    let s = crate::commands::spent_since(conn, &day.since, p.as_str())?;
    Ok(Today {
        spent: s.usd,
        unpriced: s.unpriced,
        resumes_at: day.resumes_at,
    })
}

/// The whole decision for one request to `p` at `now`, on an open store. Reads nothing about usage
/// when the provider has no limit. A limit that is on but cannot be read, or whose day or spend
/// cannot be worked out, refuses (logged, with no figure in the line).
pub(crate) fn check(conn: &Connection, p: CloudProvider, now: DateTime<Utc>) -> Result<()> {
    let limit = match read_setting(conn, p) {
        Ok(Setting::Off) => {
            forget(conn, p);
            return Ok(());
        }
        Ok(Setting::On(x)) => x,
        Ok(Setting::Unreadable) | Err(_) => {
            eprintln!(
                "daily_limit: the {} limit could not be read; paused to be safe",
                p.as_str()
            );
            return Err(limit_failure(p, unreadable_sentence(p)));
        }
    };
    if let Some(t) = remembered(conn, p, limit, now) {
        return Err(limit_failure(p, refusal_sentence(p, t.spent, t.limit)));
    }
    let today = match read_today(conn, p, now) {
        Ok(t) => t,
        Err(_) => {
            eprintln!(
                "daily_limit: today's {} usage could not be read; paused to be safe",
                p.as_str()
            );
            return Err(limit_failure(p, unreadable_sentence(p)));
        }
    };
    if is_reached(today.spent, limit) {
        remember(
            conn,
            p,
            Tripped {
                until: today.resumes_at,
                limit,
                spent: today.spent,
            },
        );
        return Err(limit_failure(p, refusal_sentence(p, today.spent, limit)));
    }
    Ok(())
}

// --- the seam the gateway asks ------------------------------------------------------------------

static APP: OnceLock<AppHandle> = OnceLock::new();

/// Hand over the app so [`gate`] can reach the store. Called once at start, before any model call. The
/// dispatch functions it guards take an arm and nothing else (the same reason the Gemini seam reads
/// the region from the OS), so the store is reached through here and not through a parameter every
/// caller would have to carry.
pub(crate) fn install(app: AppHandle) {
    let _ = APP.set(app);
}

#[cfg(test)]
type TestGate = std::rc::Rc<dyn Fn(CloudProvider) -> Result<()>>;

#[cfg(test)]
thread_local! {
    /// A test's stand-in for the store, so the dispatch functions can be driven without an app.
    static TEST_GATE: std::cell::RefCell<Option<TestGate>> = const { std::cell::RefCell::new(None) };
}

/// Makes `gate` answer from `decide` on this thread until the guard drops (a test's seam, for a
/// test that drives a dispatch function with no app).
#[cfg(test)]
pub(crate) struct TestGateGuard;

#[cfg(test)]
impl Drop for TestGateGuard {
    fn drop(&mut self) {
        TEST_GATE.with(|g| *g.borrow_mut() = None);
    }
}

#[cfg(test)]
pub(crate) fn test_gate(decide: impl Fn(CloudProvider) -> Result<()> + 'static) -> TestGateGuard {
    TEST_GATE.with(|g| *g.borrow_mut() = Some(std::rc::Rc::new(decide)));
    TestGateGuard
}

/// The refusal a reached limit gives, for a test that needs one without a store.
#[cfg(test)]
pub(crate) fn test_refusal(p: CloudProvider) -> Error {
    limit_failure(p, refusal_sentence(p, 5.12, 5.0))
}

/// May a request go to `p` now? Called FIRST by both cloud dispatch functions, before anything is
/// built or sent, with no database guard held: the guard it takes is gone before it returns, and no
/// caller awaits while it is held. A provider with no limit costs one settings read.
pub(crate) fn gate(p: CloudProvider) -> Result<()> {
    #[cfg(test)]
    if let Some(decide) = TEST_GATE.with(|g| g.borrow().clone()) {
        return decide(p);
    }
    let Some(app) = APP.get() else {
        // Not installed: no store to read (a unit test, or before start). Nothing to brake.
        return Ok(());
    };
    let state = app.state::<AppState>();
    // A store that will not open is the ordinary error every other command gives (the session is
    // unavailable), not a pause: whether a limit is on cannot be known, and a provider with no limit
    // never fails closed. The request would have needed the store a line earlier anyway (`cloud_arm`).
    let conn = state.conn()?;
    check(&conn, p, Utc::now())
}

/// Whether `p` is paused by its limit right now, for the places that decide a route without sending
/// (the On battery policy). Same answer as [`gate`]: a limit that is on and can't be read pauses; a
/// store that won't open is not a pause (the route's own store read reports that).
pub(crate) fn is_paused(app: &AppHandle, p: CloudProvider) -> bool {
    let state = app.state::<AppState>();
    let Ok(conn) = state.conn() else {
        return false;
    };
    check(&conn, p, Utc::now()).is_err()
}

// --- what the cards show ------------------------------------------------------------------------

/// A provider's limit as the card, the chip and the Usage read-out show it. Built in Rust: the page
/// never works out a day, a spend or whether the limit has been reached.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LimitView {
    /// The limit in dollars, or `None` while it is off.
    pub limit_usd: Option<f64>,
    /// What PM has logged today (zero while the limit is off: nothing is read then).
    pub spent_today_usd: f64,
    /// Calls today PM could not price: the figure above is a floor when this is above zero.
    pub unpriced_today: i64,
    /// The provider is paused for today. Also true when the limit is on but PM could not read it.
    pub reached: bool,
    /// The pause is because PM could not read the limit or today's usage, not because it was reached.
    pub unreadable: bool,
    /// When the pause lifts by itself: the next midnight in the person's zone (UTC, RFC 3339).
    pub resumes_at: String,
    /// The key is proven free (a free Gemini key), so there is nothing to limit. The page disables the
    /// control for it unless a limit is already on.
    pub free_key: bool,
    /// What the field is prefilled with when the person turns the limit on.
    pub suggested_usd: f64,
}

/// The limit's state for `p` at `now`. `free_key` is the caller's: only it knows the key's tier.
/// A limit that is off is not read against the log at all.
pub(crate) fn view(
    conn: &Connection,
    p: CloudProvider,
    free_key: bool,
    now: DateTime<Utc>,
) -> LimitView {
    let resumes = |zone_ok: Option<Tz>| {
        let zone = zone_ok.unwrap_or(Tz::UTC);
        day_bounds(zone, now).resumes_at.to_rfc3339()
    };
    let base = LimitView {
        limit_usd: None,
        spent_today_usd: 0.0,
        unpriced_today: 0,
        reached: false,
        unreadable: false,
        resumes_at: resumes(zone_of(conn).ok()),
        free_key,
        suggested_usd: SUGGESTED_DAILY_LIMIT_USD,
    };
    match read_setting(conn, p) {
        Ok(Setting::Off) => base,
        Ok(Setting::On(limit)) => match read_today(conn, p, now) {
            Ok(today) => LimitView {
                limit_usd: Some(limit),
                spent_today_usd: today.spent,
                unpriced_today: today.unpriced,
                reached: is_reached(today.spent, limit),
                resumes_at: today.resumes_at.to_rfc3339(),
                ..base
            },
            Err(_) => LimitView {
                limit_usd: Some(limit),
                reached: true,
                unreadable: true,
                ..base
            },
        },
        Ok(Setting::Unreadable) | Err(_) => LimitView {
            reached: true,
            unreadable: true,
            ..base
        },
    }
}

/// [`view`] for an open store and the real clock.
pub(crate) fn view_now(conn: &Connection, p: CloudProvider, free_key: bool) -> LimitView {
    view(conn, p, free_key, Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_gateway::{CallMeta, Provider};
    use crate::openrouter::Usage;
    use chrono::NaiveDate;

    const OR: CloudProvider = CloudProvider::OpenRouter;

    fn temp_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite");
        let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let conn = db::open(&path, key).unwrap();
        (dir, conn)
    }

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, min, 0)
            .unwrap()
            .and_utc()
    }

    fn tz(name: &str) -> Tz {
        name.parse().unwrap()
    }

    fn meta(p: CloudProvider) -> CallMeta {
        CallMeta {
            provider: Provider::Cloud,
            latency_ms: 5,
            fallback: None,
            displaced_local_model: None,
            cloud_provider: Some(p),
        }
    }

    /// One row through the ONE writer (`log_usage_as`), so it carries the real default stored
    /// `created_at`; then the stamp is set to `stamp`, written in that same form.
    fn row(conn: &Connection, p: CloudProvider, cost: Option<f64>, stamp: &str) {
        let usage = Usage {
            prompt_tokens: Some(10),
            completion_tokens: Some(5),
            cost,
            ..Default::default()
        };
        crate::commands::log_usage_as(conn, "chat", Some("vendor/model"), &usage, &meta(p), None);
        conn.execute(
            "UPDATE usage_log SET created_at = ?1 WHERE id = (SELECT MAX(id) FROM usage_log)",
            [stamp],
        )
        .unwrap();
    }

    fn set_zone(conn: &Connection, zone: &str) {
        db::set_setting(conn, "time_zone", zone).unwrap();
    }

    #[test]
    fn a_limit_is_above_zero_finite_and_under_the_ceiling() {
        for ok in [0.01, 1.0, 5.0, 100_000.0] {
            assert!(valid_limit(ok), "{ok}");
        }
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, 100_000.01] {
            assert!(!valid_limit(bad), "{bad}");
        }
        assert_eq!(SUGGESTED_DAILY_LIMIT_USD, 5.0);
    }

    #[test]
    fn the_four_providers_keep_their_limits_under_four_keys() {
        let keys: Vec<_> = [
            CloudProvider::OpenRouter,
            CloudProvider::OpenAi,
            CloudProvider::Gemini,
            CloudProvider::Anthropic,
        ]
        .map(limit_key)
        .into();
        assert_eq!(
            keys,
            [
                "daily_limit_openrouter_usd",
                "daily_limit_openai_usd",
                "daily_limit_gemini_usd",
                "daily_limit_anthropic_usd"
            ]
        );
    }

    /// The decision is "reached AT the limit": a $5.00 limit stops at exactly $5.00.
    #[test]
    fn the_limit_is_reached_at_the_limit_not_after_it() {
        assert!(is_reached(5.0, 5.0));
        assert!(is_reached(5.12, 5.0));
        assert!(!is_reached(4.99, 5.0));
        assert!(!is_reached(0.0, 5.0));
    }

    #[test]
    fn writing_a_limit_validates_and_off_is_none_never_zero() {
        let (_d, conn) = temp_db();
        assert_eq!(read_setting(&conn, OR).unwrap(), Setting::Off);
        write(&conn, OR, Some(5.0)).unwrap();
        assert_eq!(read_setting(&conn, OR).unwrap(), Setting::On(5.0));
        for bad in [0.0, -2.0, f64::NAN, 1e12] {
            assert!(write(&conn, OR, Some(bad)).is_err(), "{bad}");
        }
        assert_eq!(read_setting(&conn, OR).unwrap(), Setting::On(5.0));
        write(&conn, OR, None).unwrap();
        assert_eq!(read_setting(&conn, OR).unwrap(), Setting::Off);
        // A junk row (a hand-edited store) is "can't tell", not off.
        db::set_setting(&conn, limit_key(OR), "lots").unwrap();
        assert_eq!(read_setting(&conn, OR).unwrap(), Setting::Unreadable);
        db::set_setting(&conn, limit_key(OR), "0").unwrap();
        assert_eq!(read_setting(&conn, OR).unwrap(), Setting::Unreadable);
    }

    /// The day starts at the zone's midnight, on a day the clocks change as much as any other.
    #[test]
    fn the_day_starts_at_the_zones_midnight_even_across_a_clock_change() {
        // New York falls back on 01-11-2026 (a 25 hour day) and springs forward on 08-03-2026 (23).
        let ny = tz("America/New_York");
        let fall = day_bounds(ny, at(2026, 11, 1, 18, 0));
        assert_eq!(fall.since, "2026-11-01 04:00:00");
        assert_eq!(fall.resumes_at, at(2026, 11, 2, 5, 0));
        assert_eq!(
            (fall.resumes_at - at(2026, 11, 1, 4, 0)).num_hours(),
            25,
            "the day the clocks fall back is 25 hours long"
        );
        let spring = day_bounds(ny, at(2026, 3, 8, 18, 0));
        assert_eq!(spring.since, "2026-03-08 05:00:00");
        assert_eq!(spring.resumes_at, at(2026, 3, 9, 4, 0));
        assert_eq!(
            (spring.resumes_at - at(2026, 3, 8, 5, 0)).num_hours(),
            23,
            "the day the clocks spring forward is 23 hours long"
        );
    }

    #[test]
    fn midnight_itself_belongs_to_the_new_day() {
        let tokyo = tz("Asia/Tokyo");
        // 00:00 on the 27th in Tokyo is 15:00 UTC on the 26th.
        let just_before = day_bounds(tokyo, at(2026, 11, 26, 14, 59));
        let exactly = day_bounds(tokyo, at(2026, 11, 26, 15, 0));
        assert_eq!(just_before.since, "2026-11-25 15:00:00");
        assert_eq!(exactly.since, "2026-11-26 15:00:00");
        assert_eq!(just_before.resumes_at, at(2026, 11, 26, 15, 0));
    }

    /// A zone change mid-day restarts "today" from the new zone's midnight (said to be harmless).
    #[test]
    fn a_zone_change_restarts_the_day_from_the_new_zones_midnight() {
        let now = at(2026, 11, 27, 5, 0);
        let utc = day_bounds(Tz::UTC, now);
        let brisbane = day_bounds(tz("Australia/Brisbane"), now);
        assert_eq!(utc.since, "2026-11-27 00:00:00");
        assert_eq!(brisbane.since, "2026-11-26 14:00:00");
        assert_ne!(utc.resumes_at, brisbane.resumes_at);
    }

    /// CONFIRMS the stored format on a real row (written by the one writer): the default
    /// `created_at` is `YYYY-MM-DDTHH:MM:SS.fffZ`, which is what the day sum must be compared in.
    #[test]
    fn a_real_rows_created_at_is_in_the_t_and_z_form() {
        let (_d, conn) = temp_db();
        let usage = Usage {
            prompt_tokens: Some(1),
            completion_tokens: Some(1),
            cost: Some(0.01),
            ..Default::default()
        };
        crate::commands::log_usage_as(&conn, "chat", Some("vendor/model"), &usage, &meta(OR), None);
        let stamp: String = conn
            .query_row("SELECT created_at FROM usage_log", [], |r| r.get(0))
            .unwrap();
        let b = stamp.as_bytes();
        assert_eq!(stamp.len(), 24, "{stamp}");
        assert_eq!(
            (b[10], b[13], b[16], b[19], b[23]),
            (b'T', b':', b':', b'.', b'Z')
        );
        // And the bound `day_start_utc_in` makes is in the other form, with a space.
        assert_eq!(day_bounds(Tz::UTC, Utc::now()).since.as_bytes()[10], b' ');
    }

    /// The test that matters (critique 2, M2): rows in the REAL stored format on both sides of a
    /// local midnight that is NOT UTC midnight. Compared as raw strings, `'T'` sorts after `' '`, so
    /// every row from the UTC date's start up to the local midnight would count as today.
    #[test]
    fn today_is_summed_in_the_stored_format_east_and_west_of_utc() {
        // UTC+10. Local midnight on the 27th is 14:00 UTC on the 26th.
        let (_d, conn) = temp_db();
        set_zone(&conn, "Australia/Brisbane");
        row(&conn, OR, Some(16.0), "2026-11-26T01:00:00.000Z"); // the 26th, 11:00 local: yesterday
        row(&conn, OR, Some(1.0), "2026-11-26T13:59:59.999Z"); // one ms before local midnight
        row(&conn, OR, Some(2.0), "2026-11-26T14:00:00.000Z"); // local midnight exactly: today
        row(&conn, OR, Some(4.0), "2026-11-26T20:00:00.000Z");
        row(&conn, OR, Some(8.0), "2026-11-27T00:00:00.000Z");
        let now = at(2026, 11, 27, 5, 0);
        let today = read_today(&conn, OR, now).unwrap();
        assert_eq!(today.spent, 14.0);

        // UTC-5. Local midnight on the 27th is 05:00 UTC on the 27th.
        let (_d2, west) = temp_db();
        set_zone(&west, "America/Bogota");
        row(&west, OR, Some(1.0), "2026-11-27T04:59:59.999Z"); // 23:59 local on the 26th: yesterday
        row(&west, OR, Some(2.0), "2026-11-27T05:00:00.000Z");
        row(&west, OR, Some(4.0), "2026-11-27T19:00:00.000Z");
        let now = at(2026, 11, 27, 20, 0);
        assert_eq!(read_today(&west, OR, now).unwrap().spent, 6.0);
    }

    /// The sum is one provider's: another company's rows, a local row and an old row (no recorded
    /// company) are not counted against it.
    #[test]
    fn only_the_providers_own_rows_count() {
        let (_d, conn) = temp_db();
        let stamp = "2026-11-27T10:00:00.000Z";
        row(&conn, OR, Some(1.0), stamp);
        row(&conn, CloudProvider::Anthropic, Some(100.0), stamp);
        row(&conn, CloudProvider::OpenAi, Some(100.0), stamp);
        // A row from before PM recorded the company.
        row(&conn, OR, Some(100.0), stamp);
        conn.execute(
            "UPDATE usage_log SET served_provider = NULL WHERE id = (SELECT MAX(id) FROM usage_log)",
            [],
        )
        .unwrap();
        let now = at(2026, 11, 27, 12, 0);
        assert_eq!(read_today(&conn, OR, now).unwrap().spent, 1.0);
        assert_eq!(
            read_today(&conn, CloudProvider::Anthropic, now)
                .unwrap()
                .spent,
            100.0
        );
    }

    /// A row PM cannot price counts as zero and says so: the sum is a floor.
    #[test]
    fn a_row_with_no_price_counts_as_zero_and_is_counted_as_unpriced() {
        let (_d, conn) = temp_db();
        let stamp = "2026-11-27T10:00:00.000Z";
        row(&conn, CloudProvider::Anthropic, Some(2.5), stamp);
        row(&conn, CloudProvider::Anthropic, None, stamp);
        let today = read_today(&conn, CloudProvider::Anthropic, at(2026, 11, 27, 12, 0)).unwrap();
        assert_eq!(today.spent, 2.5);
        assert_eq!(today.unpriced, 1);
    }

    fn reached_text(r: &Result<()>) -> String {
        r.as_ref().unwrap_err().to_string()
    }

    #[test]
    fn a_provider_with_no_limit_is_never_read_and_never_refused() {
        let (_d, conn) = temp_db();
        // No usage_log at all: a limit that is off must not even look.
        conn.execute_batch("DROP TABLE usage_log").unwrap();
        assert!(check(&conn, OR, at(2026, 11, 27, 12, 0)).is_ok());
    }

    #[test]
    fn a_limit_refuses_at_the_limit_with_the_typed_error_and_lifts_when_raised() {
        let (_d, conn) = temp_db();
        write(&conn, OR, Some(5.0)).unwrap();
        let now = at(2026, 11, 27, 12, 0);
        row(&conn, OR, Some(4.0), "2026-11-27T09:00:00.000Z");
        assert!(check(&conn, OR, now).is_ok(), "under the limit");
        row(&conn, OR, Some(1.12), "2026-11-27T10:00:00.000Z");
        let refused = check(&conn, OR, now);
        let e = refused.as_ref().unwrap_err();
        let f = e.provider_failure().expect("the typed error");
        assert_eq!(f.class, FailureClass::LimitReached);
        assert_eq!(f.billing, Billing::NeverLeft);
        assert_eq!(f.provider, OR);
        assert!(reached_text(&refused).starts_with("OpenRouter is paused: PM has logged at least $5.12 today, over your daily limit of $5.00."));
        assert!(reached_text(&refused).contains("won't send this reply to another company"));
        // Remembered: a second ask is the same refusal.
        assert!(check(&conn, OR, now).is_err());
        // Raising the limit lifts it at once; so does clearing it.
        write(&conn, OR, Some(10.0)).unwrap();
        assert!(check(&conn, OR, now).is_ok());
        write(&conn, OR, Some(5.0)).unwrap();
        assert!(check(&conn, OR, now).is_err());
        write(&conn, OR, None).unwrap();
        assert!(check(&conn, OR, now).is_ok());
    }

    /// Once tripped, later calls do not sum the log again; and the pause ends at the next midnight.
    #[test]
    fn a_tripped_provider_is_remembered_until_the_next_midnight() {
        let (_d, conn) = temp_db();
        write(&conn, OR, Some(5.0)).unwrap();
        row(&conn, OR, Some(6.0), "2026-11-27T09:00:00.000Z");
        let now = at(2026, 11, 27, 12, 0);
        assert!(check(&conn, OR, now).is_err());
        // Poison the log: an answer from it would now be "unreadable", so the original refusal
        // coming back proves the sum was not read again.
        conn.execute_batch("DROP TABLE usage_log").unwrap();
        let again = check(&conn, OR, now);
        assert!(
            reached_text(&again).contains("PM has logged at least $6.00"),
            "{again:?}"
        );
        // The next day it is asked of the log again (here a poisoned one: fail closed).
        let tomorrow = at(2026, 11, 28, 0, 1);
        assert!(reached_text(&check(&conn, OR, tomorrow)).contains("couldn't read today's usage"));
    }

    /// A limit that is on and cannot be read fails CLOSED, with the sentence that says what to do.
    #[test]
    fn a_limit_that_cannot_be_read_fails_closed() {
        let (_d, conn) = temp_db();
        write(&conn, CloudProvider::Gemini, Some(5.0)).unwrap();
        conn.execute_batch("DROP TABLE usage_log").unwrap();
        let r = check(&conn, CloudProvider::Gemini, at(2026, 11, 27, 12, 0));
        let f = r.as_ref().unwrap_err().provider_failure().unwrap();
        assert_eq!(f.class, FailureClass::LimitReached);
        assert_eq!(f.billing, Billing::NeverLeft);
        assert_eq!(
            reached_text(&r),
            "PM couldn't read today's usage, so it paused Google Gemini to be safe. Check Usage, or turn the limit off."
        );
        // A junk limit row is the same refusal.
        let (_d2, conn2) = temp_db();
        db::set_setting(&conn2, limit_key(CloudProvider::OpenAi), "lots").unwrap();
        assert!(check(&conn2, CloudProvider::OpenAi, at(2026, 11, 27, 12, 0)).is_err());
        // And a provider with no limit, in the same poisoned store, is not caught by it.
        assert!(check(&conn, CloudProvider::Anthropic, at(2026, 11, 27, 12, 0)).is_ok());
    }

    #[test]
    fn the_refusal_names_where_to_raise_the_limit_for_each_provider() {
        let or = refusal_sentence(OR, 5.12, 5.0);
        assert!(
            or.contains("under your OpenRouter key in AI & Models"),
            "{or}"
        );
        let claude = refusal_sentence(CloudProvider::Anthropic, 5.12, 5.0);
        assert!(claude.starts_with("Anthropic is paused:"), "{claude}");
        assert!(claude.contains("in Your own keys"), "{claude}");
        assert!(refusal_sentence(CloudProvider::Gemini, 0.004, 0.01).contains("$0.0040 today"));
    }

    #[test]
    fn the_view_for_each_state() {
        let (_d, conn) = temp_db();
        let now = at(2026, 11, 27, 12, 0);
        // Off.
        let off = view(&conn, OR, false, now);
        assert_eq!(off.limit_usd, None);
        assert!(!off.reached && !off.unreadable && !off.free_key);
        assert_eq!(off.suggested_usd, 5.0);
        // Within.
        write(&conn, OR, Some(5.0)).unwrap();
        row(&conn, OR, Some(1.2), "2026-11-27T09:00:00.000Z");
        let within = view(&conn, OR, false, now);
        assert_eq!(within.limit_usd, Some(5.0));
        assert_eq!(within.spent_today_usd, 1.2);
        assert!(!within.reached);
        assert_eq!(within.resumes_at, at(2026, 11, 28, 0, 0).to_rfc3339());
        // Unpriced rows say the figure is a floor.
        row(&conn, OR, None, "2026-11-27T09:30:00.000Z");
        // (an OpenRouter row with tokens and no price: the model has no cached price)
        assert_eq!(view(&conn, OR, false, now).unpriced_today, 1);
        // Reached.
        row(&conn, OR, Some(4.0), "2026-11-27T10:00:00.000Z");
        let reached = view(&conn, OR, false, now);
        assert!(reached.reached && !reached.unreadable);
        // A proven-free key is reported as such; the limit is still honest about its own sum.
        assert!(view(&conn, OR, true, now).free_key);
        // Unreadable: the pause says so, and keeps the limit it could read.
        conn.execute_batch("DROP TABLE usage_log").unwrap();
        let broken = view(&conn, OR, false, now);
        assert!(broken.reached && broken.unreadable);
        assert_eq!(broken.limit_usd, Some(5.0));
    }

    #[test]
    fn the_view_serialises_for_the_page() {
        let (_d, conn) = temp_db();
        let v = serde_json::to_value(view(&conn, OR, false, at(2026, 11, 27, 12, 0))).unwrap();
        for field in [
            "limit_usd",
            "spent_today_usd",
            "unpriced_today",
            "reached",
            "unreadable",
            "resumes_at",
            "free_key",
            "suggested_usd",
        ] {
            assert!(v.get(field).is_some(), "{field}");
        }
    }

    /// A pause decided in one store is never read as another store's.
    #[test]
    fn a_pause_in_one_store_is_not_read_in_another() {
        let (_a, one) = temp_db();
        let (_b, two) = temp_db();
        for conn in [&one, &two] {
            write(conn, OR, Some(5.0)).unwrap();
        }
        row(&one, OR, Some(9.0), "2026-11-27T09:00:00.000Z");
        let now = at(2026, 11, 27, 12, 0);
        assert!(check(&one, OR, now).is_err());
        assert!(check(&two, OR, now).is_ok());
    }

    /// The gate never awaits and never holds the guard past its own return: it is a plain function
    /// whose only connection is the one it opens and drops, and nothing in this file `.await`s.
    #[test]
    fn the_check_never_awaits_so_no_guard_is_held_across_one() {
        let src = include_str!("daily_limit.rs");
        let live = src.split("#[cfg(test)]\nmod tests {").next().unwrap();
        assert!(
            !live.contains(&format!(".{}", "await")),
            "the limit's check must not await"
        );
        assert!(live.contains("pub(crate) fn gate(p: CloudProvider) -> Result<()>"));
        assert!(!live.contains("async fn"));
    }

    /// A store that cannot be opened is the caller's ordinary error, not a pause: until a limit is
    /// known to be ON nothing here may refuse, so a person with no limit never meets limit wording.
    /// A scan, because `gate` reaches the store through the app, which a unit test has none of.
    #[test]
    fn a_store_that_will_not_open_is_the_plain_error_not_a_pause() {
        let src = include_str!("daily_limit.rs");
        let live = src.split("#[cfg(test)]\nmod tests {").next().unwrap();
        let gate = live
            .split("pub(crate) fn gate(p: CloudProvider) -> Result<()> {")
            .nth(1)
            .and_then(|b| b.split("\n}\n").next())
            .expect("gate");
        assert!(
            gate.contains("state.conn()?"),
            "gate must pass the store error on"
        );
        assert!(
            !gate.contains("limit_failure("),
            "gate refuses only inside `check`"
        );
        let paused = live
            .split("pub(crate) fn is_paused(")
            .nth(1)
            .and_then(|b| b.split("\n}\n").next())
            .expect("is_paused");
        assert!(
            paused.contains("return false"),
            "no store, no known limit, so not paused"
        );
    }

    /// With no app installed `gate` answers Ok (a unit test has no store), so the brake is only as
    /// real as the one `install` call at start. Nothing else would notice that line going: every
    /// other test drives the gate through `test_gate`. A scan, because `setup` needs a window.
    #[test]
    fn the_app_is_handed_over_before_anything_can_call_a_model() {
        let lib = include_str!("lib.rs");
        let install = lib
            .find("daily_limit::install(")
            .expect("lib.rs must hand the app to the daily limit (gate is open without it)");
        let first_runtime = lib
            .find("EngineState::start_for(")
            .expect("the mod engine's start");
        assert!(
            install < first_runtime,
            "install the daily limit before the mod engine starts, which can make a model call"
        );
        // Called once, not in a comment: the line must be code.
        let line = lib[install..].lines().next().unwrap();
        assert!(line.trim_end().ends_with(");"), "{line}");
        let before = lib[..install].rsplit('\n').next().unwrap_or("");
        assert!(
            !before.contains("//"),
            "the install call must not be commented out"
        );
    }
}
