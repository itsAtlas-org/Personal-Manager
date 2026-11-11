// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a mod may ask of PM, and how fast (Mods spec §1.5 "Rate limits", §1.9): build-time constants
//! in one place, which the Developer tab shows (slice 6), and the per-mod limiter that enforces the
//! rate and the concurrency. The page holds its own line in front of these (`modBridge.ts`): it
//! closes a frame's port when the frame floods, so a hostile frame can't even queue work here.
//!
//! The numbers are set for the two pilots with a wide margin, and for a hostile mod's worst case:
//! - **50 calls a second, bursts of 100, per mod.** The Map's busiest moment is its start (a list,
//!   the vectors, the theme, a handful of storage reads); the Pinboard's is a board of widgets each
//!   reading once. Both are a few dozen calls, well inside one burst. Sustained, 50/s is far above
//!   any UI's need, and still bounds what a mod can make PM's DB do.
//! - **16 calls in flight per mod.** Each call holds a blocking-pool thread for its DB step; tokio's
//!   pool is shared with all of PM's file and DB work, so one mod mustn't be able to take much of it.
//! - **1 MiB of arguments** (the page's cap too, `modBridge.ts`'s `MAX_ARGS_BYTES`; a test holds
//!   them equal). A storage value is the largest argument; anything bigger belongs in several keys.
//! - **8 MiB of answer** (JSON). `documents.list` over a large library (thousands of documents of a
//!   few hundred bytes each) is the largest JSON answer; vectors travel as binary (part 3), not here.
//! - **50 MiB of private storage per mod** (`mod_kv` and `mod_doc_data` together), counted as each
//!   row's value and key bytes plus a fixed per-row cost, so a million empty rows fill it too. The
//!   Map's coordinates for 5,000 documents are well under 1 MiB.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use super::error::{Code, ModError};
use crate::mods::ids::ModId;

/// Calls a mod may make per second, sustained.
pub const CALLS_PER_SECOND: u64 = 50;
/// Calls a mod may make at once after a quiet spell (the bucket's size).
pub const CALL_BURST: u64 = 100;
/// Calls of one mod running at once.
pub const MAX_IN_FLIGHT: usize = 16;
/// A call's arguments, as the JSON text the page sends, in bytes.
pub const MAX_ARGS_BYTES: usize = 1 << 20;
/// A call's JSON answer, in bytes.
pub const MAX_RESULT_BYTES: usize = 8 << 20;
/// One mod's private storage, in bytes ([`row_cost`] per row).
pub const STORAGE_QUOTA_BYTES: u64 = 50 << 20;
/// What each stored row costs beyond its key and value: roughly SQLite's own overhead for a row and
/// its index entry, so a flood of tiny rows meets the quota.
pub const ROW_OVERHEAD_BYTES: u64 = 64;

/// What one stored row counts for against the quota.
pub fn row_cost(key_bytes: u64, value_bytes: u64) -> u64 {
    key_bytes
        .saturating_add(value_bytes)
        .saturating_add(ROW_OVERHEAD_BYTES)
}

/// Whether a write that replaces a row costing `old` (0 when there is none) with one costing `new`
/// fits, when everything else the mod stores costs `others`. A write that doesn't grow the mod's
/// storage always fits, so a mod over its quota (a quota lowered by an update) can still shrink.
pub fn quota_allows(others: u64, old: u64, new: u64, quota: u64) -> bool {
    new <= old || others.saturating_add(new) <= quota
}

/// A token bucket in millionths of a call, so refilling is exact integer arithmetic: `rate` calls
/// a second, at most `burst` saved up. Pure: the caller says what time it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenBucket {
    micros: u64,
    last: Instant,
    rate: u64,
    burst: u64,
}

const MICRO: u64 = 1_000_000;

impl TokenBucket {
    /// A full bucket.
    pub fn new(rate: u64, burst: u64, now: Instant) -> Self {
        Self {
            micros: burst.saturating_mul(MICRO),
            last: now,
            rate,
            burst,
        }
    }

    /// Take one call's token at `now`, refilling for the time since the last take. A clock that
    /// went backwards (never, with `Instant`) refills nothing.
    pub fn take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last);
        self.last = self.last.max(now);
        // rate calls per second = rate micro-calls per microsecond.
        let earned = u64::try_from(elapsed.as_micros())
            .unwrap_or(u64::MAX)
            .saturating_mul(self.rate);
        self.micros = self
            .micros
            .saturating_add(earned)
            .min(self.burst.saturating_mul(MICRO));
        if self.micros >= MICRO {
            self.micros -= MICRO;
            true
        } else {
            false
        }
    }
}

#[derive(Debug)]
struct PerMod {
    bucket: TokenBucket,
    in_flight: usize,
}

/// Every mod's bucket and in-flight count, for the life of the process (managed by `setup()`). A
/// mod's entry outlives its uninstall; it is a few bytes, and a reinstall in the same run starting
/// from a spent bucket is the safe side.
#[derive(Debug, Default)]
pub struct Calls {
    per_mod: Arc<Mutex<HashMap<ModId, PerMod>>>,
}

/// One admitted call: it counts as in flight until dropped.
#[derive(Debug)]
pub struct Admission {
    per_mod: Arc<Mutex<HashMap<ModId, PerMod>>>,
    id: ModId,
}

impl Drop for Admission {
    fn drop(&mut self) {
        if let Some(m) = lock(&self.per_mod).get_mut(&self.id) {
            m.in_flight = m.in_flight.saturating_sub(1);
        }
    }
}

/// The counts are plain integers changed in one step each, so a poisoned lock still holds sound
/// state and is read through.
fn lock(m: &Mutex<HashMap<ModId, PerMod>>) -> MutexGuard<'_, HashMap<ModId, PerMod>> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Calls {
    /// Admit one call of `id` at `now`, or refuse it: too many of its calls are running, or its
    /// bucket is empty. A call refused for concurrency spends no token.
    pub fn admit(&self, id: &ModId, now: Instant) -> Result<Admission, ModError> {
        self.admit_with(id, now, CALLS_PER_SECOND, CALL_BURST, MAX_IN_FLIGHT)
    }

    fn admit_with(
        &self,
        id: &ModId,
        now: Instant,
        rate: u64,
        burst: u64,
        max_in_flight: usize,
    ) -> Result<Admission, ModError> {
        let mut all = lock(&self.per_mod);
        let m = all.entry(id.clone()).or_insert_with(|| PerMod {
            bucket: TokenBucket::new(rate, burst, now),
            in_flight: 0,
        });
        if m.in_flight >= max_in_flight {
            return Err(ModError::new(
                Code::RateLimited,
                "Too many of this mod's calls are running at once.",
            ));
        }
        if !m.bucket.take(now) {
            return Err(ModError::new(
                Code::RateLimited,
                "This mod is calling PM too fast.",
            ));
        }
        m.in_flight += 1;
        Ok(Admission {
            per_mod: Arc::clone(&self.per_mod),
            id: id.clone(),
        })
    }

    #[cfg(test)]
    pub fn in_flight(&self, id: &ModId) -> usize {
        lock(&self.per_mod).get(id).map_or(0, |m| m.in_flight)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_bucket_spends_its_burst_then_refills_at_its_rate_and_never_past_its_size() {
        let t0 = Instant::now();
        let mut b = TokenBucket::new(50, 100, t0);
        for i in 0..100 {
            assert!(b.take(t0), "token {i}");
        }
        assert!(!b.take(t0), "the 101st at once");
        // 50 a second: one every 20 ms, none before.
        assert!(!b.take(t0 + Duration::from_millis(19)));
        assert!(b.take(t0 + Duration::from_millis(20)));
        assert!(!b.take(t0 + Duration::from_millis(20)));
        // A second's rest earns exactly 50 more.
        let t1 = t0 + Duration::from_millis(1020);
        for i in 0..50 {
            assert!(b.take(t1), "token {i} after a second");
        }
        assert!(!b.take(t1));
        // A long rest refills to the burst, never past it.
        let t2 = t1 + Duration::from_secs(3600);
        for i in 0..100 {
            assert!(b.take(t2), "token {i} after an hour");
        }
        assert!(!b.take(t2));
        // Time going backwards earns nothing and spends nothing extra.
        assert!(!b.take(t0));
        // Sub-token remainders carry over: 10 ms + 10 ms is one token at 50/s.
        let mut c = TokenBucket::new(50, 1, t0);
        assert!(c.take(t0));
        assert!(!c.take(t0 + Duration::from_millis(10)));
        assert!(c.take(t0 + Duration::from_millis(20)));
    }

    #[test]
    fn the_sustained_rate_is_held_over_a_long_run() {
        // Calls every 5 ms for 10 s (2,000 asks): the burst plus 50 a second get through.
        let t0 = Instant::now();
        let mut b = TokenBucket::new(CALLS_PER_SECOND, CALL_BURST, t0);
        let admitted = (0..2000)
            .filter(|i| b.take(t0 + Duration::from_millis(5 * i)))
            .count() as u64;
        let ceiling = CALL_BURST + CALLS_PER_SECOND * 10;
        assert!(
            admitted <= ceiling && admitted >= ceiling - 2,
            "{admitted} of a ceiling of {ceiling}"
        );
    }

    #[test]
    fn the_quota_counts_rows_and_lets_a_full_mod_shrink() {
        assert_eq!(row_cost(3, 10), 77);
        assert_eq!(row_cost(u64::MAX, 1), u64::MAX);
        let q = 1000;
        assert!(quota_allows(0, 0, 1000, q));
        assert!(!quota_allows(0, 0, 1001, q));
        assert!(quota_allows(900, 0, 100, q));
        assert!(!quota_allows(901, 0, 100, q));
        // Replacing a row: only the others and the new size count.
        assert!(quota_allows(500, 400, 500, q));
        assert!(!quota_allows(500, 400, 501, q));
        // Over quota already: shrinking or keeping the size is allowed, growing isn't.
        assert!(quota_allows(5000, 300, 200, q));
        assert!(quota_allows(5000, 300, 300, q));
        assert!(!quota_allows(5000, 300, 301, q));
        assert!(!quota_allows(u64::MAX, 0, 1, q));
    }

    fn id(s: &str) -> ModId {
        ModId::parse(s).unwrap()
    }

    #[test]
    fn a_mod_is_held_to_its_concurrency_and_its_rate_and_another_mod_is_not() {
        let calls = Calls::default();
        let t0 = Instant::now();
        let a = id("acme.a");
        let b = id("acme.b");
        let mut held = Vec::new();
        for _ in 0..3 {
            held.push(calls.admit_with(&a, t0, 50, 100, 3).unwrap());
        }
        let refused = calls.admit_with(&a, t0, 50, 100, 3).unwrap_err();
        assert_eq!(refused.code, Code::RateLimited);
        assert_eq!(calls.in_flight(&a), 3);
        // Another mod has its own count and bucket.
        let other = calls.admit_with(&b, t0, 50, 100, 3).unwrap();
        // One finishing frees a place, and the refused call spent no token.
        held.pop();
        assert_eq!(calls.in_flight(&a), 2);
        held.push(calls.admit_with(&a, t0, 50, 100, 3).unwrap());
        drop(held);
        drop(other);
        assert_eq!(calls.in_flight(&a), 0);
        assert_eq!(calls.in_flight(&b), 0);

        // The bucket: 4 admitted (3 + the one above), so 96 remain of 100 at t0.
        for i in 0..96 {
            drop(
                calls
                    .admit_with(&a, t0, 50, 100, 3)
                    .unwrap_or_else(|e| panic!("{i}: {e}")),
            );
        }
        let empty = calls.admit_with(&a, t0, 50, 100, 3).unwrap_err();
        assert_eq!(empty.code, Code::RateLimited);
        assert!(calls
            .admit_with(&a, t0 + Duration::from_millis(20), 50, 100, 3)
            .is_ok());
        assert!(calls.admit_with(&b, t0, 50, 100, 3).is_ok());
    }

    #[test]
    fn the_shipped_numbers_are_the_ones_the_spec_and_the_page_assume() {
        const {
            assert!(MAX_ARGS_BYTES == 1_048_576);
            assert!(CALL_BURST >= CALLS_PER_SECOND);
            assert!(MAX_RESULT_BYTES > MAX_ARGS_BYTES);
        }
        // The page's flood line sits past this bucket (`modBridge.ts`).
        let page = include_str!("../../../../src/components/mods/modBridge.ts");
        let number = |name: &str| -> u64 {
            let line = page
                .lines()
                .find(|l| l.starts_with(&format!("export const {name} = ")))
                .unwrap_or_else(|| panic!("modBridge.ts has no {name}"));
            line.split('=')
                .nth(1)
                .unwrap()
                .trim()
                .trim_end_matches(';')
                .replace('_', "")
                .parse()
                .unwrap_or_else(|_| panic!("{name} isn't a plain number: {line}"))
        };
        assert_eq!(number("MAX_ARGS_BYTES"), MAX_ARGS_BYTES as u64);
        assert_eq!(number("FLOOD_CALLS_PER_SECOND"), CALLS_PER_SECOND);
        assert!(
            number("FLOOD_BURST") > CALL_BURST,
            "the page must flood past the bucket"
        );
        assert!(number("MAX_IN_FLIGHT") > MAX_IN_FLIGHT as u64);
    }
}
