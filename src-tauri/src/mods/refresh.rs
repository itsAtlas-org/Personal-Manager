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
//! 1. check the local record is usable (a damaged one with no backup means nothing can be taken, so
//!    nothing is downloaded: [`RefreshError::RecordUnusable`]), then fetch `index.json` and its
//!    signature ([`fetch::fetch_index_pair`]: pinned URL, redirects to GitHub's asset hosts only,
//!    size and time caps);
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
                    // A damaged or unreadable record is silent in the log after the first line, and
                    // nothing is fetched until it is mended: the strip must appear without a
                    // restart, so the page is told on every pass that meets it.
                    if let Some(notice) = record_notice(e) {
                        emit_notices(&app, &[notice]);
                    }
                }
                let jitter = Utc::now().timestamp_subsec_nanos();
                schedule.record(now, schedule_ok(&result), jitter);
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
    /// Both files came down but the pair wasn't taken: a signature that doesn't match the index (a
    /// torn pair, from a republish caught between its two uploads or a CDN edge holding one new
    /// file), an untrusted key, or an index that is stale, replayed or too old for this PM. The
    /// cached index stayed in force and its pass ran. It counts as a failure for the schedule, so
    /// the next try comes on the short back-off ([`fetch::delay_after`]) and a blacklist entry
    /// published in that same republish isn't six hours late.
    Refused(String),
    /// This machine's record of the safety list is damaged and no backup copy stands in for it
    /// ([`index_state::Loaded::fault`]), so no index could be taken. Found *before* any download:
    /// fetching would only repeat the same refusal. The cause is local and mends only when the
    /// record is deleted or PM is reinstalled at a version that reads it, so the retry is on the
    /// full six hours ([`Self::counts_as_failure`]), not the short back-off, and the line
    /// [`index_state::load`] logs is the only noise. The Mods tab tells the user: a
    /// [`ModNotice::RecordFault`] goes out each time ([`spawn_refresher`]), and `mods_safety_status`
    /// reads [`fault_of`].
    RecordUnusable(String),
    /// The same refusal, but the cause is that the record couldn't be *read* (a transient I/O
    /// error, a data folder on a network or synced drive that is briefly away), not that it is
    /// damaged: that can mend by itself, so the retry is on the short back-off and a blacklist
    /// entry published meanwhile isn't six hours late. Nothing is downloaded either.
    RecordUnreadable(String),
}

impl RefreshError {
    /// Offline, or not published yet: the ordinary weather, which PM doesn't log. A damaged
    /// record is already logged by the read that found it.
    pub fn is_quiet(&self) -> bool {
        match self {
            Self::Fetch(e) => e.is_quiet(),
            Self::RecordUnusable(_) | Self::RecordUnreadable(_) => true,
            Self::Local(_) | Self::Refused(_) => false,
        }
    }

    /// Whether the schedule retries on the short back-off. A damaged record doesn't: retrying
    /// sooner can't mend it. An unreadable one does: the read may succeed next time.
    pub fn counts_as_failure(&self) -> bool {
        !matches!(self, Self::RecordUnusable(_))
    }
}

impl std::fmt::Display for RefreshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fetch(e) => write!(f, "{e}"),
            Self::Local(why)
            | Self::Refused(why)
            | Self::RecordUnusable(why)
            | Self::RecordUnreadable(why) => f.write_str(why),
        }
    }
}

/// Why `outcome` wasn't taken, `None` when it was (a newer index, or the same one again; pure).
fn refusal(outcome: &Outcome) -> Option<RefreshError> {
    match &outcome.verdict {
        Err(refused) => Some(RefreshError::Refused(format!(
            "the fetched safety list was refused: {refused}"
        ))),
        Ok(Verdict::Refused(why)) => Some(RefreshError::Refused(format!(
            "the fetched safety list wasn't taken: {why}"
        ))),
        Ok(Verdict::Same { .. } | Verdict::Newer { .. }) => None,
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

/// Fetch, accept, apply and update (module doc). `Ok` when an index was taken (or the one held was
/// fetched again). A pair that downloaded but wasn't taken is [`RefreshError::Refused`], returned
/// after the index in force has been worked on, so the pass isn't lost and the retry is soon.
pub async fn refresh_once(app: &AppHandle) -> Result<(), RefreshError> {
    // After "Remove PM completely" nothing may bring the data folder back.
    if crate::paths::data_dir_is_purged() {
        return Ok(());
    }
    let data_dir = crate::paths::data_dir(app).map_err(|e| RefreshError::Local(e.to_string()))?;
    let endpoints = Endpoints::machine();
    let transport = ReqwestTransport::new(&endpoints);
    let (index_bytes, signature) =
        fetch_pair_if_record_usable(&transport, &endpoints, &data_dir, safety::ring()).await?;
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
    let refused = refusal(&outcome);
    if let Some(current) = outcome.current.as_ref() {
        work_on(app, &transport, &endpoints, &outcome.state, current, now).await;
    }
    refused.map_or(Ok(()), Err)
}

/// The notice a pass that stopped at the record is owed (pure): none for any other outcome.
fn record_notice(e: &RefreshError) -> Option<ModNotice> {
    match e {
        RefreshError::RecordUnusable(text) => Some(ModNotice::RecordFault {
            unreadable: false,
            text: text.clone(),
        }),
        RefreshError::RecordUnreadable(text) => Some(ModNotice::RecordFault {
            unreadable: true,
            text: text.clone(),
        }),
        _ => None,
    }
}

/// How a pass's result is told to the schedule: ok waits the full six hours, a failure backs off.
fn schedule_ok(result: &Result<(), RefreshError>) -> bool {
    result
        .as_ref()
        .map_or_else(|e| !e.counts_as_failure(), |_| true)
}

/// Step 1 of a pass, behind a check of the record: with it damaged and no backup, [`accept`] would
/// refuse whatever came down, so nothing is downloaded (no traffic every few minutes to no effect).
async fn fetch_pair_if_record_usable<T: Transport>(
    transport: &T,
    endpoints: &Endpoints,
    data_dir: &Path,
    ring: &KeyRing,
) -> Result<(Vec<u8>, Vec<u8>), RefreshError> {
    let (dir, ring) = (data_dir.to_path_buf(), ring.clone());
    // A panicked read counts as usable: `accept` re-checks, so this gate only saves traffic.
    if let Some(Some((why, unread))) = blocking(move || record_fault_in(&dir, &ring)).await {
        return Err(if unread {
            RefreshError::RecordUnreadable(why)
        } else {
            RefreshError::RecordUnusable(why)
        });
    }
    Ok(fetch::fetch_index_pair(transport, endpoints).await?)
}

/// Why the record at `data_dir` is unusable, `None` when it is whole, backed up, or not there yet;
/// with whether it was an I/O failure reading it (`unread`: may mend by itself) rather than damage.
fn record_fault_in(data_dir: &Path, ring: &KeyRing) -> Option<(String, bool)> {
    fault_of(&index_state::load(data_dir, ring))
}

/// Why a record that was read is unusable, in a sentence, with whether the cause was an I/O failure
/// reading it (`true`: may mend by itself) rather than damage; `None` when it is whole, backed up,
/// or not there yet (pure). The Mods tab's strip says it, and `reset_safety_record` is its one way
/// out, which forgets the seen seq and the revoked keys (the floor goes back to `MIN_INDEX_SEQ`),
/// so it is the user's confirmed choice, never automatic.
pub fn fault_of(loaded: &index_state::Loaded) -> Option<(String, bool)> {
    loaded.fault.then(|| {
        let problem = loaded.problem.as_deref().unwrap_or("unreadable");
        let why = if loaded.unread {
            format!("this machine's record of the safety list couldn't be read ({problem}); it is tried again soon")
        } else {
            format!("this machine's record of the safety list is damaged ({problem}); no index is taken until it is reset")
        };
        (why, loaded.unread)
    })
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
    if !planned.notices.is_empty() {
        emit_changed(app, CHANGED_DECIDED);
    }
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
                emit_changed(app, CHANGED_UPDATED);
            }
            // A newly recorded waiting update is a note the list reads (and a notice was sent).
            Landed::Waiting { changed: true, .. } => emit_changed(app, CHANGED_DECIDED),
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
    // A notice is a row that changed (paused, resumed, removed): the list reads the rows again.
    if !report.notices.is_empty() {
        emit_changed(app, CHANGED_SAFETY);
    }
}

/// Send the notices to the main window, the only one that draws strips.
pub fn emit_notices(app: &AppHandle, notices: &[ModNotice]) {
    for n in notices {
        if let Err(e) = app.emit_to(super::MAIN_LABEL, NOTICE_EVENT, n) {
            eprintln!("mods: a notice wasn't delivered ({e})");
        }
    }
}

/// The one event that says "the installed mods, or what the registry serves, changed": sent to the
/// main window after every change to the `mods` rows or the registry, whoever made it (the Mods
/// tab's own commands, the safety list's apply, the background updater, the startup load, a
/// same-machine restore's recovery). The payload says why ([`ModsChanged::cause`]); the page's
/// answer is always the same, to read the rows again (`list_mods`). A listener that missed one reads
/// the rows on mount anyway: this is a convenience, not the record.
pub const CHANGED_EVENT: &str = "mods://changed";
/// [`ModsChanged::cause`] for mods whose files were copied back from another store of the vault
/// after a same-machine restore (track B's `after_writer_open` emit, routed through here when the
/// two branches merge).
#[allow(dead_code)]
pub const CHANGED_RECOVERED: &str = "recovered";
/// A mod was installed, updated or repaired from the Mods tab.
pub const CHANGED_INSTALLED: &str = "installed";
pub const CHANGED_UNINSTALLED: &str = "uninstalled";
pub const CHANGED_PAUSED: &str = "paused";
pub const CHANGED_RESUMED: &str = "resumed";
/// A first-open "Turn on" (D17).
pub const CHANGED_CONSENTED: &str = "consented";
/// The safety list paused, resumed or removed a mod.
pub const CHANGED_SAFETY: &str = "safety";
/// The background updater installed a newer version.
pub const CHANGED_UPDATED: &str = "updated";
/// The startup load made the registry from the rows.
pub const CHANGED_LOADED: &str = "loaded";
/// A permission decision, or a note kept for a strip (dismissed, waiting), changed.
pub const CHANGED_DECIDED: &str = "decided";

/// What `mods://changed` carries.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModsChanged {
    pub cause: &'static str,
}

/// Tell the main window the installed mods changed (see [`CHANGED_EVENT`]).
pub fn emit_changed(app: &AppHandle, cause: &'static str) {
    if let Err(e) = app.emit_to(super::MAIN_LABEL, CHANGED_EVENT, ModsChanged { cause }) {
        eprintln!("mods: a change notice wasn't delivered ({e})");
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

    /// A pair that downloaded but wasn't taken is a failed pass for the schedule: the next try is
    /// on the short back-off, not six hours away.
    #[tokio::test]
    async fn a_pair_that_wasnt_taken_is_retried_soon_not_in_six_hours() {
        let keys = Keys::new();
        let ring = keys.ring();
        let dir = tempfile::tempdir().unwrap();
        let launch = t("2027-01-15T10:00:00Z");
        let run = |out: &Outcome, schedule: &mut Schedule| {
            let ok = refusal(out).is_none();
            schedule.record(launch, ok, 0);
            ok
        };

        // Taken: the full wait.
        let (five, five_sig) = signed(&good_index(5), &keys.main);
        let taken = fetch_and_accept(&pair(&five, &five_sig), dir.path(), &ring)
            .await
            .unwrap();
        let mut s = Schedule::default();
        assert!(run(&taken, &mut s));
        assert!(!s.due(launch + chrono::Duration::hours(5)));
        // The same index again is a pass that worked.
        let again = fetch_and_accept(&pair(&five, &five_sig), dir.path(), &ring)
            .await
            .unwrap();
        assert!(refusal(&again).is_none());

        // A torn pair: the new index with the old signature. Refused at the signature, and retried
        // within minutes.
        let (six, _) = signed(&good_index(6), &keys.main);
        let torn = fetch_and_accept(&pair(&six, &five_sig), dir.path(), &ring)
            .await
            .unwrap();
        assert!(torn.verdict.is_err());
        let mut s = Schedule::default();
        assert!(!run(&torn, &mut s));
        assert!(s.due(launch + chrono::Duration::minutes(6)));
        assert!(matches!(refusal(&torn), Some(RefreshError::Refused(_))));
        assert!(!refusal(&torn).unwrap().is_quiet());

        // A replay (an older authentic index), a forgery and a record that can't be used are
        // failures too; the ones further failing back off in steps.
        let (old, old_sig) = signed(&good_index(4), &keys.main);
        let replay = fetch_and_accept(&pair(&old, &old_sig), dir.path(), &ring)
            .await
            .unwrap();
        assert!(matches!(
            replay.verdict,
            Ok(Verdict::Refused(Stale::Older { .. }))
        ));
        let mut s = Schedule::default();
        assert!(!run(&replay, &mut s));
        assert!(s.due(launch + chrono::Duration::minutes(6)));
        let stranger = Keys::new();
        let (forged, forged_sig) = signed(&good_index(9), &stranger.main);
        let forgery = fetch_and_accept(&pair(&forged, &forged_sig), dir.path(), &ring)
            .await
            .unwrap();
        assert!(refusal(&forgery).is_some());
        let damaged = tempfile::tempdir().unwrap();
        std::fs::write(damaged.path().join(index_state::STATE_FILE), b"{").unwrap();
        let unusable = fetch_and_accept(&pair(&five, &five_sig), damaged.path(), &ring)
            .await
            .unwrap();
        assert!(matches!(
            unusable.verdict,
            Ok(Verdict::Refused(Stale::RecordUnusable))
        ));
        assert!(refusal(&unusable).is_some());
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

    #[tokio::test]
    async fn a_damaged_record_is_found_before_any_download_and_retried_on_the_long_wait() {
        let keys = Keys::new();
        let ring = keys.ring();
        let eps = Endpoints::production();
        let launch = t("2027-01-15T10:00:00Z");
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(index_state::STATE_FILE), b"{").unwrap();
        // The script has no answers: any request panics ("no more answers"), so this passing
        // means nothing was fetched.
        let silent = Script(Mutex::new(VecDeque::new()));
        let err = fetch_pair_if_record_usable(&silent, &eps, dir.path(), &ring)
            .await
            .unwrap_err();
        assert!(matches!(err, RefreshError::RecordUnusable(_)), "{err}");
        assert!(err.is_quiet() && !err.counts_as_failure());
        assert!(record_fault_in(dir.path(), &ring).is_some());
        // Told to the schedule as a success: the next pass is six hours off, not five minutes.
        let mut s = Schedule::default();
        s.record(launch, schedule_ok(&Err(err)), 0);
        assert!(!s.due(launch + chrono::Duration::minutes(6)));
        assert!(!s.due(launch + chrono::Duration::hours(5)));
        assert!(s.due(launch + chrono::Duration::hours(7)));
        // Any other failure still backs off soon.
        let mut s = Schedule::default();
        s.record(
            launch,
            schedule_ok(&Err(RefreshError::Refused("torn".into()))),
            0,
        );
        assert!(s.due(launch + chrono::Duration::minutes(6)));

        // A record that is whole, or not there yet, fetches as before.
        let (bytes, sig) = signed(&good_index(5), &keys.main);
        for dir in [tempfile::tempdir().unwrap(), {
            let d = tempfile::tempdir().unwrap();
            state_at(d.path(), &ring, 5, &keys);
            d
        }] {
            let (i, s) = fetch_pair_if_record_usable(&pair(&bytes, &sig), &eps, dir.path(), &ring)
                .await
                .unwrap();
            assert_eq!((i, s), (bytes.clone(), sig.clone()));
            assert!(record_fault_in(dir.path(), &ring).is_none());
        }
    }

    /// The page's `ModsChangedCause` is exactly the causes this side sends, and every `CHANGED_*`
    /// constant is one of them, so a cause added here can't reach the page unnamed.
    #[test]
    fn the_pages_change_causes_are_the_constants_here() {
        let consts = [
            CHANGED_RECOVERED,
            CHANGED_INSTALLED,
            CHANGED_UNINSTALLED,
            CHANGED_PAUSED,
            CHANGED_RESUMED,
            CHANGED_CONSENTED,
            CHANGED_SAFETY,
            CHANGED_UPDATED,
            CHANGED_LOADED,
            CHANGED_DECIDED,
        ];
        let source = include_str!("refresh.rs");
        let declared = source
            .lines()
            .filter(|l| {
                l.starts_with("pub const CHANGED_") && !l.starts_with("pub const CHANGED_EVENT")
            })
            .count();
        assert_eq!(
            declared,
            consts.len(),
            "a CHANGED_* constant isn't in this list"
        );
        let mut here: Vec<String> = consts.iter().map(|c| (*c).to_owned()).collect();
        let mut page = crate::mods::tab::error::tests::ts_union("ModsChangedCause");
        here.sort();
        page.sort();
        assert_eq!(here, page);
        assert_eq!(CHANGED_EVENT, "mods://changed");
        assert_eq!(
            serde_json::to_value(ModsChanged {
                cause: CHANGED_SAFETY
            })
            .unwrap(),
            serde_json::json!({"cause": "safety"})
        );
    }

    /// A pass that stops at the record tells the page, so the strip appears without a restart; no
    /// other outcome does, and a damaged record is told apart from one that couldn't be read.
    #[test]
    fn only_a_pass_that_stopped_at_the_record_owes_the_page_a_notice() {
        let damaged = record_notice(&RefreshError::RecordUnusable("it is damaged".into())).unwrap();
        assert_eq!(
            damaged,
            ModNotice::RecordFault {
                unreadable: false,
                text: "it is damaged".into()
            }
        );
        let unread = record_notice(&RefreshError::RecordUnreadable("eio".into())).unwrap();
        assert!(matches!(
            unread,
            ModNotice::RecordFault {
                unreadable: true,
                ..
            }
        ));
        for other in [
            RefreshError::Fetch(FetchError::Offline("no route".into())),
            RefreshError::Local("no folder".into()),
            RefreshError::Refused("stale".into()),
        ] {
            assert!(record_notice(&other).is_none(), "{other:?}");
        }
        // And the wire shape the page reads.
        assert_eq!(
            serde_json::to_value(&damaged).unwrap(),
            serde_json::json!({"kind": "recordFault", "unreadable": false, "text": "it is damaged"})
        );
    }

    /// An I/O failure reading the record (both files, so no backup stands in) is not damage: the
    /// pass fetches nothing, but the retry is on the short back-off, not the six-hour wait.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_unreadable_record_is_retried_on_the_short_backoff() {
        use std::os::unix::fs::PermissionsExt;
        let keys = Keys::new();
        let ring = keys.ring();
        let eps = Endpoints::production();
        let launch = t("2027-01-15T10:00:00Z");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(index_state::STATE_FILE);
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&path).is_ok() {
            // Running as root: permissions can't make it unreadable here.
            return;
        }
        let silent = Script(Mutex::new(VecDeque::new()));
        let err = fetch_pair_if_record_usable(&silent, &eps, dir.path(), &ring)
            .await
            .unwrap_err();
        assert!(matches!(err, RefreshError::RecordUnreadable(_)), "{err}");
        assert!(err.is_quiet() && err.counts_as_failure());
        let mut s = Schedule::default();
        s.record(launch, schedule_ok(&Err(err)), 0);
        assert!(s.due(launch + chrono::Duration::minutes(6)));
        // The same file damaged rather than unreadable keeps the long wait.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(&path, b"{").unwrap();
        let err = fetch_pair_if_record_usable(&silent, &eps, dir.path(), &ring)
            .await
            .unwrap_err();
        assert!(matches!(err, RefreshError::RecordUnusable(_)), "{err}");
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
