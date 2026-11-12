// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The user's own changes to an installed row (Mods spec §1.13, D17): pausing it, turning it back
//! on, and the first-open "Turn on" of an auto-installed pilot. The decisions are small and pure
//! ([`pause_decision`], [`resume_decision`], [`consent_decision`]); this is the edge that carries
//! them out under the ledger lock, one transaction each, and then keeps the registry in step.
//!
//! **A pause is the user's own** (`store::REASON_USER`). It outlives an update, the safety list
//! leaves it alone (so a lifted entry can't switch it back on), and only the user's resume lifts it.
//! A resume never lifts anyone else's stop: not the safety list's pause or removal, not dev mode's.
//! And it asks the cached safety list itself (`ServeCheck`) before it writes anything: the startup
//! check only runs at launch, and the list leaves a user-paused row alone, so without this a mod with
//! a standing entry would serve from the user's resume until the next fetch re-paused it
//! (`ledger::ServeCheck`'s seam note, slice 5 fixes round 2).
//!
//! **Order.** A pause commits its row first and then ends the mod's sessions
//! ([`ModRuntime::pause`](super::ModRuntime::pause)): the dispatcher already refuses a call on a row
//! that isn't active, so nothing runs in between. A resume and a consent reload the registry the way a
//! startup does (the files are checked, a row that can't serve is parked, not served).

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use super::{read_rows, time_text, Ledger, LedgerError, LedgerStore, ServeCheck};
use crate::mods::api::grants::GrantError;
use crate::mods::ids::ModId;
use crate::mods::store;

/// Why the user's change was refused. Each reads as a sentence on the Mods tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeRefusal {
    /// A pause of a mod that isn't running (parked, still installing, or not turned on yet).
    NotRunning {
        state: String,
        reason: Option<String>,
    },
    /// A resume of a mod the user didn't pause: another owner's stop is theirs to lift.
    NotYourPause { reason: Option<String> },
    /// The safety list names this mod now, so it stays off; the text is the list's words.
    Listed(String),
    /// The safety list removed it: the only way forward is the uninstall dialog.
    Removed,
    /// A first-open "Turn on" for a mod that is already on.
    AlreadyOn,
    /// The row changed between the check and the write (another window, an update).
    Changed,
}

impl std::fmt::Display for ChangeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRunning { state, .. } if state == store::STATE_ACTIVE => {
                f.write_str("It hasn't been turned on yet, so there is nothing to pause.")
            }
            Self::NotRunning { .. } => {
                f.write_str("It isn't running, so there is nothing to pause.")
            }
            Self::NotYourPause { reason } => match reason.as_deref() {
                Some(store::REASON_BLACKLIST) => f.write_str(
                    "PM's safety list paused it. It turns back on by itself when the fix arrives.",
                ),
                Some(store::REASON_DEV_MODE) => {
                    f.write_str("It is paused because developer mode is off.")
                }
                _ => f.write_str("You didn't pause it, so it can't be resumed here."),
            },
            Self::Listed(words) => write!(f, "It stays off. {words}"),
            Self::Removed => f.write_str(
                "PM's safety list removed it. Uninstall it to clear what it left behind.",
            ),
            Self::AlreadyOn => f.write_str("It is already turned on."),
            Self::Changed => f.write_str("It changed while you were doing that. Nothing changed."),
        }
    }
}

/// What a pause does to a row in `state` and `reason` (pure). `consented`: its "Turn on" was done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PauseDecision {
    Pause,
    /// Already paused by the user: nothing to write, and not an error.
    Already,
    Refuse(ChangeRefusal),
}

pub fn pause_decision(state: &str, reason: Option<&str>, consented: bool) -> PauseDecision {
    if reason == Some(store::REASON_BLACKLIST_REMOVED) {
        return PauseDecision::Refuse(ChangeRefusal::Removed);
    }
    if state == store::STATE_PAUSED && reason == Some(store::REASON_USER) {
        return PauseDecision::Already;
    }
    if state == store::STATE_ACTIVE && consented {
        return PauseDecision::Pause;
    }
    PauseDecision::Refuse(ChangeRefusal::NotRunning {
        state: state.to_owned(),
        reason: reason.map(str::to_owned),
    })
}

/// What a resume does (pure). `held`: the safety list's words, when it names this mod now.
pub fn resume_decision(
    state: &str,
    reason: Option<&str>,
    held: Option<String>,
) -> Result<(), ChangeRefusal> {
    if reason == Some(store::REASON_BLACKLIST_REMOVED) {
        return Err(ChangeRefusal::Removed);
    }
    if state != store::STATE_PAUSED || reason != Some(store::REASON_USER) {
        return Err(ChangeRefusal::NotYourPause {
            reason: reason.map(str::to_owned),
        });
    }
    match held {
        Some(words) => Err(ChangeRefusal::Listed(words)),
        None => Ok(()),
    }
}

/// What a first-open "Turn on" does (pure): only a row that exists, isn't removed and has not been
/// turned on.
pub fn consent_decision(reason: Option<&str>, consented: bool) -> Result<(), ChangeRefusal> {
    if reason == Some(store::REASON_BLACKLIST_REMOVED) {
        return Err(ChangeRefusal::Removed);
    }
    if consented {
        return Err(ChangeRefusal::AlreadyOn);
    }
    Ok(())
}

/// Where a row stands after a resume or a consent has reloaded the registry: what the Mods tab
/// words next.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Standing {
    pub state: String,
    pub reason: Option<String>,
    /// Whether the registry now serves it.
    pub serving: bool,
}

struct Seen {
    version: String,
    sha: Option<String>,
    state: String,
    reason: Option<String>,
    consented: bool,
}

fn read_seen(conn: &Connection, id: &ModId) -> Result<Seen, LedgerError> {
    conn.query_row(
        "SELECT version, package_sha256, state, state_reason, consented_at IS NOT NULL \
         FROM mods WHERE id = ?1",
        [id.as_str()],
        |r| {
            Ok(Seen {
                version: r.get(0)?,
                sha: r.get(1)?,
                state: r.get(2)?,
                reason: r.get(3)?,
                consented: r.get(4)?,
            })
        },
    )
    .optional()?
    .ok_or(LedgerError::NotInstalled)
}

impl<S: LedgerStore> Ledger<'_, S> {
    /// Pause `id` for the user. Idempotent for a mod the user already paused.
    pub fn pause_by_user(&self, id: &ModId, now: DateTime<Utc>) -> Result<(), LedgerError> {
        let _ledger = self.runtime.ledger_lock();
        let at = time_text(now);
        let wrote = self.with_conn(|conn| {
            let seen = read_seen(conn, id)?;
            match pause_decision(&seen.state, seen.reason.as_deref(), seen.consented) {
                PauseDecision::Already => Ok(false),
                PauseDecision::Refuse(why) => Err(LedgerError::Change(why)),
                PauseDecision::Pause => {
                    // Compare-and-set on the state read: another owner's change since wins.
                    let n = conn.execute(
                        "UPDATE mods SET state = ?2, state_reason = ?3, updated_at = ?4 \
                         WHERE id = ?1 AND state = ?5 AND state_reason IS NULL",
                        params![
                            id.as_str(),
                            store::STATE_PAUSED,
                            store::REASON_USER,
                            at,
                            store::STATE_ACTIVE
                        ],
                    )?;
                    if n == 1 {
                        Ok(true)
                    } else {
                        Err(LedgerError::Change(ChangeRefusal::Changed))
                    }
                }
            }
        })?;
        if wrote {
            // The row says paused; now end the sessions, so a call in flight commits nothing.
            self.runtime.pause(id);
        }
        Ok(())
    }

    /// Turn the user's own pause off. `check` is the cached safety list's serve check: a mod it
    /// names stays off, with its words. Refused for any other owner's stop.
    pub fn resume_by_user(
        &self,
        id: &ModId,
        check: &dyn ServeCheck,
        now: DateTime<Utc>,
    ) -> Result<Standing, LedgerError> {
        let _ledger = self.runtime.ledger_lock();
        let at = time_text(now);
        self.with_conn(|conn| {
            let seen = read_seen(conn, id)?;
            let held = check.hold(id.as_str(), &seen.version, seen.sha.as_deref());
            resume_decision(&seen.state, seen.reason.as_deref(), held)
                .map_err(LedgerError::Change)?;
            let n = conn.execute(
                "UPDATE mods SET state = ?2, state_reason = NULL, updated_at = ?3 \
                 WHERE id = ?1 AND state = ?4 AND state_reason = ?5",
                params![
                    id.as_str(),
                    store::STATE_ACTIVE,
                    at,
                    store::STATE_PAUSED,
                    store::REASON_USER
                ],
            )?;
            if n == 1 {
                Ok(())
            } else {
                Err(LedgerError::Change(ChangeRefusal::Changed))
            }
        })?;
        self.reload_and_stand(id, now, check)
    }

    /// The first-open "Turn on" of an auto-installed pilot (D17): set `consented_at`, run `write`
    /// (the grants the user's answers made, in the same transaction, so a refused grant leaves the
    /// mod off and nothing written), then load the mod the way a startup would.
    pub fn consent(
        &self,
        id: &ModId,
        check: &dyn ServeCheck,
        now: DateTime<Utc>,
        write: impl FnOnce(&Connection) -> Result<(), GrantError>,
    ) -> Result<Standing, LedgerError> {
        let _ledger = self.runtime.ledger_lock();
        let at = time_text(now);
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            let seen = read_seen(&tx, id)?;
            consent_decision(seen.reason.as_deref(), seen.consented)
                .map_err(LedgerError::Change)?;
            let n = tx.execute(
                "UPDATE mods SET consented_at = ?2, updated_at = ?2 \
                 WHERE id = ?1 AND consented_at IS NULL",
                params![id.as_str(), at],
            )?;
            if n != 1 {
                return Err(LedgerError::Change(ChangeRefusal::Changed));
            }
            write(&tx).map_err(LedgerError::Grants)?;
            tx.commit()?;
            Ok(())
        })?;
        self.reload_and_stand(id, now, check)
    }

    /// Make the registry exactly the rows that serve, and say where `id` stands. The caller holds
    /// the ledger lock.
    fn reload_and_stand(
        &self,
        id: &ModId,
        now: DateTime<Utc>,
        check: &dyn ServeCheck,
    ) -> Result<Standing, LedgerError> {
        let rows = self.with_conn(|c| Ok(read_rows(c)?))?;
        self.load(&rows, now, check);
        let seen = self.with_conn(|c| read_seen(c, id))?;
        let serving = self.runtime.is_installed(id);
        Ok(Standing {
            state: seen.state,
            reason: seen.reason,
            serving,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pause_is_only_of_a_running_mod_and_never_over_another_owners_stop() {
        use PauseDecision::*;
        assert_eq!(pause_decision("active", None, true), Pause);
        assert_eq!(pause_decision("paused", Some("user"), true), Already);
        let refused = |state: &str, reason: Option<&str>, consented| {
            matches!(pause_decision(state, reason, consented), Refuse(_))
        };
        // Not turned on yet, parked, installing, and someone else's pauses.
        assert!(refused("active", None, false));
        assert!(refused("parked", Some("files-missing"), true));
        assert!(refused("installing", None, true));
        assert!(refused("paused", Some("blacklist"), true));
        assert!(refused("paused", Some("dev-mode"), true));
        assert_eq!(
            pause_decision("paused", Some("blacklist-removed"), true),
            Refuse(ChangeRefusal::Removed)
        );
    }

    #[test]
    fn a_resume_lifts_only_the_users_own_pause_and_never_past_the_safety_list() {
        assert_eq!(resume_decision("paused", Some("user"), None), Ok(()));
        for (state, reason) in [
            ("active", None),
            ("paused", Some("blacklist")),
            ("paused", Some("dev-mode")),
            ("paused", None),
            ("parked", Some("files-missing")),
        ] {
            assert_eq!(
                resume_decision(state, reason, None),
                Err(ChangeRefusal::NotYourPause {
                    reason: reason.map(str::to_owned)
                }),
                "{state} {reason:?}"
            );
        }
        assert_eq!(
            resume_decision("paused", Some("blacklist-removed"), None),
            Err(ChangeRefusal::Removed)
        );
        // The words of a standing entry beat the user's wish, and are carried back.
        assert_eq!(
            resume_decision(
                "paused",
                Some("user"),
                Some("PM's safety list: it loses notes".into())
            ),
            Err(ChangeRefusal::Listed(
                "PM's safety list: it loses notes".into()
            ))
        );
    }

    #[test]
    fn a_first_open_turn_on_is_once_and_never_for_a_removed_mod() {
        assert_eq!(consent_decision(None, false), Ok(()));
        assert_eq!(consent_decision(Some("blacklist"), false), Ok(()));
        assert_eq!(consent_decision(None, true), Err(ChangeRefusal::AlreadyOn));
        assert_eq!(
            consent_decision(Some("blacklist-removed"), false),
            Err(ChangeRefusal::Removed)
        );
    }
}
