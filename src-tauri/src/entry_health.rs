// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Skip memory for the rows of a person's list, kept in memory only.
//!
//! When a row fails in a way that walks on to the next, PM remembers it for a few minutes so the
//! next requests skip it and say so, instead of paying a failing round trip each time (a Review run
//! of two hundred documents would otherwise hit a dead key two hundred times). The same record is
//! what the list's status reads: "Not working since 14:05: no credit left".
//!
//! The memory is a convenience, never a gate on safety, so it errs towards trying:
//! - it is NEVER persisted (an app restart forgets it);
//! - when EVERY usable row is down it is ignored for that request, so a stale mark can never wedge
//!   a role ([`Health::unless_all_down`]);
//! - a successful call, a key change, an opt-in change and any list edit clear it (the callers that
//!   own those events call [`Health::clear_provider`], [`Health::succeeded`] or [`Health::clear_all`]).
//!
//! The clock is injectable so the lifetimes are plain tests.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use crate::error::FailureClass;
use crate::llm_gateway::CloudProvider;

/// A key or account level failure (a bad key, no credit) marks the PROVIDER down this long.
pub(crate) const KEY_LEVEL_TTL: Duration = Duration::from_secs(10 * 60);
/// A model level failure (the model is gone, PM's setup for it is stale) marks the MODEL down.
pub(crate) const MODEL_LEVEL_TTL: Duration = Duration::from_secs(10 * 60);
/// A busy model is tried again after this.
pub(crate) const BUSY_TTL: Duration = Duration::from_secs(60);
/// A rate limit lasts as long as the provider's `retry-after` says, never longer than this, and this
/// long when it says nothing.
pub(crate) const RATE_LIMIT_CAP: Duration = Duration::from_secs(300);
const RATE_LIMIT_DEFAULT: Duration = Duration::from_secs(60);

/// What the memory is keyed on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Provider(CloudProvider),
    Model(CloudProvider, String),
}

/// A row that is remembered as not working.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Down {
    pub class: FailureClass,
    /// Wall-clock time of the failure, for "Not working since 14:05".
    pub since: SystemTime,
    until: Instant,
}

/// The two clocks the memory reads: a monotonic one for lifetimes and a wall one for display.
pub(crate) trait Clock: Send + Sync {
    fn now(&self) -> Instant;
    fn wall(&self) -> SystemTime;
}

struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn wall(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// How long a class is remembered, and at which level; `None` for a class that is not remembered
/// (a refusal, a limit, a region pause, a failure that stopped the walk: each has its own way of
/// being said, and none of them is "this row is down for a while").
fn rule(class: FailureClass, retry_after: Option<Duration>) -> Option<(bool, Duration)> {
    match class {
        FailureClass::KeyBad | FailureClass::NoCredit => Some((true, KEY_LEVEL_TTL)),
        FailureClass::ModelGone | FailureClass::SetupStale => Some((false, MODEL_LEVEL_TTL)),
        // Busy is about one model (Anthropic's 529 on one model says nothing about its cheaper one),
        // so only that model is skipped; a key-level failure is the provider's.
        FailureClass::Busy => Some((false, BUSY_TTL)),
        FailureClass::RateLimited => Some((
            true,
            retry_after
                .unwrap_or(RATE_LIMIT_DEFAULT)
                .min(RATE_LIMIT_CAP),
        )),
        FailureClass::ContextTooLong
        | FailureClass::NeedsRetention
        | FailureClass::LimitReached
        | FailureClass::Paused
        | FailureClass::Refused
        | FailureClass::StoppedPartway
        | FailureClass::Other => None,
    }
}

pub(crate) struct Health {
    down: Mutex<HashMap<Key, Down>>,
    clock: Arc<dyn Clock>,
}

impl Health {
    pub(crate) fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self {
            down: Mutex::new(HashMap::new()),
            clock,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Key, Down>> {
        // A poisoned map still holds valid marks; the memory is only a convenience.
        self.down.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Remember a failure that walked on. A class that is not remembered does nothing.
    pub(crate) fn mark(
        &self,
        provider: CloudProvider,
        model: &str,
        class: FailureClass,
        retry_after: Option<Duration>,
    ) {
        let Some((provider_level, ttl)) = rule(class, retry_after) else {
            return;
        };
        let key = if provider_level {
            Key::Provider(provider)
        } else {
            Key::Model(provider, model.to_string())
        };
        let down = Down {
            class,
            since: self.clock.wall(),
            until: self.clock.now() + ttl,
        };
        self.lock().insert(key, down);
    }

    /// Whether the row is remembered as down right now (the provider, or this model of it). Expired
    /// marks are dropped as they are met.
    pub(crate) fn is_down(&self, provider: CloudProvider, model: &str) -> Option<Down> {
        let now = self.clock.now();
        let mut map = self.lock();
        map.retain(|_, d| d.until > now);
        map.get(&Key::Provider(provider))
            .or_else(|| map.get(&Key::Model(provider, model.to_string())))
            .cloned()
    }

    /// For one request: what to skip for each of these rows, which are the ones that passed every
    /// other check. If EVERY one of them is remembered as down the memory is ignored (all `None`):
    /// a stale mark can never leave a role with nothing to try.
    pub(crate) fn unless_all_down(&self, rows: &[(CloudProvider, &str)]) -> Vec<Option<Down>> {
        let marks: Vec<Option<Down>> = rows.iter().map(|(p, m)| self.is_down(*p, m)).collect();
        if !marks.is_empty() && marks.iter().all(Option::is_some) {
            return vec![None; rows.len()];
        }
        marks
    }

    /// A call to this row worked: it is not down, nor is its provider.
    pub(crate) fn succeeded(&self, provider: CloudProvider, model: &str) {
        let mut map = self.lock();
        map.remove(&Key::Provider(provider));
        map.remove(&Key::Model(provider, model.to_string()));
    }

    /// A key save, delete or "Check again", or an opt-in change, for this provider.
    pub(crate) fn clear_provider(&self, provider: CloudProvider) {
        self.lock().retain(|k, _| match k {
            Key::Provider(p) | Key::Model(p, _) => *p != provider,
        });
    }

    /// Any edit of a list: the person has just said what they want, so a mark about the old list
    /// must not outlive it (`route::edit::save`).
    pub(crate) fn clear_all(&self) {
        self.lock().clear();
    }
}

/// The one memory PM keeps, on the real clock.
pub(crate) fn global() -> &'static Health {
    static HEALTH: OnceLock<Health> = OnceLock::new();
    HEALTH.get_or_init(|| Health::with_clock(Arc::new(SystemClock)))
}

/// A private memory on the real clock, for a test that must not share the process-wide one (any
/// list edit in a parallel test clears that one).
#[cfg(test)]
pub(crate) fn private_for_test() -> Health {
    Health::with_clock(Arc::new(SystemClock))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clock the test moves by hand.
    struct Fake {
        t0: Instant,
        w0: SystemTime,
        offset: Mutex<Duration>,
    }
    impl Fake {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                t0: Instant::now(),
                w0: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
                offset: Mutex::new(Duration::ZERO),
            })
        }
        fn advance(&self, d: Duration) {
            *self.offset.lock().unwrap() += d;
        }
    }
    impl Clock for Fake {
        fn now(&self) -> Instant {
            self.t0 + *self.offset.lock().unwrap()
        }
        fn wall(&self) -> SystemTime {
            self.w0 + *self.offset.lock().unwrap()
        }
    }

    use CloudProvider::{Anthropic, Gemini, OpenAi};

    fn health() -> (Health, Arc<Fake>) {
        let clock = Fake::new();
        (Health::with_clock(clock.clone()), clock)
    }

    #[test]
    fn a_key_level_failure_marks_the_provider_for_ten_minutes() {
        let (h, clock) = health();
        h.mark(Anthropic, "m1", FailureClass::NoCredit, None);
        // The provider is down for every model of it, not for another provider.
        assert!(h.is_down(Anthropic, "m1").is_some());
        assert!(h.is_down(Anthropic, "m2").is_some());
        assert!(h.is_down(Gemini, "m1").is_none());
        clock.advance(KEY_LEVEL_TTL - Duration::from_secs(1));
        assert!(h.is_down(Anthropic, "m1").is_some());
        clock.advance(Duration::from_secs(1));
        assert!(h.is_down(Anthropic, "m1").is_none());

        h.mark(Gemini, "m", FailureClass::KeyBad, None);
        clock.advance(KEY_LEVEL_TTL);
        assert!(h.is_down(Gemini, "m").is_none());
    }

    #[test]
    fn a_model_level_failure_marks_only_that_model() {
        let (h, clock) = health();
        for class in [FailureClass::ModelGone, FailureClass::SetupStale] {
            h.mark(OpenAi, "gone", class, None);
            assert_eq!(h.is_down(OpenAi, "gone").map(|d| d.class), Some(class));
            assert!(h.is_down(OpenAi, "other").is_none());
            clock.advance(MODEL_LEVEL_TTL);
            assert!(h.is_down(OpenAi, "gone").is_none());
        }
    }

    #[test]
    fn busy_is_sixty_seconds_and_a_rate_limit_follows_retry_after_up_to_five_minutes() {
        let (h, clock) = health();
        h.mark(Anthropic, "m", FailureClass::Busy, None);
        clock.advance(BUSY_TTL - Duration::from_secs(1));
        assert!(h.is_down(Anthropic, "m").is_some());
        clock.advance(Duration::from_secs(1));
        assert!(h.is_down(Anthropic, "m").is_none());

        h.mark(
            Gemini,
            "m",
            FailureClass::RateLimited,
            Some(Duration::from_secs(20)),
        );
        clock.advance(Duration::from_secs(19));
        assert!(h.is_down(Gemini, "m").is_some());
        clock.advance(Duration::from_secs(1));
        assert!(h.is_down(Gemini, "m").is_none());

        // A huge retry-after is capped; none at all is a minute.
        h.mark(
            Gemini,
            "m",
            FailureClass::RateLimited,
            Some(Duration::from_secs(86_400)),
        );
        clock.advance(RATE_LIMIT_CAP);
        assert!(h.is_down(Gemini, "m").is_none());
        h.mark(Gemini, "m", FailureClass::RateLimited, None);
        clock.advance(Duration::from_secs(59));
        assert!(h.is_down(Gemini, "m").is_some());
        clock.advance(Duration::from_secs(1));
        assert!(h.is_down(Gemini, "m").is_none());
    }

    /// A busy model is one model: another model of the same company stays askable, where a bad key
    /// takes every model of its company with it.
    #[test]
    fn a_busy_model_does_not_mark_its_providers_other_models() {
        let (h, _) = health();
        h.mark(Anthropic, "fable", FailureClass::Busy, None);
        assert!(h.is_down(Anthropic, "fable").is_some());
        assert!(h.is_down(Anthropic, "haiku").is_none());
        h.mark(Anthropic, "fable", FailureClass::KeyBad, None);
        assert!(h.is_down(Anthropic, "haiku").is_some());
    }

    #[test]
    fn a_class_that_says_nothing_about_a_row_being_down_is_never_remembered() {
        let (h, _) = health();
        for class in [
            FailureClass::ContextTooLong,
            FailureClass::NeedsRetention,
            FailureClass::LimitReached,
            FailureClass::Paused,
            FailureClass::Refused,
            FailureClass::StoppedPartway,
            FailureClass::Other,
        ] {
            h.mark(OpenAi, "m", class, None);
            assert!(h.is_down(OpenAi, "m").is_none(), "{class:?}");
        }
    }

    #[test]
    fn the_mark_says_when_it_began_and_why() {
        let (h, clock) = health();
        let at = clock.wall();
        h.mark(Anthropic, "m", FailureClass::NoCredit, None);
        clock.advance(Duration::from_secs(30));
        let d = h.is_down(Anthropic, "m").unwrap();
        assert_eq!(d.class, FailureClass::NoCredit);
        assert_eq!(d.since, at);
    }

    #[test]
    fn a_success_a_key_change_and_a_list_edit_clear_it() {
        let (h, _) = health();
        h.mark(Anthropic, "m", FailureClass::NoCredit, None);
        h.mark(Anthropic, "gone", FailureClass::ModelGone, None);
        h.succeeded(Anthropic, "m");
        assert!(h.is_down(Anthropic, "m").is_none());
        // The model mark of a different model is not the success's business.
        assert!(h.is_down(Anthropic, "gone").is_some());

        h.mark(Anthropic, "m", FailureClass::NoCredit, None);
        h.mark(Gemini, "g", FailureClass::KeyBad, None);
        h.clear_provider(Anthropic);
        assert!(h.is_down(Anthropic, "m").is_none());
        assert!(h.is_down(Anthropic, "gone").is_none());
        assert!(h.is_down(Gemini, "g").is_some());

        h.clear_all();
        assert!(h.is_down(Gemini, "g").is_none());
    }

    #[test]
    fn when_every_usable_row_is_down_the_memory_is_ignored() {
        let (h, _) = health();
        h.mark(Anthropic, "a", FailureClass::NoCredit, None);
        let rows = [(Anthropic, "a"), (Gemini, "g")];
        // One of two is down: it is skipped.
        let marks = h.unless_all_down(&rows);
        assert!(marks[0].is_some() && marks[1].is_none());
        // Both down: nothing is skipped, so a stale mark cannot wedge the role.
        h.mark(Gemini, "g", FailureClass::KeyBad, None);
        assert_eq!(h.unless_all_down(&rows), vec![None, None]);
        // And with no rows there is nothing to say.
        assert!(h.unless_all_down(&[]).is_empty());
    }
}
