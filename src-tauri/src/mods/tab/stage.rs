// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The bytes a consent screen is about (Mods spec §1.13; plan decision 2). Installing a mod from a
//! file or the catalogue is two commands: the first reads (or downloads) and verifies a `.pmmod` and
//! returns what the consent screen shows, *without installing*; the second installs it once the user
//! has answered. Between them the verified bytes wait here, in memory, under a short-lived token.
//!
//! **Why bytes and not a path.** The second command never reads the path again. A file picked
//! and shown to the user, then swapped on disk before "Install", would otherwise install something
//! the user was never shown. What was verified is what is installed (and the install verifies it
//! once more, which re-checks the signing keys against the safety list's revocations).
//!
//! **The token** is 128 random bits, good for one use and ten minutes, bound to the sha256 of the
//! staged bytes ([`Stage::take`] recomputes the hash before it hands the bytes back) and to the
//! store it was made in (`AppState::store_epoch`, so a vault switch or a lock leaves a stale
//! dialog's token good for nothing: what the screen showed was judged against that store's
//! installed mods). At most [`MAX_STAGED`] screens wait at once (the oldest is dropped), so the
//! memory held is bounded by two packages at the verifier's file cap, and an abandoned screen
//! frees itself.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::mods::ids::ModId;
use crate::mods::ledger::Source;

/// How long a consent screen's token is good for.
pub const TTL: Duration = Duration::from_secs(10 * 60);
/// Consent screens held at once.
pub const MAX_STAGED: usize = 2;

/// Which install the staged bytes are for. Decided when the screen is made and carried by the
/// staged entry itself, never by what the page sends back, so a token made for an unsigned file can
/// only finish as one (with the stark screen's id confirm and developer mode checked again) and a
/// signed one only as signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    Signed,
    Unsigned,
    /// A developer's test folder (`tab::folder`): the stark screen's rules, and nothing staged but
    /// the folder's path and the manifest the screen was made from.
    Folder,
}

/// What one screen is about.
#[derive(Debug, Clone)]
pub struct Staged {
    pub id: ModId,
    pub version: String,
    /// The `.pmmod`'s sha256, lowercase hex, as the screen showed it. For a test folder, the
    /// sha256 of its `manifest.json`: the thing the user's answers are about.
    pub sha256: String,
    pub source: Source,
    pub trust: Trust,
    /// The package's bytes. For a test folder, its `manifest.json` (hashed by `sha256`, so the
    /// token's integrity check covers it): the files themselves are the developer's and are read
    /// again at the load, which refuses a manifest that is no longer this one.
    pub bytes: Vec<u8>,
    /// A test folder's canonical root; `None` for a package.
    pub folder: Option<std::path::PathBuf>,
}

struct Entry {
    token: String,
    at: Instant,
    /// The store that was open when the screen was made (`AppState::store_epoch`): what the screen
    /// showed (an update's widening, the answers' defaults, a reinstall) was judged against it.
    epoch: u64,
    staged: Staged,
}

/// The waiting screens, managed state.
#[derive(Default)]
pub struct Stage {
    entries: Mutex<Vec<Entry>>,
}

fn new_token() -> Option<String> {
    let mut raw = [0u8; 16];
    getrandom::fill(&mut raw).ok()?;
    Some(hex::encode(raw))
}

fn sha_of(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

impl Stage {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Hold `staged` and return its token. `None` when no randomness is available (nothing held).
    /// Its sha256 must be the hash of its bytes: a caller that passes otherwise has a bug, and the
    /// screen would then never install (the check in [`Self::take`]).
    pub fn put(&self, staged: Staged, now: Instant, epoch: u64) -> Option<String> {
        let token = new_token()?;
        let mut entries = self.lock();
        entries.retain(|e| now.saturating_duration_since(e.at) < TTL);
        // Room for this one: the oldest goes first.
        while entries.len() >= MAX_STAGED {
            entries.remove(0);
        }
        entries.push(Entry {
            token: token.clone(),
            at: now,
            epoch,
            staged,
        });
        Some(token)
    }

    /// The screen for `token`, once: it is removed whether or not it is still good. `None` for an
    /// unknown, used or expired token, a screen made for another store than the one open now
    /// (`epoch`: a vault switch or a lock since, so what it showed was judged against a different
    /// set of installed mods), or bytes that no longer hash to what the screen showed.
    pub fn take(&self, token: &str, now: Instant, epoch: u64) -> Option<Staged> {
        let mut entries = self.lock();
        let at = entries.iter().position(|e| e.token == token)?;
        let entry = entries.remove(at);
        drop(entries);
        if now.saturating_duration_since(entry.at) >= TTL || entry.epoch != epoch {
            return None;
        }
        (sha_of(&entry.staged.bytes) == entry.staged.sha256).then_some(entry.staged)
    }

    /// How many screens are waiting (the tests).
    #[cfg(test)]
    fn len(&self) -> usize {
        self.lock().len()
    }
}

/// The sha256 a [`Staged`] must carry for `bytes`.
pub fn sha256_of(bytes: &[u8]) -> String {
    sha_of(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staged(n: u8) -> Staged {
        let bytes = vec![n; 64];
        Staged {
            id: ModId::parse("atlas.map").unwrap(),
            version: "1.0.0".into(),
            sha256: sha_of(&bytes),
            source: Source::File,
            trust: Trust::Signed,
            bytes,
            folder: None,
        }
    }

    #[test]
    fn a_token_is_good_once_and_returns_exactly_the_staged_bytes() {
        let stage = Stage::default();
        let now = Instant::now();
        let token = stage.put(staged(1), now, 0).unwrap();
        assert_eq!(token.len(), 32);
        let got = stage.take(&token, now, 0).unwrap();
        assert_eq!(got.bytes, vec![1u8; 64]);
        assert!(stage.take(&token, now, 0).is_none(), "single use");
        assert_eq!(stage.len(), 0);
    }

    #[test]
    fn tokens_are_distinct_and_an_unknown_one_takes_nothing() {
        let stage = Stage::default();
        let now = Instant::now();
        let a = stage.put(staged(1), now, 0).unwrap();
        let b = stage.put(staged(2), now, 0).unwrap();
        assert_ne!(a, b);
        assert!(stage.take("0".repeat(32).as_str(), now, 0).is_none());
        assert!(stage.take("", now, 0).is_none());
        // The wrong guess did not disturb the real ones.
        assert_eq!(stage.take(&b, now, 0).unwrap().bytes[0], 2);
        assert_eq!(stage.take(&a, now, 0).unwrap().bytes[0], 1);
    }

    #[test]
    fn a_screen_is_good_for_ten_minutes_and_an_expired_take_still_spends_it() {
        let stage = Stage::default();
        let t0 = Instant::now();
        let token = stage.put(staged(1), t0, 0).unwrap();
        assert!(stage.take(&token, t0 + TTL, 0).is_none());
        // Spent by the failed take: a second try inside the window gets nothing either.
        assert!(stage.take(&token, t0, 0).is_none());
        let token = stage.put(staged(1), t0, 0).unwrap();
        assert!(stage
            .take(&token, t0 + TTL - Duration::from_secs(1), 0)
            .is_some());
    }

    #[test]
    fn at_most_two_screens_wait_and_the_oldest_makes_room() {
        let stage = Stage::default();
        let now = Instant::now();
        let a = stage.put(staged(1), now, 0).unwrap();
        let b = stage.put(staged(2), now, 0).unwrap();
        let c = stage.put(staged(3), now, 0).unwrap();
        assert_eq!(stage.len(), MAX_STAGED);
        assert!(stage.take(&a, now, 0).is_none(), "the oldest was dropped");
        assert!(stage.take(&b, now, 0).is_some());
        assert!(stage.take(&c, now, 0).is_some());
    }

    #[test]
    fn expired_screens_free_themselves_when_the_next_arrives() {
        let stage = Stage::default();
        let t0 = Instant::now();
        stage.put(staged(1), t0, 0).unwrap();
        stage.put(staged(2), t0, 0).unwrap();
        let later = t0 + TTL + Duration::from_secs(1);
        let fresh = stage.put(staged(3), later, 0).unwrap();
        assert_eq!(
            stage.len(),
            1,
            "both expired ones were swept, not just one evicted"
        );
        assert!(stage.take(&fresh, later, 0).is_some());
    }

    #[test]
    fn bytes_that_no_longer_hash_to_what_the_screen_showed_are_never_handed_back() {
        let stage = Stage::default();
        let now = Instant::now();
        let mut tampered = staged(1);
        tampered.bytes[0] ^= 1;
        let token = stage.put(tampered, now, 0).unwrap();
        assert!(stage.take(&token, now, 0).is_none());
    }

    /// A screen is about the store it was made in: after a vault switch or a lock (a new epoch), the
    /// token that is still in hand finishes nothing, and is spent.
    #[test]
    fn a_token_made_in_one_store_does_not_finish_in_another() {
        let stage = Stage::default();
        let now = Instant::now();
        for later_epoch in [8, 6, 0, u64::MAX] {
            let token = stage.put(staged(1), now, 7).unwrap();
            assert!(
                stage.take(&token, now, later_epoch).is_none(),
                "{later_epoch}"
            );
            // Spent by the refusal, and no second try in the right store gets it back.
            assert!(stage.take(&token, now, 7).is_none(), "{later_epoch}");
        }
        let token = stage.put(staged(1), now, 7).unwrap();
        assert!(stage.take(&token, now, 7).is_some(), "the same store");
    }
}
