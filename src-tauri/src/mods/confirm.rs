// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The confirm registry (PR C, K1): the state behind a question PM asks the person for a mod's call
//! ("Delete this milestone?", "Open this address?", "Create a new project with this name?").
//!
//! **Why PM asks and not the mod.** A grant says a mod may try; a few acts (`policy`: the table
//! [`CONFIRM_EACH_TIME`](super::api::permissions::CONFIRM_EACH_TIME)) are the person's to weigh each
//! time. The question is drawn by PM's own page, in PM's words, and the mod never sees the answer
//! being given, only what came of it.
//!
//! **How it works.**
//! 1. A handler that finds its call needs the yes reads its target under the DB guard, builds the
//!    question's text in Rust from PM's data (never from mod text), and asks the registry for a
//!    token ([`ModRuntime::confirm_mint`](super::runtime::ModRuntime::confirm_mint)). The call is
//!    answered `MOD-2003` carrying `{token, summary}`.
//! 2. The page draws the dialog and answers with `mod_confirm(token, accept)`. The page never
//!    performs the action, and never from arguments a frame supplied: the action is the one Rust
//!    stored with the token.
//! 3. Rust takes the token (single use), then on Accept re-runs the dispatcher's checks under the DB
//!    guard, reads the target again, refuses if what the question was about changed, and performs the
//!    action. A Decline is answered `MOD-2004`.
//!
//! **What a token is.** 128 random bits, bound to the call's session (and so its mod, its generation
//! and its store), its action, and a hash of the facts the question was built from; good for
//! [`CONFIRM_TTL`] and used once, whatever the answer. A token the registry does not hold (used,
//! expired, revoked, never minted) does nothing.
//!
//! **One question at a time.** A session holds at most one outstanding token. The dialog is modal,
//! so the page cannot click past it: a second request for the same session is a script or a double
//! dispatch, and is refused (`MOD-3001`), never queued; the first stays.
//!
//! **A mod that keeps asking.** The dialog is modal over the whole app, so a mod that asked again the
//! moment it was told no would leave the person no way to reach the Mods tab and pause it. The slot
//! counts the noes ([`Slot::answered`]): after [`DECLINES_BEFORE_QUIET`] in a row (a yes clears the
//! count) PM asks nothing more for that session for [`QUIET_AFTER_DECLINES`], and the call is answered
//! `MOD-3001` with no dialog. It is Rust's count, not the page's.
//!
//! **The registry dies with the session.** The slot lives inside the session's own record
//! (`runtime::Session`), so every way a session ends takes its token with it: the lock moves the
//! session to a new generation and clears the slot ([`ModRuntime::bump_all`]); a vault switch or a
//! store close ends every session ([`ModRuntime::clear`]); an uninstall, a pause, a reinstall, the
//! frame's own close (a tab switch) and a page reload end theirs. A token is also checked against
//! the session's generation and the store's epoch when it is taken, so a revocation missed could
//! still not be redeemed. A suspend (K5) never waits for a question: its deadline calls
//! [`ModRuntime::confirm_revoke`](super::runtime::ModRuntime::confirm_revoke).
//!
//! [`ModRuntime::bump_all`]: super::runtime::ModRuntime::bump_all
//! [`ModRuntime::clear`]: super::runtime::ModRuntime::clear

use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::api::limits::CONFIRM_TTL;
use super::api::table::Row;

/// How many noes in a row (none of them followed by a yes) a session may collect inside
/// [`DECLINE_WINDOW`] before PM stops asking for it, and how long it then stays quiet. A mod that asks
/// again the moment it is told no would otherwise put a modal dialog over the whole app for good, with
/// the Mods tab (where the person could pause it) out of reach. The person's own next move, a new
/// click on a button that asks, is let through for the first noes: this only ends a treadmill.
pub const DECLINES_BEFORE_QUIET: u8 = 3;
pub const DECLINE_WINDOW: Duration = Duration::from_secs(120);
pub const QUIET_AFTER_DECLINES: Duration = Duration::from_secs(120);

/// What the person's yes performs. A variant holds what Rust validated and nothing a frame can change
/// afterwards: the target's own ids, or text the handler already held to its rules (a parsed address,
/// a project name, a milestone's label and date). Each has its three methods in `api::confirm`: how
/// to read its target again, how to say it, and how to do it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `shell.openLink` for a second-party or unsigned mod: the parsed https address, which is the
    /// only thing shown and the only thing opened.
    OpenLink { url: String },
    /// `milestones.delete` of a milestone the mod did not make.
    DeleteMilestone { id: i64 },
    /// `milestones.add` naming a project that does not exist yet ("Create a new project with this
    /// name?"). On a yes the project and the milestone are made in one transaction, and the project is
    /// the person's own.
    CreateProject {
        project: String,
        label: String,
        due_date: Option<String>,
    },
    /// A test's action: sets the mod's own `mod_kv` row `key` to `"confirmed"`. The question is built
    /// from that row's current value, so a test can change what the question was about.
    #[cfg(test)]
    Probe { key: String },
    /// A test's action that does what `Probe` does and counts as making `adding` gifts, so the answer's
    /// quota check has something to refuse.
    #[cfg(test)]
    Gifting { key: String, adding: u64 },
}

/// What a handler asks the registry to hold: the call's table row (its permission is checked again
/// when the answer comes), the action, and the hash of the facts the question was built from
/// ([`snapshot_hash`]).
#[derive(Debug, Clone)]
pub struct Ask {
    pub row: &'static Row,
    pub action: Action,
    pub snapshot: [u8; 32],
}

/// What a session must still be, when a token is taken, for the token to count: the generation it
/// had and the store it was over when the token was minted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bound {
    pub generation: u64,
    pub store_epoch: u64,
}

/// One outstanding question.
#[derive(Debug, Clone)]
pub struct Outstanding {
    token: String,
    pub ask: Ask,
    bound: Bound,
    expires: Instant,
}

/// Why a token wasn't minted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The session is already waiting for the person's answer to another call.
    Busy,
    /// The person has said no to this session's questions [`DECLINES_BEFORE_QUIET`] times in a row, so
    /// PM is not asking again for a while ([`QUIET_AFTER_DECLINES`]).
    Quiet,
    /// The call's session is no longer the one it began under.
    Ended,
    /// PM couldn't make a random token.
    Random,
}

/// A session's place for its one outstanding question. It is a field of the session's record, so it
/// cannot outlive it.
#[derive(Debug, Default)]
pub struct Slot {
    held: Option<Outstanding>,
    /// Noes since the last yes, and when the first of them was said.
    declines: u8,
    declines_from: Option<Instant>,
    /// PM asks nothing for this session until then.
    quiet_until: Option<Instant>,
}

impl Slot {
    /// Hold `ask` under a fresh token, unless a question is already waiting. An expired one is gone, and
    /// doesn't block (it could never be answered).
    pub fn mint(&mut self, ask: Ask, bound: Bound, now: Instant) -> Result<String, Refused> {
        if self.is_waiting(now) {
            return Err(Refused::Busy);
        }
        if self.quiet_until.is_some_and(|until| until > now) {
            return Err(Refused::Quiet);
        }
        let token = new_token().ok_or(Refused::Random)?;
        self.held = Some(Outstanding {
            token: token.clone(),
            ask,
            bound,
            expires: now + CONFIRM_TTL,
        });
        Ok(token)
    }

    /// Take the question `token` names, if this slot holds it: gone from the slot whatever happens
    /// next (a token is single use, answered or not). `None` for any other token. The caller checks
    /// the question is still [`Outstanding::answerable`].
    pub fn take(&mut self, token: &str) -> Option<Outstanding> {
        if self.held.as_ref().is_some_and(|o| o.token == token) {
            self.held.take()
        } else {
            None
        }
    }

    /// The person answered the question this session held (taken by [`Slot::take`], and answerable
    /// when it was). A yes clears the count of noes; the [`DECLINES_BEFORE_QUIET`]th no inside
    /// [`DECLINE_WINDOW`] makes PM quiet for this session for [`QUIET_AFTER_DECLINES`].
    pub fn answered(&mut self, accept: bool, now: Instant) {
        if accept {
            self.declines = 0;
            self.declines_from = None;
            return;
        }
        if self
            .declines_from
            .is_none_or(|from| now.duration_since(from) > DECLINE_WINDOW)
        {
            self.declines = 0;
            self.declines_from = Some(now);
        }
        self.declines = self.declines.saturating_add(1);
        if self.declines >= DECLINES_BEFORE_QUIET {
            self.quiet_until = Some(now + QUIET_AFTER_DECLINES);
            self.declines = 0;
            self.declines_from = None;
        }
    }

    /// Forget the question, if any. True when there was one.
    pub fn revoke(&mut self) -> bool {
        self.held.take().is_some()
    }

    /// Whether a question that can still be answered is held.
    pub fn is_waiting(&self, now: Instant) -> bool {
        self.held.as_ref().is_some_and(|o| o.expires > now)
    }

    /// Whether any question, answerable or not, is held (a test's question).
    #[cfg(test)]
    pub fn holds_any(&self) -> bool {
        self.held.is_some()
    }
}

impl Outstanding {
    /// Whether the question may still be answered at `now`, for a session now at `bound`: not run out,
    /// and the session is the one it was asked in (the lock, a store swap or anything else that moved
    /// either one ended it).
    pub fn answerable(&self, now: Instant, bound: Bound) -> bool {
        self.expires > now && self.bound == bound
    }
}

/// 128 random bits as 32 lowercase hex characters.
fn new_token() -> Option<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).ok()?;
    Some(hex::encode(bytes))
}

/// The hash a question is bound to: the action's name and the facts the question's text was built
/// from, each length-prefixed so one can never be read as part of the other. Taken again when the
/// answer comes: a different hash means what the question was about changed.
pub fn snapshot_hash(action: &str, facts: &str) -> [u8; 32] {
    let mut hash = Sha256::new();
    for part in [action, facts] {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    hash.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::api::limits::Family;
    use crate::mods::api::permissions::Class;
    use crate::mods::api::table::{Kind, Row};
    use std::time::Duration;

    const ROW: Row = Row {
        name: "test.confirm",
        kind: Kind::Call,
        permission: Some("storage"),
        class: Class::Local,
        added: (1, 0),
        deprecated: None,
        removed: None,
        experimental: false,
        family: Family::General,
    };

    fn ask() -> Ask {
        Ask {
            row: &ROW,
            action: Action::Probe { key: "k".into() },
            snapshot: snapshot_hash("probe", "v"),
        }
    }

    const BOUND: Bound = Bound {
        generation: 3,
        store_epoch: 1,
    };

    #[test]
    fn a_token_is_128_random_bits_as_hex_and_never_repeats() {
        let now = Instant::now();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..64 {
            let mut slot = Slot::default();
            let token = slot.mint(ask(), BOUND, now).unwrap();
            assert_eq!(token.len(), 32);
            assert!(token
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
            assert!(seen.insert(token), "a token repeated");
        }
    }

    #[test]
    fn a_token_is_used_once_and_only_by_its_own_slot() {
        let now = Instant::now();
        let mut slot = Slot::default();
        let token = slot.mint(ask(), BOUND, now).unwrap();
        // Another token (even one a character off) takes nothing and leaves the question held.
        let mut near = token.clone();
        near.replace_range(..1, if near.starts_with('0') { "1" } else { "0" });
        for wrong in ["", "x", near.as_str()] {
            assert!(slot.take(wrong).is_none(), "{wrong:?}");
        }
        assert!(slot.is_waiting(now));
        let got = slot.take(&token).expect("its own token");
        assert!(got.answerable(now, BOUND));
        // A second use finds nothing: single use.
        assert!(slot.take(&token).is_none());
        assert!(!slot.holds_any());
    }

    #[test]
    fn a_second_question_is_refused_while_one_waits_and_an_expired_one_does_not_block() {
        let now = Instant::now();
        let mut slot = Slot::default();
        let first = slot.mint(ask(), BOUND, now).unwrap();
        assert_eq!(slot.mint(ask(), BOUND, now), Err(Refused::Busy));
        assert_eq!(
            slot.mint(ask(), BOUND, now + CONFIRM_TTL - Duration::from_millis(1)),
            Err(Refused::Busy)
        );
        // The first stays answerable after the refused second.
        assert!(slot.take(&first).is_some());
        // Run out: the next question replaces it, and the old token is dead.
        let old = slot.mint(ask(), BOUND, now).unwrap();
        let later = now + CONFIRM_TTL;
        assert!(!slot.is_waiting(later));
        let fresh = slot.mint(ask(), BOUND, later).unwrap();
        assert_ne!(old, fresh);
        assert!(slot.take(&old).is_none());
        assert!(slot.take(&fresh).is_some());
    }

    /// Asking, then declining, [n] times from `start`, a second apart.
    fn decline_n(slot: &mut Slot, n: u32, start: Instant) -> Instant {
        let mut at = start;
        for _ in 0..n {
            let token = slot.mint(ask(), BOUND, at).unwrap();
            slot.take(&token).unwrap();
            slot.answered(false, at);
            at += Duration::from_secs(1);
        }
        at
    }

    #[test]
    fn a_third_no_in_a_row_makes_pm_quiet_for_a_while_and_then_it_asks_again() {
        let now = Instant::now();
        let mut slot = Slot::default();
        let at = decline_n(&mut slot, 2, now);
        // Two noes are let through: the person may well have clicked twice.
        let token = slot.mint(ask(), BOUND, at).unwrap();
        slot.take(&token).unwrap();
        slot.answered(false, at);
        assert_eq!(slot.mint(ask(), BOUND, at), Err(Refused::Quiet));
        assert_eq!(
            slot.mint(
                ask(),
                BOUND,
                at + QUIET_AFTER_DECLINES - Duration::from_millis(1)
            ),
            Err(Refused::Quiet)
        );
        assert!(slot.mint(ask(), BOUND, at + QUIET_AFTER_DECLINES).is_ok());
    }

    #[test]
    fn a_yes_clears_the_noes_and_old_noes_age_out() {
        let now = Instant::now();
        let mut slot = Slot::default();
        let at = decline_n(&mut slot, 2, now);
        slot.answered(true, at);
        let at = decline_n(&mut slot, 2, at);
        assert!(
            slot.mint(ask(), BOUND, at).is_ok(),
            "a yes cleared the count"
        );
        let mut slot = Slot::default();
        let at = decline_n(&mut slot, 2, now);
        // The third no comes after the window: it is the first of a new count.
        let late = at + DECLINE_WINDOW;
        let token = slot.mint(ask(), BOUND, late).unwrap();
        slot.take(&token).unwrap();
        slot.answered(false, late);
        assert!(slot.mint(ask(), BOUND, late).is_ok());
    }

    #[test]
    fn a_question_is_answerable_only_within_its_minute_and_in_its_own_session() {
        let now = Instant::now();
        let mut slot = Slot::default();
        let token = slot.mint(ask(), BOUND, now).unwrap();
        let o = slot.take(&token).unwrap();
        assert!(o.answerable(now + CONFIRM_TTL - Duration::from_millis(1), BOUND));
        assert!(!o.answerable(now + CONFIRM_TTL, BOUND));
        for moved in [
            Bound {
                generation: 4,
                ..BOUND
            },
            Bound {
                store_epoch: 2,
                ..BOUND
            },
        ] {
            assert!(!o.answerable(now, moved), "{moved:?}");
        }
    }

    #[test]
    fn revoking_empties_the_slot() {
        let now = Instant::now();
        let mut slot = Slot::default();
        assert!(!slot.revoke());
        let token = slot.mint(ask(), BOUND, now).unwrap();
        assert!(slot.revoke());
        assert!(slot.take(&token).is_none());
        assert!(
            slot.mint(ask(), BOUND, now).is_ok(),
            "the slot is free again"
        );
    }

    /// The snapshot is of the action's name and the facts, and two different splits of the same
    /// characters are different snapshots.
    #[test]
    fn the_snapshot_hash_binds_the_action_and_every_fact() {
        let base = snapshot_hash("milestone.delete", "12|Launch|2026-12-01");
        assert_eq!(
            base,
            snapshot_hash("milestone.delete", "12|Launch|2026-12-01")
        );
        for (action, facts) in [
            ("milestone.delete", "12|Launch|2026-12-02"),
            ("milestone.delete", "12|Launch!|2026-12-01"),
            ("milestone.remove", "12|Launch|2026-12-01"),
            ("milestone.delet", "e12|Launch|2026-12-01"),
        ] {
            assert_ne!(base, snapshot_hash(action, facts), "{action} {facts}");
        }
    }
}
