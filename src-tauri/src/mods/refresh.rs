// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What runs the safety list: the background task that fetches the signed index at launch and every
//! six hours, applies its blacklist and updates the mods it lists (Mods spec §1.10, D6), and the
//! entry points for the rest of PM: the cached index at startup and a restore's copy of it.
//!
//! **The task is its own** ([`spawn_refresher`]). It does not hang off the updater's check
//! (`useUpdater.ts` returns early when that check fails, so a blacklist could wait days), it needs no
//! open vault (the index is per machine; the apply that follows finds no store and does nothing
//! until one is open as the writer, and the startup load applies the cached index then), and it is
//! quiet when offline: a failed attempt is logged only when it is something other than weather, and
//! retried on a short back-off ([`fetch::delay_after`]).
//!
//! **One pass** ([`refresh_once`]), each step off the async runtime where it touches files or the
//! DB, and none of it under the DB lock:
//! 1. fetch `index.json` and its signature ([`fetch::fetch_index_pair`]: pinned URL, redirects to
//!    GitHub's asset hosts only, size and time caps);
//! 2. accept it ([`index_state::accept`]: signature first, then the rules; a refused index changes
//!    nothing, and the cached one stays in force);
//! 3. apply the blacklist of the index in force to the installed rows
//!    ([`ledger::Ledger::apply_blacklist`]): restrictions first, so nothing below runs a mod the
//!    list just stopped;
//! 4. update the catalogue mods it lists newer ([`updater`]), one download at a time, each checked
//!    against the index's sha256 before the verifier sees it;
//! 5. apply again when something was installed, because an update to a paused mod's `fixedIn` is
//!    what resumes it.
//!
//! What the page learns goes out as one typed event, [`safety::NOTICE_EVENT`], to the main window.
//! It is a convenience, not the record: the paused, removed and waiting states are in the rows and
//! the cached index, so a strip drawn after the event was missed (the page wasn't up yet) still shows.

use std::path::Path;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::{DateTime, Utc};
use tauri::{AppHandle, Emitter};

use super::fetch::{
    self, Endpoints, FetchError, ReqwestTransport, Transport, LAUNCH_DELAY, REFRESH_EVERY, TICK,
};
use super::index::{Index, Verdict, VerifiedIndex};
use super::index_state::{self, IndexState, Origin, Outcome};
use super::keys::KeyRing;
use super::ledger::{self, ApplyReport, Landed, Mint};
use super::safety::{self, ModNotice, NOTICE_EVENT};
use super::updater;
use crate::backup::manifest::ModsIndex;

// ---- The schedule -------------------------------------------------------------------------------

/// When the next fetch is due. Pure: [`fetch::due`] and [`fetch::delay_after`] decide, this only
/// remembers the last attempt and how many in a row have failed.
#[derive(Debug, Clone, Default)]
pub struct Schedule {
    last_attempt: Option<DateTime<Utc>>,
    wait: Option<Duration>,
    failures: u32,
}

impl Schedule {
    /// Due at launch (no attempt yet), and then when the wait since the last attempt has passed.
    pub fn due(&self, now: DateTime<Utc>) -> bool {
        fetch::due(self.last_attempt, now, self.wait.unwrap_or(REFRESH_EVERY))
    }

    /// Note an attempt made at `at`: `ok` resets the back-off and waits the full six hours, a
    /// failure waits less ([`fetch::delay_after`]). `jitter` is any random-ish number, so PMs that
    /// started together drift apart.
    pub fn record(&mut self, at: DateTime<Utc>, ok: bool, jitter: u32) {
        self.failures = if ok {
            0
        } else {
            self.failures.saturating_add(1)
        };
        self.last_attempt = Some(at);
        self.wait = Some(fetch::jittered(fetch::delay_after(self.failures), jitter));
    }
}

/// Start the refresher for the life of the app: the first pass shortly after launch, then whenever
/// the [`Schedule`] says. A clock that went backwards, or a laptop that slept through a pass, is
/// simply due at the next tick.
pub fn spawn_refresher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(LAUNCH_DELAY).await;
        let mut schedule = Schedule::default();
        loop {
            let now = Utc::now();
            if schedule.due(now) {
                let result = refresh_once(&app).await;
                if let Err(e) = &result {
                    if !e.is_quiet() {
                        eprintln!("mods: the safety list wasn't refreshed: {e}");
                    }
                }
                let jitter = Utc::now().timestamp_subsec_nanos();
                schedule.record(now, result.is_ok(), jitter);
            }
            tokio::time::sleep(TICK).await;
        }
    });
}

// ---- One pass -----------------------------------------------------------------------------------

/// Why a pass gave nothing.
#[derive(Debug)]
pub enum RefreshError {
    Fetch(FetchError),
    /// Something on this machine (the data folder).
    Local(String),
}

impl RefreshError {
    /// Offline, or not published yet: the ordinary weather, which PM doesn't log.
    pub fn is_quiet(&self) -> bool {
        matches!(self, Self::Fetch(e) if e.is_quiet())
    }
}

impl std::fmt::Display for RefreshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fetch(e) => write!(f, "{e}"),
            Self::Local(why) => f.write_str(why),
        }
    }
}

impl From<FetchError> for RefreshError {
    fn from(e: FetchError) -> Self {
        Self::Fetch(e)
    }
}

/// Run `f` off the async runtime. `None` when the thread panicked (logged).
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| eprintln!("mods: a safety-list task failed ({e})"))
        .ok()
}

/// Fetch, accept, apply and update (module doc). `Ok` when the index was fetched, whatever the
/// acceptance rules made of it: a refused index is a fetch that worked.
pub async fn refresh_once(app: &AppHandle) -> Result<(), RefreshError> {
    // After "Remove PM completely" nothing may bring the data folder back.
    if crate::paths::data_dir_is_purged() {
        return Ok(());
    }
    let data_dir = crate::paths::data_dir(app).map_err(|e| RefreshError::Local(e.to_string()))?;
    let endpoints = Endpoints::machine();
    let transport = ReqwestTransport::new(&endpoints);
    let (index_bytes, signature) = fetch::fetch_index_pair(&transport, &endpoints).await?;
    let now = Utc::now();
    let Some(outcome) = blocking(move || {
        index_state::accept(
            &data_dir,
            safety::ring(),
            &index_bytes,
            &signature,
            now,
            Origin::Fetched,
        )
    })
    .await
    else {
        return Ok(());
    };
    match &outcome.verdict {
        Err(refused) => eprintln!("mods: the fetched safety list was refused: {refused}"),
        Ok(Verdict::Refused(why)) => {
            eprintln!("mods: the fetched safety list wasn't taken: {why}");
        }
        Ok(_) => {}
    }
    let Some(current) = outcome.current.as_ref() else {
        return Ok(());
    };
    work_on(app, &transport, &endpoints, &outcome.state, current, now).await;
    Ok(())
}

/// Steps 3 to 5 of a pass, for the index in force.
async fn work_on<T: Transport>(
    app: &AppHandle,
    transport: &T,
    endpoints: &Endpoints,
    state: &IndexState,
    current: &VerifiedIndex,
    now: DateTime<Utc>,
) {
    let index = current.index.clone();

    // Restrictions first.
    apply_in_background(app, &index, now).await;

    // Then what the index lists newer.
    let planned = {
        let (app, index) = (app.clone(), index.clone());
        blocking(move || {
            ledger::with_active_ledger(&app, Mint::IfRows, |l| match l.plan_pass(&index, now) {
                Ok(pass) => pass,
                Err(e) => {
                    eprintln!("mods: updates not planned ({e})");
                    ledger::Pass::default()
                }
            })
        })
        .await
        .flatten()
        .unwrap_or_default()
    };
    emit_notices(app, &planned.notices);
    if planned.downloads.is_empty() {
        return;
    }

    let keys = safety::trusted_keys(state);
    let mut installed_any = false;
    for done in updater::download_all(transport, endpoints, planned.downloads).await {
        let updater::Downloaded { download, bytes } = done;
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(e) => {
                if !e.is_quiet() {
                    eprintln!(
                        "mods: {} {} not downloaded: {e}",
                        download.id, download.version
                    );
                }
                continue;
            }
        };
        let (app2, index2, keys2) = (app.clone(), index.clone(), keys.clone());
        let (id, version) = (download.id.clone(), download.version.clone());
        let landed = blocking(move || {
            ledger::with_active_ledger(&app2, Mint::IfRows, |l| {
                l.land(&download, &bytes, &keys2, &index2, Utc::now())
            })
        })
        .await
        .flatten();
        let Some((landed, notices)) = landed else {
            continue;
        };
        emit_notices(app, &notices);
        match landed {
            Landed::Installed(i) => {
                installed_any = true;
                eprintln!("mods: {id} updated to {} ({})", i.version, i.state);
            }
            Landed::Waiting { .. } => {}
            Landed::Failed(why) => eprintln!("mods: {id} {version} not installed: {why}"),
        }
    }
    if installed_any {
        // An update to a paused mod's fix is what lets the blacklist turn it back on.
        apply_in_background(app, &index, Utc::now()).await;
    }
}

/// Apply `index`'s blacklist to the open store's rows, off the async runtime, and tell the page.
async fn apply_in_background(app: &AppHandle, index: &Index, now: DateTime<Utc>) {
    let (app2, index2) = (app.clone(), index.clone());
    blocking(move || apply_now(&app2, &index2, now)).await;
}

/// [`ledger::Ledger::apply_blacklist`] on the open store, if one is open as the writer, with its
/// report logged and its notices sent. Blocking.
pub fn apply_now(app: &AppHandle, index: &Index, now: DateTime<Utc>) {
    if let Some(report) =
        ledger::with_active_ledger(app, Mint::IfRows, |l| l.apply_blacklist(index, now))
    {
        report_applied(app, &report);
    }
}

// ---- What the rest of PM calls ------------------------------------------------------------------

/// The accepted index on this machine, verified again, for the startup load and recovery: the
/// blacklist applies offline. `None` when none was ever accepted (or the file doesn't verify).
/// Blocking: one small file read.
pub fn cached_index(app: &AppHandle) -> Option<Index> {
    let data_dir = crate::paths::data_dir(app)
        .map_err(|e| eprintln!("mods: no data folder for the safety list ({e})"))
        .ok()?;
    index_state::load(&data_dir, safety::ring())
        .index
        .map(|v| v.index)
}

/// Log what a blacklist pass did and send its notices.
pub fn report_applied(app: &AppHandle, report: &ApplyReport) {
    if let Some(why) = &report.skipped {
        eprintln!("mods: the safety list wasn't applied, the ledger couldn't be read ({why})");
    }
    for n in &report.notices {
        match n {
            ModNotice::Paused { id, reason, .. } => {
                eprintln!("mods: {id} paused by the safety list: {reason}");
            }
            ModNotice::Removed { id, reason, .. } => {
                eprintln!("mods: {id} removed by the safety list: {reason}");
            }
            ModNotice::Resumed { id, version, .. } => {
                eprintln!("mods: {id} resumed at {version}");
            }
            _ => {}
        }
    }
    for (id, why) in &report.failed {
        eprintln!("mods: the safety list couldn't be applied to {id}: {why}");
    }
    for (id, why) in &report.files_left {
        eprintln!("mods: {id}'s files couldn't all be removed ({why}); retried next time");
    }
    emit_notices(app, &report.notices);
}

/// Send the notices to the main window, the only one that draws strips.
pub fn emit_notices(app: &AppHandle, notices: &[ModNotice]) {
    for n in notices {
        if let Err(e) = app.emit_to(super::MAIN_LABEL, NOTICE_EVENT, n) {
            eprintln!("mods: a notice wasn't delivered ({e})");
        }
    }
}

// ---- Backup and restore -------------------------------------------------------------------------

/// The backup manifest's copy of this machine's cached index, for a new backup. `None` when none
/// was ever accepted. Blocking: one small file read.
pub fn backup_copy(app: &AppHandle) -> Option<ModsIndex> {
    let data_dir = crate::paths::data_dir(app).ok()?;
    copy_of(&index_state::load(&data_dir, safety::ring()).state)
}

/// The cached pair as the manifest carries it (pure).
pub fn copy_of(state: &IndexState) -> Option<ModsIndex> {
    state.cached.as_ref().map(|c| ModsIndex {
        index_b64: B64.encode(&c.index),
        signature_b64: B64.encode(&c.signature),
    })
}

/// A restore's copy of the index run through the ordinary acceptance rules with the keys `ring`
/// holds. The manifest is not trusted: a copy that doesn't verify, or is older than what this
/// machine has seen, changes nothing, so the seen seq ends as the larger of the two and a crafted
/// backup can't lock the real index out. `None` when the copy isn't even base64.
pub fn accept_restored(
    data_dir: &Path,
    ring: &KeyRing,
    copy: &ModsIndex,
    now: DateTime<Utc>,
) -> Option<Outcome> {
    let index = B64.decode(&copy.index_b64).ok()?;
    let signature = B64.decode(&copy.signature_b64).ok()?;
    Some(index_state::accept(
        data_dir,
        ring,
        &index,
        &signature,
        now,
        Origin::Restored,
    ))
}

/// At a restore (the three restore commands, off the async runtime): take the backup's copy of the
/// index if it is newer than the one held (the max of the two seqs, authenticated), and, whichever
/// index is in force now, apply its blacklist to the store open now. The restored store itself is
/// opened later, by `switch_to_vault`, whose engage runs the same apply from the cached index
/// before any frame can mount ([`ledger::after_writer_open`]).
pub fn take_restored(app: &AppHandle, copy: Option<&ModsIndex>) {
    let Some(copy) = copy else {
        return;
    };
    let Ok(data_dir) = crate::paths::data_dir(app) else {
        return;
    };
    let now = Utc::now();
    let Some(outcome) = accept_restored(&data_dir, safety::ring(), copy, now) else {
        eprintln!("mods: the backup's copy of the safety list isn't readable; ignored");
        return;
    };
    match &outcome.verdict {
        Ok(verdict) => eprintln!("mods: the backup's safety list: {verdict:?}"),
        Err(e) => eprintln!("mods: the backup's safety list was refused: {e}"),
    }
    if let Some(current) = outcome.current {
        apply_now(app, &current.index, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::fetch::Hop;
    use crate::mods::index::tests::{good_index, signed, t, Keys};
    use crate::mods::index::{Stale, Verdict};
    use std::collections::VecDeque;
    use std::future::Future;
    use std::sync::Mutex;

    const NOW: &str = "2027-01-15T00:00:00Z";

    // ---- The schedule ---------------------------------------------------------------------------

    #[test]
    fn the_schedule_is_due_at_launch_then_every_six_hours_after_a_success() {
        let mut s = Schedule::default();
        let launch = t("2027-01-15T10:00:00Z");
        assert!(s.due(launch), "the first pass is due at launch");
        s.record(launch, true, 0);
        // Jitter is within 10% of six hours, so 5h24m is never enough and 6h36m always is.
        assert!(!s.due(t("2027-01-15T15:00:00Z")));
        assert!(!s.due(t("2027-01-15T15:23:00Z")));
        assert!(s.due(t("2027-01-15T16:37:00Z")));
        assert!(
            s.due(t("2027-01-17T10:00:00Z")),
            "a laptop that slept is due on waking"
        );
        // A clock set back is due too, rather than blind for hours.
        assert!(s.due(t("2027-01-15T03:00:00Z")));
    }

    #[test]
    fn a_failing_fetch_backs_off_in_steps_and_a_success_resets_it() {
        let mut s = Schedule::default();
        let mut at = t("2027-01-15T10:00:00Z");
        // Offline: 5 min, 15 min, 1 h, then 3 h, never longer than the normal six.
        for (failures, minutes) in [(1u32, 5i64), (2, 15), (3, 60), (4, 180), (5, 180)] {
            s.record(at, false, 0);
            assert_eq!(s.failures, failures);
            let wait = chrono::Duration::minutes(minutes);
            let ten_percent = chrono::Duration::seconds(minutes * 6);
            assert!(
                !s.due(at + wait - ten_percent - chrono::Duration::seconds(1)),
                "{minutes}"
            );
            assert!(
                s.due(at + wait + ten_percent + chrono::Duration::seconds(1)),
                "{minutes}"
            );
            at += wait;
        }
        s.record(at, true, 0);
        assert_eq!(s.failures, 0);
        assert!(!s.due(at + chrono::Duration::hours(5)));
        assert!(s.due(at + chrono::Duration::hours(7)));
    }

    // ---- Fetching and accepting -----------------------------------------------------------------

    struct Script(Mutex<VecDeque<Result<Hop, FetchError>>>);

    impl Transport for Script {
        fn get(
            &self,
            _url: &reqwest::Url,
            _cap: usize,
            _timeout: Duration,
        ) -> impl Future<Output = Result<Hop, FetchError>> + Send {
            let next = self.0.lock().unwrap().pop_front().expect("no more answers");
            async move { next }
        }
    }

    fn pair(index: &[u8], signature: &[u8]) -> Script {
        let hop = |b: &[u8]| {
            Ok(Hop {
                status: 200,
                location: None,
                body: b.to_vec(),
            })
        };
        Script(Mutex::new(vec![hop(index), hop(signature)].into()))
    }

    fn offline() -> Script {
        Script(Mutex::new(
            vec![Err(FetchError::Offline("no route to host".into()))].into(),
        ))
    }

    async fn fetch_and_accept(
        script: &Script,
        dir: &Path,
        ring: &KeyRing,
    ) -> Result<Outcome, FetchError> {
        let (i, s) = fetch::fetch_index_pair(script, &Endpoints::production()).await?;
        Ok(index_state::accept(
            dir,
            ring,
            &i,
            &s,
            t(NOW),
            Origin::Fetched,
        ))
    }

    #[tokio::test]
    async fn a_fetched_index_is_accepted_and_a_forged_or_replayed_one_changes_nothing() {
        let keys = Keys::new();
        let ring = keys.ring();
        let dir = tempfile::tempdir().unwrap();
        let (bytes, sig) = signed(&good_index(5), &keys.main);
        let out = fetch_and_accept(&pair(&bytes, &sig), dir.path(), &ring)
            .await
            .unwrap();
        assert!(matches!(out.verdict, Ok(Verdict::Newer { good: true, .. })));
        assert_eq!(out.state.seq, 5);
        assert!(out.current.is_some());

        // Signed by a key nobody trusts: refused at the signature, the cached one stays in force.
        let stranger = Keys::new();
        let (forged, forged_sig) = signed(&good_index(9), &stranger.main);
        let out = fetch_and_accept(&pair(&forged, &forged_sig), dir.path(), &ring)
            .await
            .unwrap();
        assert!(out.verdict.is_err());
        assert_eq!(out.state.seq, 5);
        assert_eq!(out.current.unwrap().index.seq, 5);

        // An old authentic index: a replay.
        let (old, old_sig) = signed(&good_index(4), &keys.main);
        let out = fetch_and_accept(&pair(&old, &old_sig), dir.path(), &ring)
            .await
            .unwrap();
        assert!(matches!(
            out.verdict,
            Ok(Verdict::Refused(Stale::Older { seq: 4, seen: 5 }))
        ));
        assert_eq!(out.state.seq, 5);
        assert_eq!(
            out.current.unwrap().index.seq,
            5,
            "the newer one still applies"
        );
    }

    #[tokio::test]
    async fn offline_gives_a_quiet_failure_and_leaves_the_record_alone() {
        let keys = Keys::new();
        let dir = tempfile::tempdir().unwrap();
        let err = fetch_and_accept(&offline(), dir.path(), &keys.ring())
            .await
            .unwrap_err();
        assert!(RefreshError::from(err).is_quiet());
        assert!(!dir.path().join(index_state::STATE_FILE).exists());
        let local = RefreshError::Local("no data folder".into());
        assert!(!local.is_quiet());
        assert!(!RefreshError::from(FetchError::TooLarge).is_quiet());
    }

    // ---- Restore --------------------------------------------------------------------------------

    fn state_at(dir: &Path, ring: &KeyRing, seq: u64, keys: &Keys) -> IndexState {
        let (bytes, sig) = signed(&good_index(seq), &keys.main);
        index_state::accept(dir, ring, &bytes, &sig, t(NOW), Origin::Fetched).state
    }

    #[test]
    fn a_backup_carries_the_cached_index_and_a_machine_with_none_has_nothing_to_carry() {
        let keys = Keys::new();
        let ring = keys.ring();
        assert_eq!(copy_of(&IndexState::default()), None);
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path(), &ring, 5, &keys);
        let copy = copy_of(&state).unwrap();
        let cached = state.cached.unwrap();
        assert_eq!(B64.decode(copy.index_b64).unwrap(), cached.index);
        assert_eq!(B64.decode(copy.signature_b64).unwrap(), cached.signature);
    }

    #[test]
    fn a_restore_takes_the_newer_index_and_never_rolls_the_seen_seq_back() {
        let keys = Keys::new();
        let ring = keys.ring();
        let backup_dir = tempfile::tempdir().unwrap();
        let backup_state = state_at(backup_dir.path(), &ring, 5, &keys);
        let copy = copy_of(&backup_state).unwrap();

        // A machine that has seen seq 9: the older backup changes nothing, and the seq stays 9.
        let ahead = tempfile::tempdir().unwrap();
        state_at(ahead.path(), &ring, 9, &keys);
        let out = accept_restored(ahead.path(), &ring, &copy, t(NOW)).unwrap();
        assert!(matches!(
            out.verdict,
            Ok(Verdict::Refused(Stale::Older { seq: 5, seen: 9 }))
        ));
        assert_eq!(out.state.seq, 9);
        assert_eq!(out.current.unwrap().index.seq, 9);
        assert_eq!(index_state::load(ahead.path(), &ring).state.seq, 9);

        // A machine behind it (a fresh install restoring a backup): takes the backup's index, and
        // the restore is not a fetch, so it doesn't count as a good one.
        let behind = tempfile::tempdir().unwrap();
        let out = accept_restored(behind.path(), &ring, &copy, t(NOW)).unwrap();
        assert!(matches!(out.verdict, Ok(Verdict::Newer { .. })));
        assert_eq!(out.state.seq, 5);
        assert_eq!(out.state.last_good_fetch, None);
        assert_eq!(index_state::load(behind.path(), &ring).state.seq, 5);
        // A machine at the same seq with the same bytes: the same index, nothing to do.
        let same = accept_restored(behind.path(), &ring, &copy, t(NOW)).unwrap();
        assert!(matches!(same.verdict, Ok(Verdict::Same { .. })));
    }

    #[test]
    fn a_crafted_backup_cannot_raise_the_seen_seq_or_revoke_a_key() {
        let keys = Keys::new();
        let ring = keys.ring();
        let dir = tempfile::tempdir().unwrap();
        state_at(dir.path(), &ring, 5, &keys);
        // A huge seq, signed by a key nobody trusts; and the right key's signature on other bytes.
        let stranger = Keys::new();
        let (huge, huge_sig) = signed(&good_index((1 << 53) - 1), &stranger.main);
        let (real, real_sig) = signed(&good_index(6), &keys.main);
        let (other, _) = signed(&good_index(7), &keys.main);
        for (index, sig) in [
            (huge.clone(), huge_sig.clone()),
            (other, real_sig.clone()),
            (real.clone(), huge_sig),
            (huge, real_sig),
        ] {
            let copy = ModsIndex {
                index_b64: B64.encode(index),
                signature_b64: B64.encode(sig),
            };
            let out = accept_restored(dir.path(), &ring, &copy, t(NOW)).unwrap();
            assert!(out.verdict.is_err(), "{:?}", out.verdict);
            assert_eq!(out.state.seq, 5);
        }
        assert_eq!(index_state::load(dir.path(), &ring).state.seq, 5);
        // Not even base64: ignored.
        let junk = ModsIndex {
            index_b64: "***".into(),
            signature_b64: "***".into(),
        };
        assert!(accept_restored(dir.path(), &ring, &junk, t(NOW)).is_none());
        // And the genuine newer one is still taken afterwards.
        let genuine = ModsIndex {
            index_b64: B64.encode(real),
            signature_b64: B64.encode(signed(&good_index(6), &keys.main).1),
        };
        let out = accept_restored(dir.path(), &ring, &genuine, t(NOW)).unwrap();
        assert_eq!(out.state.seq, 6);
    }

    // ---- Decision 9, in the build the tests run in ---------------------------------------------

    #[test]
    fn this_build_fetches_the_public_index_unless_a_dev_build_with_a_data_folder_override_says_not()
    {
        // A test run has no PM_DATA_DIR, so even a debug build can't read either override: it
        // fetches the public index and trusts the two Atlas keys. (The gate itself, and the
        // release build's, are pinned in `keys`' tests and `fetch`'s.)
        assert!(Endpoints::machine().is_production());
        assert!(crate::mods::keys::dev_setting(fetch::INDEX_BASE_VAR).is_none());
        assert!(crate::mods::keys::dev_test_key().is_none());
        assert_eq!(
            crate::mods::keys::KeyRing::machine()
                .trusted(&Default::default())
                .public_keys()
                .len(),
            2
        );
    }
}
