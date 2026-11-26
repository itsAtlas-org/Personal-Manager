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
//!   few hundred bytes each) is the largest JSON answer, paged under three quarters of it; vectors
//!   travel as binary, up to **20 MiB**, not here.
//! - **5,000 documents a call** (the Map's largest node cap): a page of the list, the ids a call
//!   names, the entries of one batched write.
//! - **The experimental calls (PR B)** have limits of their own, below: one outer deadline
//!   ([`CALL_TIMEOUT`]) for any call that waits on something outside PM, a bucket and a concurrency
//!   cap per [`Family`] (a call of the AI or network family holds its admission for as long as the
//!   model or the website takes, so the generic 16 at once would let one mod hold PM's shared local
//!   model slot or its sockets), and the payload caps of `ai.complete`, `net.fetch`, `notify` and
//!   `secrets.set`. They were chosen before any mod uses a call, so each is a first guess to tune
//!   after the live tests; every one errs toward refusing.
//! - **PR C's families (K1)** stay in the same place: the gifts (`tasks.add`, `tasks.importChecklist`,
//!   `milestones.add`), the document embed (`documents.saveNote`) and the links (`shell.openLink`)
//!   each have a bucket and a concurrency cap of their own, a mod's gifts are held to
//!   [`MAX_GIFTS_PER_MOD`] rows in `mod_ledger`, and a question PM asks the person ([`CONFIRM_TTL`])
//!   runs out after a minute. They are first guesses for the live tests, and err toward refusing.
//! - **50 MiB of private storage per mod** (`mod_kv` and `mod_doc_data` together), counted as each
//!   row's value and key bytes plus a fixed per-row cost, so a million empty rows fill it too. The
//!   Map's coordinates for 5,000 documents are well under 1 MiB.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

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
/// A binary answer (`embeddings.documentVectors`), in bytes: 20 MiB, which holds the largest the
/// Host API sizes for (5,000 documents of 1,024 dimensions, 19.6 MiB with its header and ids).
pub const MAX_BINARY_RESULT_BYTES: usize = 20 << 20;
/// Documents one call may name or return: the Map's largest node cap. A page of `documents.list`, the
/// ids a vector or storage call takes, and the entries of one `storage.doc.set` are held to it.
pub const MAX_DOCS_PER_CALL: usize = 5000;
/// What one page of `documents.list` aims to stay under, in bytes of JSON: three quarters of the
/// answer cap, so a page ends early (with a cursor) rather than ever being refused as too large.
pub const LIST_PAGE_BUDGET_BYTES: usize = MAX_RESULT_BYTES / 4 * 3;
/// One mod's private storage, in bytes ([`row_cost`] per row).
pub const STORAGE_QUOTA_BYTES: u64 = 50 << 20;
/// What each stored row costs beyond its key and value: roughly SQLite's own overhead for a row and
/// its index entry, so a flood of tiny rows meets the quota.
pub const ROW_OVERHEAD_BYTES: u64 = 64;
/// Storage writes per second, sustained, that GROW what a mod stores. Only a growing write must add
/// up every row the mod has (a scan under the DB guard, 10 to 60 ms at the quota), so the call
/// bucket, which counts calls and not what they cost, is not what bounds it: a full mod could
/// otherwise ask for a scan fifty times a second. A write that doesn't grow never scans.
pub const STORAGE_GROWTH_PER_SECOND: u64 = 4;
/// Growing writes a mod may make at once after a quiet spell. The Map's first layout of a large
/// library is a few dozen batches.
pub const STORAGE_GROWTH_BURST: u64 = 40;

// The experimental calls (plan decisions D-15, D-20, D-22). Each is shown in the Developer tab.

/// The outer deadline of a call that waits on something outside PM (a model, a website). The AI
/// route's own worst case is 20 s waiting for the shared local slot plus a 120 s cold load of the
/// model, then the answer itself; past this the call is stopped and the mod is told
/// `MOD-3005`. Only cancellable async work can be stopped by it: a `spawn_blocking` step
/// that has begun runs to its end, which is why a handler commits nothing after a timeout (it
/// checks [`super::still_current`] first) and keeps its blocking steps short.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(180);

/// `ai.complete` calls of one mod running at once. A call holds PM's one local-model slot while it
/// runs (`local_slot.rs`), so a mod may not queue many ahead of PM's own background jobs.
pub const AI_MAX_IN_FLIGHT: usize = 2;
/// `ai.complete` calls a mod may make per minute, sustained. Every call spends the mod's money cap
/// as well; this bounds how fast a mod can reach it, and how fast it can fill the usage log.
pub const AI_CALLS_PER_MINUTE: u64 = 20;
/// `ai.complete` calls a mod may make at once after a quiet spell.
pub const AI_CALL_BURST: u64 = 5;
/// The messages of one `ai.complete`, as UTF-8 bytes of content. The reservation is priced from this
/// (a token covers at least one byte of text, so the bytes are a provable upper bound on the tokens
/// without a tokenizer in Rust), so a larger prompt is a larger worst-case price.
pub const MAX_AI_PROMPT_BYTES: usize = 64 << 10;
/// Messages in one `ai.complete`.
pub const MAX_AI_MESSAGES: usize = 32;
/// The most a mod may ask a model to write, in tokens (`maxTokens` is capped to it, never refused
/// for being over it: a smaller answer is the safe side).
pub const MAX_AI_OUTPUT_TOKENS: u32 = 4096;

/// `net.fetch` calls of one mod running at once: the call holds its admission for the whole fetch,
/// and `getaddrinfo` cannot be cancelled once begun.
pub const NET_MAX_IN_FLIGHT: usize = 4;
/// `net.fetch` calls a mod may make per second, sustained.
pub const NET_FETCHES_PER_SECOND: u64 = 5;
/// `net.fetch` calls a mod may make at once after a quiet spell.
pub const NET_FETCH_BURST: u64 = 20;
/// A fetch's request body, in bytes.
pub const MAX_FETCH_REQUEST_BYTES: usize = 256 << 10;
/// A fetch's response body, in bytes, read to this and no further. Sent to the frame as text or
/// base64. As base64 it is under [`MAX_RESULT_BYTES`]; as text it is sent only when its JSON form
/// is within [`MAX_FETCH_TEXT_JSON_BYTES`] (escaping can grow text up to six times), else as base64,
/// so a body within this cap always fits (a test holds both).
pub const MAX_FETCH_RESPONSE_BYTES: usize = 4 << 20;
/// The most a fetch's text body may weigh as a JSON string (quotes, backslashes and control
/// characters escaped) before it is sent as base64 instead: above the base64 form of the largest
/// body, below the answer cap with the headers' room.
pub const MAX_FETCH_TEXT_JSON_BYTES: usize = 6 << 20;
/// How long a fetch may take to connect, per hop.
pub const FETCH_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a whole fetch may take, redirects and body included.
pub const FETCH_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
/// Redirects a fetch follows, each one re-checked as a new request.
pub const MAX_FETCH_REDIRECTS: usize = 5;

/// A `notify` text, in characters (after PM cleans it like any mod text).
pub const NOTIFY_MAX_CHARS: usize = 140;
/// Notices a mod may show per day, by the user's local date.
pub const NOTIFY_PER_DAY: u32 = 5;
/// Times a mod may ask for a key in one session (`secrets.set`).
pub const SECRET_ASKS_PER_SESSION: u32 = 3;

// PR C's families (K1). The rows that use them arrive in K2; the numbers are the plan's.

/// Gift calls (`tasks.add`, `tasks.importChecklist`, `milestones.add`) of one mod running at once.
/// Each writes the user's own lists, so a mod may not queue many.
pub const GIFT_MAX_IN_FLIGHT: usize = 2;
/// Gift calls a mod may make per second, sustained.
pub const GIFTS_PER_SECOND: u64 = 10;
/// Gift calls a mod may make at once after a quiet spell (the Pinboard's first import of a checklist
/// is a few dozen).
pub const GIFT_BURST: u64 = 50;
/// The most tasks and milestones one mod may ever have made (its rows in `mod_ledger`, counted
/// inside the transaction that adds one; an uninstall keeps the rows, so a reinstall doesn't reset
/// it). Past it the call is refused `MOD-3003`.
pub const MAX_GIFTS_PER_MOD: u64 = 10_000;

/// Embed-family calls (`documents.saveNote`, `documents.search`) of one mod running at once: each holds a
/// CPU-heavy embed.
pub const EMBED_MAX_IN_FLIGHT: usize = 1;
/// Embed-family calls a mod may make per minute, sustained.
pub const EMBEDS_PER_MINUTE: u64 = 6;
/// Embed-family calls a mod may make at once after a quiet spell.
pub const EMBED_BURST: u64 = 10;

/// `shell.openLink` calls of one mod running at once (a launcher may block on the desktop).
pub const LINKS_MAX_IN_FLIGHT: usize = 1;
/// `shell.openLink` calls a mod may make per minute, sustained.
pub const LINKS_PER_MINUTE: u64 = 6;
/// `shell.openLink` calls a mod may make at once after a quiet spell.
pub const LINK_BURST: u64 = 3;

// The caps of PR C's calls (K2), each held by its handler as a refusal, never a trim.

/// Tasks (or milestones) one page of a list returns at most, and ids one `tasks.reorder` names.
pub const MAX_TASKS_PER_CALL: usize = 5000;
/// Lines one `tasks.importChecklist` takes.
pub const MAX_CHECKLIST_ITEMS: usize = 200;
/// Project names `projects.list` returns before it says it was cut short.
pub const MAX_PROJECTS_LISTED: usize = 5000;
/// A note's key, in a mod's own namespace (`mod-<id>.<key>.md` with an id of up to 159 characters
/// stays under a file name's 255).
pub const NOTE_KEY_MAX_OWN: usize = 64;
/// A note's key through the Pinboard's legacy mapping (`note-<key>.md`): no tighter than the board's
/// own widget ids allow, so a board whose ids are older or imported still saves after the move.
pub const NOTE_KEY_MAX_LEGACY: usize = 200;
/// Keys one `documents.statusOf` asks about.
pub const MAX_NOTE_KEYS_PER_CALL: usize = 500;
/// A note's title, in characters: one plain line, which lands in the vault file's front matter.
pub const NOTE_TITLE_MAX_CHARS: usize = 200;
/// A note's body, in bytes.
pub const NOTE_BODY_MAX_BYTES: usize = 512 << 10;
/// An address `shell.openLink` takes, in bytes.
pub const LINK_MAX_BYTES: usize = 2048;
/// How much of an address PM's question shows before it says how many characters more there are.
pub const LINK_SHOWN_CHARS: usize = 300;
/// Entries one `calendar.overlay.set` publishes: the whole set of the mod.
pub const OVERLAY_MAX_ENTRIES: usize = 2000;
/// An overlay entry's id, in characters. The Pinboard's are `<widgetId>:<itemId>`, two random UUIDs
/// (73), and the Calendar's event ids, hidden sets and Runway key on them, so a mod can't shorten them.
pub const OVERLAY_ID_MAX_CHARS: usize = 200;
/// An overlay entry's label, in characters.
pub const OVERLAY_LABEL_MAX_CHARS: usize = 140;
/// The first and last year an overlay entry may fall in.
pub const OVERLAY_YEARS: std::ops::RangeInclusive<i32> = 1900..=2200;

/// How long a question PM asks the person for a mod's call (`mods::confirm`) stays answerable. Past
/// it the answer does nothing and the call's token is gone.
pub const CONFIRM_TTL: Duration = Duration::from_secs(60);

/// The calls that are counted against a bucket and a cap of their own, as well as the generic
/// ones (every call is admitted by [`Calls::admit`] first). A row names its family in the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    /// Everything else: only the generic limits.
    General,
    /// `ai.complete`.
    Ai,
    /// `net.fetch`.
    Net,
    /// The calls that add a task or a milestone to the user's lists (K1: `tasks.add`,
    /// `tasks.importChecklist`, `milestones.add`).
    Gifts,
    /// The calls that hold a CPU-heavy embed (`documents.saveNote`, `documents.search`).
    Embed,
    /// `shell.openLink`: it waits on the desktop's launcher.
    Links,
}

/// One family's concurrency cap and bucket, `rate` calls every `per_seconds` seconds.
struct FamilyLimit {
    max_in_flight: usize,
    rate: u64,
    per_seconds: u64,
    burst: u64,
    /// What a refusal says, for too many at once and for too fast.
    busy: &'static str,
    fast: &'static str,
}

impl Family {
    fn limit(self) -> Option<FamilyLimit> {
        match self {
            Self::General => None,
            Self::Ai => Some(FamilyLimit {
                max_in_flight: AI_MAX_IN_FLIGHT,
                rate: AI_CALLS_PER_MINUTE,
                per_seconds: 60,
                burst: AI_CALL_BURST,
                busy: "Too many of this mod's AI requests are running at once.",
                fast: "This mod is asking the AI model too often.",
            }),
            Self::Net => Some(FamilyLimit {
                max_in_flight: NET_MAX_IN_FLIGHT,
                rate: NET_FETCHES_PER_SECOND,
                per_seconds: 1,
                burst: NET_FETCH_BURST,
                busy: "Too many of this mod's web requests are running at once.",
                fast: "This mod is fetching from the web too fast.",
            }),
            Self::Gifts => Some(FamilyLimit {
                max_in_flight: GIFT_MAX_IN_FLIGHT,
                rate: GIFTS_PER_SECOND,
                per_seconds: 1,
                burst: GIFT_BURST,
                busy: "Too many of this mod's additions are running at once.",
                fast: "This mod is adding to your lists too fast.",
            }),
            Self::Embed => Some(FamilyLimit {
                max_in_flight: EMBED_MAX_IN_FLIGHT,
                rate: EMBEDS_PER_MINUTE,
                per_seconds: 60,
                burst: EMBED_BURST,
                busy: "This mod is already saving a document.",
                fast: "This mod is saving documents too often.",
            }),
            Self::Links => Some(FamilyLimit {
                max_in_flight: LINKS_MAX_IN_FLIGHT,
                rate: LINKS_PER_MINUTE,
                per_seconds: 60,
                burst: LINK_BURST,
                busy: "This mod is already opening a link.",
                fast: "This mod is opening links too often.",
            }),
        }
    }
}

// Events (spec §1.5 "Events"). A frame is told things as they happen, and a frame can be slow, so the
// pipe from Rust to a frame holds a bounded number of events nobody has acknowledged, whatever the
// frame does. The threat model is a mod that is slow or buggy, which these hold; a mod that lies in
// its acknowledgements can only fill its own port, and a mod can already burn its own frame's memory
// (spec §1.9, "Not enforceable").
//
// - **256 events outstanding per session** ([`EVENT_WINDOW`]). Past it an event is dropped and
//   counted, and the next one delivered says how many were missed so the frame can ask again
//   (`documents.list`). The Map's busiest burst is a big import: its documents arrive coalesced
//   ([`EVENT_COALESCE_MS`]) a few dozen to a page, so one burst is a handful of events, and a
//   frame that keeps up never meets the window.
// - **64 KiB an event** ([`MAX_EVENT_BYTES`]), so the worst case in flight for one session is 16 MiB.
//   A `documents.added` page ends under [`EVENT_DOCS_BUDGET_BYTES`], which leaves room for any one
//   document; an event that is nonetheless too large is dropped and counted like any other.
// - **Pending ids** ([`MAX_PENDING_IDS`]): the documents that landed or went within one coalescing
//   interval, held until it ends. Past the cap the rest are dropped and counted, never queued.

/// Events a session may have sent and not had acknowledged.
pub const EVENT_WINDOW: u64 = 256;
/// One event's payload, as JSON, in bytes.
pub const MAX_EVENT_BYTES: usize = 64 << 10;
/// What a `documents.added` event aims to stay under, in bytes of JSON: three quarters of the cap.
pub const EVENT_DOCS_BUDGET_BYTES: usize = MAX_EVENT_BYTES / 4 * 3;
/// Ids a `documents.deleted` event carries at most.
pub const MAX_IDS_PER_EVENT: usize = 2000;
/// How long a landing or a deletion waits for the next, so a burst is one event and not hundreds.
pub const EVENT_COALESCE_MS: u64 = 250;
/// Ids held at once between a landing or deletion and the end of its interval.
pub const MAX_PENDING_IDS: usize = 100_000;

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

/// A token bucket whose unit is a millionth of a call per second of the refill period, so
/// refilling is exact integer arithmetic (no remainder is ever dropped): `rate` calls every
/// `per_seconds` seconds (one second, unless made with [`TokenBucket::per`]), at most `burst` saved
/// up. One call costs `MICRO * per_seconds` units, and a microsecond earns `rate` of them. Pure: the
/// caller says what time it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenBucket {
    units: u64,
    last: Instant,
    rate: u64,
    /// What one call costs, in units.
    cost: u64,
    burst: u64,
}

const MICRO: u64 = 1_000_000;

impl TokenBucket {
    /// A full bucket refilling at `rate` calls a second.
    pub fn new(rate: u64, burst: u64, now: Instant) -> Self {
        Self::per(rate, 1, burst, now)
    }

    /// A full bucket refilling at `rate` calls every `per_seconds` seconds (at least one).
    pub fn per(rate: u64, per_seconds: u64, burst: u64, now: Instant) -> Self {
        let cost = MICRO.saturating_mul(per_seconds.max(1));
        Self {
            units: burst.saturating_mul(cost),
            last: now,
            rate,
            cost,
            burst,
        }
    }

    /// Take one call's token at `now`, refilling for the time since the last take. A clock that
    /// went backwards (never, with `Instant`) refills nothing.
    pub fn take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last);
        self.last = self.last.max(now);
        let earned = u64::try_from(elapsed.as_micros())
            .unwrap_or(u64::MAX)
            .saturating_mul(self.rate);
        self.units = self
            .units
            .saturating_add(earned)
            .min(self.burst.saturating_mul(self.cost));
        if self.units >= self.cost {
            self.units -= self.cost;
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
    /// Growing storage writes, which pay for a scan of everything the mod stores.
    growth: TokenBucket,
    ai: FamilyState,
    net: FamilyState,
    gifts: FamilyState,
    embed: FamilyState,
    links: FamilyState,
}

/// One family's bucket and in-flight count for one mod.
#[derive(Debug)]
struct FamilyState {
    bucket: TokenBucket,
    in_flight: usize,
}

impl FamilyState {
    fn new(family: Family, now: Instant) -> Self {
        // A family without limits is never asked; its bucket is the generic one, unused.
        let (rate, per, burst) = family
            .limit()
            .map_or((CALLS_PER_SECOND, 1, CALL_BURST), |l| {
                (l.rate, l.per_seconds, l.burst)
            });
        Self {
            bucket: TokenBucket::per(rate, per, burst, now),
            in_flight: 0,
        }
    }
}

impl PerMod {
    fn new(now: Instant, rate: u64, burst: u64) -> Self {
        Self {
            bucket: TokenBucket::new(rate, burst, now),
            in_flight: 0,
            growth: TokenBucket::new(STORAGE_GROWTH_PER_SECOND, STORAGE_GROWTH_BURST, now),
            ai: FamilyState::new(Family::Ai, now),
            net: FamilyState::new(Family::Net, now),
            gifts: FamilyState::new(Family::Gifts, now),
            embed: FamilyState::new(Family::Embed, now),
            links: FamilyState::new(Family::Links, now),
        }
    }

    /// The state of a family that has limits of its own.
    fn family_mut(&mut self, family: Family) -> Option<&mut FamilyState> {
        match family {
            Family::General => None,
            Family::Ai => Some(&mut self.ai),
            Family::Net => Some(&mut self.net),
            Family::Gifts => Some(&mut self.gifts),
            Family::Embed => Some(&mut self.embed),
            Family::Links => Some(&mut self.links),
        }
    }
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

/// One admitted call of a family with limits of its own: it counts as in flight in that family until
/// dropped. A [`Family::General`] call gets one that counts nothing.
#[derive(Debug)]
pub struct FamilyAdmission {
    per_mod: Arc<Mutex<HashMap<ModId, PerMod>>>,
    id: ModId,
    family: Family,
}

impl Drop for FamilyAdmission {
    fn drop(&mut self) {
        if let Some(s) = lock(&self.per_mod)
            .get_mut(&self.id)
            .and_then(|m| m.family_mut(self.family))
        {
            s.in_flight = s.in_flight.saturating_sub(1);
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
        let m = all
            .entry(id.clone())
            .or_insert_with(|| PerMod::new(now, rate, burst));
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

    /// Admit one call of `family` for `id` at `now`, on top of [`Calls::admit`]: the family's own
    /// concurrency cap and bucket. A call refused for concurrency spends no token. [`Family::General`]
    /// is always admitted (it has no limits beyond the generic ones).
    pub fn admit_family(
        &self,
        id: &ModId,
        family: Family,
        now: Instant,
    ) -> Result<FamilyAdmission, ModError> {
        self.admit_family_charging(id, family, now, true)
    }

    /// Admit the PERFORMING of a call the person has just said yes to: the family's concurrency cap,
    /// but no token from its bucket, because the call that asked the question already paid one. So a
    /// confirmed call cannot run past the cap on what is in flight, and is not charged twice.
    pub fn admit_family_answer(
        &self,
        id: &ModId,
        family: Family,
        now: Instant,
    ) -> Result<FamilyAdmission, ModError> {
        self.admit_family_charging(id, family, now, false)
    }

    fn admit_family_charging(
        &self,
        id: &ModId,
        family: Family,
        now: Instant,
        charge: bool,
    ) -> Result<FamilyAdmission, ModError> {
        // Built only once a slot is taken (a `FamilyAdmission` gives one back when dropped, and its
        // drop takes the lock this method holds). A general call counts nothing, and `family_mut`
        // is `None` for it, so its drop counts nothing either.
        let admission = || FamilyAdmission {
            per_mod: Arc::clone(&self.per_mod),
            id: id.clone(),
            family,
        };
        let Some(limit) = family.limit() else {
            return Ok(admission());
        };
        {
            let mut all = lock(&self.per_mod);
            let m = all
                .entry(id.clone())
                .or_insert_with(|| PerMod::new(now, CALLS_PER_SECOND, CALL_BURST));
            let state = m
                .family_mut(family)
                .expect("a family with limits has a state");
            if state.in_flight >= limit.max_in_flight {
                return Err(ModError::new(Code::RateLimited, limit.busy));
            }
            if charge && !state.bucket.take(now) {
                return Err(ModError::new(Code::RateLimited, limit.fast));
            }
            state.in_flight += 1;
        }
        Ok(admission())
    }

    /// Whether `id` may make one more storage write that grows what it stores, which costs a scan of
    /// everything it stores ([`STORAGE_GROWTH_PER_SECOND`]). Called under the DB guard, and takes
    /// only this limiter's own short lock (nothing takes the guard while holding it).
    pub fn allow_growth(&self, id: &ModId, now: Instant) -> Result<(), ModError> {
        let mut all = lock(&self.per_mod);
        let m = all
            .entry(id.clone())
            .or_insert_with(|| PerMod::new(now, CALLS_PER_SECOND, CALL_BURST));
        if m.growth.take(now) {
            Ok(())
        } else {
            Err(ModError::new(
                Code::RateLimited,
                "This mod is storing more data too fast.",
            ))
        }
    }

    #[cfg(test)]
    pub fn in_flight(&self, id: &ModId) -> usize {
        lock(&self.per_mod).get(id).map_or(0, |m| m.in_flight)
    }

    #[cfg(test)]
    pub fn family_in_flight(&self, id: &ModId, family: Family) -> usize {
        lock(&self.per_mod)
            .get_mut(id)
            .and_then(|m| m.family_mut(family))
            .map_or(0, |s| s.in_flight)
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
    fn growing_writes_have_their_own_small_bucket_per_mod() {
        let calls = Calls::default();
        let t0 = Instant::now();
        let (a, b) = (id("acme.a"), id("acme.b"));
        for i in 0..STORAGE_GROWTH_BURST {
            calls
                .allow_growth(&a, t0)
                .unwrap_or_else(|e| panic!("{i}: {e}"));
        }
        assert_eq!(
            calls.allow_growth(&a, t0).unwrap_err().code,
            Code::RateLimited
        );
        // It refills at its own rate, one token per 250 ms.
        assert!(calls
            .allow_growth(&a, t0 + Duration::from_millis(249))
            .is_err());
        assert!(calls
            .allow_growth(&a, t0 + Duration::from_millis(250))
            .is_ok());
        // Another mod is unaffected, and the call bucket is separate from this one.
        assert!(calls.allow_growth(&b, t0).is_ok());
        assert!(calls.admit(&a, t0).is_ok());
        const {
            assert!(STORAGE_GROWTH_PER_SECOND < CALLS_PER_SECOND);
            assert!(STORAGE_GROWTH_BURST <= CALL_BURST);
        }
    }

    #[test]
    fn a_bucket_can_refill_per_minute_exactly_with_no_remainder_lost() {
        // 20 a minute, bursts of 5: a token every 3 s.
        let t0 = Instant::now();
        let mut b = TokenBucket::per(AI_CALLS_PER_MINUTE, 60, AI_CALL_BURST, t0);
        for i in 0..AI_CALL_BURST {
            assert!(b.take(t0), "burst {i}");
        }
        assert!(!b.take(t0));
        assert!(!b.take(t0 + Duration::from_millis(2999)));
        assert!(b.take(t0 + Duration::from_secs(3)));
        assert!(!b.take(t0 + Duration::from_secs(3)));
        // Asking again and again in the gap loses nothing: the same instant is reached by one
        // take or by a thousand microsecond-spaced refusals (a truncating rate would starve here).
        let mut c = TokenBucket::per(20, 60, 1, t0);
        assert!(c.take(t0));
        let mut spent = 0;
        for i in 1..=3_000_000u64 {
            // 3 s in 1 microsecond steps.
            if c.take(t0 + Duration::from_micros(i)) {
                spent += 1;
            }
        }
        assert_eq!(spent, 1, "exactly the token earned in 3 s");
        // A long rest refills to the burst and no further; a zero period is read as one second.
        let mut d = TokenBucket::per(20, 60, 5, t0);
        for _ in 0..5 {
            assert!(d.take(t0));
        }
        let later = t0 + Duration::from_secs(86_400);
        for i in 0..5 {
            assert!(d.take(later), "after a day, token {i}");
        }
        assert!(!d.take(later));
        let mut z = TokenBucket::per(1, 0, 1, t0);
        assert!(z.take(t0));
        assert!(!z.take(t0 + Duration::from_millis(999)));
        assert!(z.take(t0 + Duration::from_secs(1)));
    }

    #[test]
    fn a_general_call_is_never_held_by_a_family_and_counts_nothing() {
        let calls = Calls::default();
        let t0 = Instant::now();
        let a = id("acme.a");
        let held: Vec<_> = (0..500)
            .map(|_| calls.admit_family(&a, Family::General, t0).unwrap())
            .collect();
        assert_eq!(calls.family_in_flight(&a, Family::Ai), 0);
        assert_eq!(calls.family_in_flight(&a, Family::Net), 0);
        drop(held);
        assert_eq!(calls.in_flight(&a), 0);
    }

    #[test]
    fn the_ai_family_holds_two_at_once_and_a_burst_then_a_slow_refill() {
        let calls = Calls::default();
        let t0 = Instant::now();
        let (a, b) = (id("acme.a"), id("acme.b"));
        let one = calls.admit_family(&a, Family::Ai, t0).unwrap();
        let two = calls.admit_family(&a, Family::Ai, t0).unwrap();
        assert_eq!(calls.family_in_flight(&a, Family::Ai), AI_MAX_IN_FLIGHT);
        let busy = calls.admit_family(&a, Family::Ai, t0).unwrap_err();
        assert_eq!(busy.code, Code::RateLimited);
        assert!(busy.message.contains("at once"), "{busy}");
        // The net family, the generic count and another mod are unaffected.
        assert!(calls.admit_family(&a, Family::Net, t0).is_ok());
        assert!(calls.admit_family(&b, Family::Ai, t0).is_ok());
        assert_eq!(calls.in_flight(&a), 0, "the generic count is `admit`'s");
        // A finished call frees its place, and the refused one spent no token: three of the bucket's
        // five are taken (`one`, `two`, `three`), so exactly two remain.
        drop(one);
        assert_eq!(calls.family_in_flight(&a, Family::Ai), 1);
        let three = calls.admit_family(&a, Family::Ai, t0).unwrap();
        drop((two, three));
        let last_two: Vec<_> = (0..2)
            .map(|_| calls.admit_family(&a, Family::Ai, t0).unwrap())
            .collect();
        drop(last_two);
        let empty = calls.admit_family(&a, Family::Ai, t0).unwrap_err();
        assert_eq!(empty.code, Code::RateLimited);
        assert!(empty.message.contains("too often"), "{empty}");
        // One token every 3 s.
        assert!(calls
            .admit_family(&a, Family::Ai, t0 + Duration::from_millis(2999))
            .is_err());
        assert!(calls
            .admit_family(&a, Family::Ai, t0 + Duration::from_secs(3))
            .is_ok());
        assert_eq!(calls.family_in_flight(&a, Family::Ai), 0);
    }

    #[test]
    fn the_net_family_holds_four_at_once_and_refills_by_the_second() {
        let calls = Calls::default();
        let t0 = Instant::now();
        let a = id("acme.a");
        let mut held = Vec::new();
        for _ in 0..NET_MAX_IN_FLIGHT {
            held.push(calls.admit_family(&a, Family::Net, t0).unwrap());
        }
        let busy = calls.admit_family(&a, Family::Net, t0).unwrap_err();
        assert!(busy.message.contains("at once"), "{busy}");
        assert_eq!(calls.family_in_flight(&a, Family::Net), NET_MAX_IN_FLIGHT);
        drop(held);
        assert_eq!(calls.family_in_flight(&a, Family::Net), 0);
        // Burst of 20: four taken; sixteen more one at a time, then it is empty.
        for i in 0..(NET_FETCH_BURST - NET_MAX_IN_FLIGHT as u64) {
            drop(
                calls
                    .admit_family(&a, Family::Net, t0)
                    .unwrap_or_else(|e| panic!("{i}: {e}")),
            );
        }
        let fast = calls.admit_family(&a, Family::Net, t0).unwrap_err();
        assert!(fast.message.contains("too fast"), "{fast}");
        assert!(calls
            .admit_family(&a, Family::Net, t0 + Duration::from_millis(199))
            .is_err());
        assert!(calls
            .admit_family(&a, Family::Net, t0 + Duration::from_millis(200))
            .is_ok());
    }

    /// One test per PR C family (K1): its in-flight cap, its burst, and its refill, with the refusal's
    /// own sentence, and another family and another mod untouched.
    #[test]
    fn the_gift_family_holds_two_at_once_a_burst_of_fifty_and_ten_a_second() {
        let calls = Calls::default();
        let t0 = Instant::now();
        let (a, b) = (id("acme.a"), id("acme.b"));
        let one = calls.admit_family(&a, Family::Gifts, t0).unwrap();
        let two = calls.admit_family(&a, Family::Gifts, t0).unwrap();
        assert_eq!(
            calls.family_in_flight(&a, Family::Gifts),
            GIFT_MAX_IN_FLIGHT
        );
        let busy = calls.admit_family(&a, Family::Gifts, t0).unwrap_err();
        assert_eq!(busy.code, Code::RateLimited);
        assert!(busy.message.contains("at once"), "{busy}");
        // Another family and another mod have their own.
        assert!(calls.admit_family(&a, Family::Embed, t0).is_ok());
        assert!(calls.admit_family(&b, Family::Gifts, t0).is_ok());
        drop((one, two));
        // The burst: two are spent, so 48 more at once, then it is empty.
        for i in 0..(GIFT_BURST - GIFT_MAX_IN_FLIGHT as u64) {
            drop(
                calls
                    .admit_family(&a, Family::Gifts, t0)
                    .unwrap_or_else(|e| panic!("{i}: {e}")),
            );
        }
        let fast = calls.admit_family(&a, Family::Gifts, t0).unwrap_err();
        assert!(fast.message.contains("too fast"), "{fast}");
        // Ten a second: one every 100 ms.
        assert!(calls
            .admit_family(&a, Family::Gifts, t0 + Duration::from_millis(99))
            .is_err());
        assert!(calls
            .admit_family(&a, Family::Gifts, t0 + Duration::from_millis(100))
            .is_ok());
    }

    #[test]
    fn the_embed_family_holds_one_at_once_a_burst_of_ten_and_six_a_minute() {
        let calls = Calls::default();
        let t0 = Instant::now();
        let a = id("acme.a");
        let held = calls.admit_family(&a, Family::Embed, t0).unwrap();
        let busy = calls.admit_family(&a, Family::Embed, t0).unwrap_err();
        assert_eq!(busy.code, Code::RateLimited);
        assert!(busy.message.contains("already"), "{busy}");
        assert_eq!(
            calls.family_in_flight(&a, Family::Embed),
            EMBED_MAX_IN_FLIGHT
        );
        drop(held);
        // Nine more of the ten, one at a time, then the bucket is empty.
        for i in 1..EMBED_BURST {
            drop(
                calls
                    .admit_family(&a, Family::Embed, t0)
                    .unwrap_or_else(|e| panic!("{i}: {e}")),
            );
        }
        let fast = calls.admit_family(&a, Family::Embed, t0).unwrap_err();
        assert!(fast.message.contains("too often"), "{fast}");
        // Six a minute: one every ten seconds.
        assert!(calls
            .admit_family(&a, Family::Embed, t0 + Duration::from_millis(9999))
            .is_err());
        assert!(calls
            .admit_family(&a, Family::Embed, t0 + Duration::from_secs(10))
            .is_ok());
    }

    #[test]
    fn the_links_family_holds_one_at_once_a_burst_of_three_and_six_a_minute() {
        let calls = Calls::default();
        let t0 = Instant::now();
        let a = id("acme.a");
        let held = calls.admit_family(&a, Family::Links, t0).unwrap();
        let busy = calls.admit_family(&a, Family::Links, t0).unwrap_err();
        assert!(busy.message.contains("already"), "{busy}");
        assert_eq!(
            calls.family_in_flight(&a, Family::Links),
            LINKS_MAX_IN_FLIGHT
        );
        drop(held);
        for i in 1..LINK_BURST {
            drop(
                calls
                    .admit_family(&a, Family::Links, t0)
                    .unwrap_or_else(|e| panic!("{i}: {e}")),
            );
        }
        let fast = calls.admit_family(&a, Family::Links, t0).unwrap_err();
        assert!(fast.message.contains("too often"), "{fast}");
        assert!(calls
            .admit_family(&a, Family::Links, t0 + Duration::from_millis(9999))
            .is_err());
        assert!(calls
            .admit_family(&a, Family::Links, t0 + Duration::from_secs(10))
            .is_ok());
        // A family's slot is its own: the embed family was never touched.
        assert_eq!(calls.family_in_flight(&a, Family::Embed), 0);
    }

    /// The answer to a question is admitted by the cap alone: it is refused with the slot taken, and
    /// when let through it takes no token (the question paid it), however many answers there are.
    #[test]
    fn an_answer_is_held_to_the_cap_and_spends_no_token() {
        let calls = Calls::default();
        let t0 = Instant::now();
        let a = id("acme.a");
        let held = calls.admit_family(&a, Family::Links, t0).unwrap();
        let busy = calls
            .admit_family_answer(&a, Family::Links, t0)
            .unwrap_err();
        assert!(busy.message.contains("already"), "{busy}");
        drop(held);
        // Many answers in turn: the burst is untouched, so the full burst of asks is still there.
        for _ in 0..(LINK_BURST * 4) {
            drop(calls.admit_family_answer(&a, Family::Links, t0).unwrap());
        }
        // (`held` took one token of the burst.)
        for i in 1..LINK_BURST {
            drop(
                calls
                    .admit_family(&a, Family::Links, t0)
                    .unwrap_or_else(|e| panic!("{i}: {e}")),
            );
        }
        assert!(calls.admit_family(&a, Family::Links, t0).is_err());
        // The slot is given back each time.
        assert_eq!(calls.family_in_flight(&a, Family::Links), 0);
    }

    /// PR C's numbers are the plan's, tighter than the generic limits, and every non-general family
    /// has limits (a family that returned `None` would be admitted unlimited).
    #[test]
    fn the_pr_c_limits_are_the_plans_and_every_family_has_limits() {
        const {
            assert!(GIFT_MAX_IN_FLIGHT == 2 && GIFTS_PER_SECOND == 10 && GIFT_BURST == 50);
            assert!(EMBED_MAX_IN_FLIGHT == 1 && EMBEDS_PER_MINUTE == 6 && EMBED_BURST == 10);
            assert!(LINKS_MAX_IN_FLIGHT == 1 && LINKS_PER_MINUTE == 6 && LINK_BURST == 3);
            assert!(MAX_GIFTS_PER_MOD == 10_000);
            assert!(GIFT_MAX_IN_FLIGHT < MAX_IN_FLIGHT);
            assert!(GIFTS_PER_SECOND < CALLS_PER_SECOND && GIFT_BURST <= CALL_BURST);
            assert!(EMBED_BURST <= CALL_BURST && LINK_BURST <= CALL_BURST);
        }
        assert_eq!(CONFIRM_TTL, Duration::from_secs(60));
        for family in [
            Family::Ai,
            Family::Net,
            Family::Gifts,
            Family::Embed,
            Family::Links,
        ] {
            let limit = family.limit().expect("a family has limits");
            assert!(limit.max_in_flight >= 1 && limit.rate >= 1 && limit.burst >= 1);
            assert!(limit.busy.ends_with('.') && limit.fast.ends_with('.'));
            assert_ne!(limit.busy, limit.fast);
        }
        assert!(Family::General.limit().is_none());
    }

    #[test]
    fn the_experimental_limits_are_tighter_than_the_generic_ones_and_fit_together() {
        const {
            // A family only ever narrows what the generic limits allow.
            assert!(AI_MAX_IN_FLIGHT < MAX_IN_FLIGHT);
            assert!(NET_MAX_IN_FLIGHT < MAX_IN_FLIGHT);
            assert!(AI_MAX_IN_FLIGHT >= 1 && NET_MAX_IN_FLIGHT >= 1);
            assert!(AI_CALLS_PER_MINUTE < 60 * CALLS_PER_SECOND);
            assert!(NET_FETCHES_PER_SECOND < CALLS_PER_SECOND);
            assert!(AI_CALL_BURST <= CALL_BURST && NET_FETCH_BURST <= CALL_BURST);
            // A burst is something a bucket can hold: at least one, and the rates refill it.
            assert!(AI_CALL_BURST >= 1 && NET_FETCH_BURST >= 1);
            assert!(AI_CALLS_PER_MINUTE >= 1 && NET_FETCHES_PER_SECOND >= 1);
            // A fetch's body, base64 encoded (4 bytes per 3) with its headers' room, is an answer
            // PM will send (MAX_RESULT_BYTES); the request is an argument PM will take.
            assert!(MAX_FETCH_RESPONSE_BYTES / 3 * 4 + (64 << 10) < MAX_RESULT_BYTES);
            // The text form's budget holds the base64 form of the largest body (so a body is never
            // sent as base64 only to be larger) and still leaves the headers' room.
            assert!(MAX_FETCH_TEXT_JSON_BYTES >= MAX_FETCH_RESPONSE_BYTES.div_ceil(3) * 4);
            assert!(MAX_FETCH_TEXT_JSON_BYTES + (64 << 10) < MAX_RESULT_BYTES);
            assert!(MAX_FETCH_REQUEST_BYTES < MAX_ARGS_BYTES);
            // The prompt and its messages fit in one call's arguments.
            assert!(MAX_AI_PROMPT_BYTES * 2 < MAX_ARGS_BYTES);
            assert!(MAX_AI_MESSAGES >= 1 && MAX_AI_OUTPUT_TOKENS >= 1);
            assert!(MAX_FETCH_REDIRECTS >= 1);
            assert!(NOTIFY_MAX_CHARS >= 1 && NOTIFY_PER_DAY >= 1 && SECRET_ASKS_PER_SESSION >= 1);
        }
        // Deadlines nest: connecting fits in a fetch, a fetch in a call.
        assert!(FETCH_CONNECT_TIMEOUT <= FETCH_TOTAL_TIMEOUT);
        assert!(FETCH_TOTAL_TIMEOUT < CALL_TIMEOUT);
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
        assert_eq!(number("MAX_BINARY_BYTES"), MAX_BINARY_RESULT_BYTES as u64);
        assert_eq!(number("FLOOD_CALLS_PER_SECOND"), CALLS_PER_SECOND);
        assert!(
            number("FLOOD_BURST") > CALL_BURST,
            "the page must flood past the bucket"
        );
        assert!(number("MAX_IN_FLIGHT") > MAX_IN_FLIGHT as u64);
        // The page buffers events a frame can't take yet, and never more than the backend lets it
        // have outstanding.
        assert_eq!(number("EVENT_BUFFER"), EVENT_WINDOW);
        const {
            assert!(EVENT_DOCS_BUDGET_BYTES < MAX_EVENT_BYTES);
            assert!(MAX_IDS_PER_EVENT * 21 < MAX_EVENT_BYTES);
        }
    }
}
