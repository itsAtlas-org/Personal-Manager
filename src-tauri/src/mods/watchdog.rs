// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The frozen-page watchdog (Mods spec §1.4, plan decision 5; slice 6, part 4).
//!
//! **The problem.** On an engine that runs a mod's sealed frame on the same thread as PM's own page
//! (WebKitGTK measured it in the sealed-frame spike, build log: a busy frame froze the main page), a mod that spins
//! can make PM's whole page stop answering, and nothing inside the page can notice, because the
//! page is what is stuck.
//!
//! **The check.** While a mod frame is mounted, PM's page sends a heartbeat to Rust every
//! [`BEAT_EVERY`] (`mods_heartbeat`, a cheap command, main window only, started and stopped with
//! the ModFrame mounts, `src/lib/modWatchdog.ts`). A tick in Rust every [`TICK`] asks [`Watch::tick`]
//! whether a frame is mounted, and if no heartbeat has come for [`FROZEN_AFTER`] it says the page
//! is frozen. Then Rust pauses the mod whose frame was last active ([`culprit`]: the last bridge
//! call, or the frame's opening) with the reason `frozen` (`store::REASON_FROZEN`, "stopped
//! responding", lifted only by the user's own resume) and reloads the main webview, which ends
//! every session. On Linux the web process is ended first: reloading a page whose thread a mod
//! holds would wait for the very thread that is stuck.
//!
//! **What must not count as frozen.** The page's timers are the heartbeat, and a page that is not on
//! screen has them throttled, so the clock is tied to the page being seen and to PM itself running:
//! - *a page that is hidden*, as it said in its last heartbeat (`visible`, sent at once when the
//!   page's visibility changes) or as the window says (minimised, or not shown): the clock restarts
//!   when it is shown again, so a window that comes back starts a fresh [`FROZEN_AFTER`];
//! - *no frame mounted*: nothing to blame, and the clock starts when one is;
//! - *a suspend, a stall, or a clock set forward*: a tick that finds more than [`GAP`] between itself
//!   and the one before (on either the monotonic or the wall clock, since each is blind to some of
//!   the ways a machine sleeps) gives the page a fresh [`FROZEN_AFTER`] instead of a verdict.
//!
//! A verdict also restarts the clock, so a reload that takes a while isn't judged again at once.
//!
//! **Everything here that decides is pure and tested** ([`Watch`], [`culprit`]); the glue that reads
//! the runtime, pauses a row and reloads a webview is small, and on a platform other than Linux
//! untested (it can't be compiled here): see the build log.

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use tauri::{AppHandle, Manager, Webview};

use super::ids::ModId;
use super::ledger::{self, Mint};
use super::runtime::{ModRuntime, MountedFrame};
use super::tab::commands::check_window;
use super::tab::error::Result as ModsResult;
use super::MAIN_LABEL;

/// How often the page sends a heartbeat (the page's own constant, `BEAT_MS` in `modWatchdog.ts`,
/// mirrors this one). Rust never waits on it, only on [`FROZEN_AFTER`]: it is here so the margin
/// between the two is pinned by a test.
#[cfg_attr(not(test), allow(dead_code))]
pub const BEAT_EVERY: Duration = Duration::from_secs(2);
/// How long without a heartbeat, with a frame mounted and the page on screen, is "frozen".
pub const FROZEN_AFTER: Duration = Duration::from_secs(10);
/// How often Rust looks.
pub const TICK: Duration = Duration::from_secs(1);
/// A gap between two ticks past this means PM wasn't running for part of it (a suspend, a stall).
pub const GAP: Duration = Duration::from_secs(5);

/// A reading of both clocks, as a time since an arbitrary start (the monotonic clock) and since the
/// Unix epoch (the wall clock).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clock {
    pub mono: Duration,
    pub wall: Duration,
}

/// What a tick found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Fine,
    Frozen,
}

/// The watchdog's state: when the page last proved it was alive, whether it said it was visible, and
/// the last tick (pure; every input is a parameter).
#[derive(Debug, Clone)]
pub struct Watch {
    /// When the clock that [`FROZEN_AFTER`] measures started: the last heartbeat, or the last moment
    /// nothing could be judged.
    started: Duration,
    /// What the page's last heartbeat said about being on screen.
    visible: bool,
    last_tick: Option<Clock>,
}

impl Watch {
    pub fn new(now: Clock) -> Self {
        Self {
            started: now.mono,
            visible: true,
            last_tick: Some(now),
        }
    }

    /// The page's heartbeat: it is alive, and `visible` is whether it is on screen.
    pub fn beat(&mut self, now: Clock, visible: bool) {
        self.started = now.mono;
        self.visible = visible;
    }

    /// The window has the user's focus, so the page is on screen whatever its last heartbeat said:
    /// a page that was hidden and is frozen before it could say it is visible again would otherwise
    /// never be judged. The clock isn't touched, so the verdict is still a full threshold after the
    /// last moment nothing could be judged.
    pub fn assume_visible(&mut self) {
        self.visible = true;
    }

    /// One look. `mounted`: a mod frame is mounted. `shown`: the window is shown and not minimised.
    pub fn tick(&mut self, now: Clock, mounted: bool, shown: bool) -> Verdict {
        let slept = self.last_tick.is_some_and(|prev| {
            // Either clock can be the one that saw it: a suspend stops the monotonic clock on some
            // systems and not on others, and a wall clock set forward moves only itself. Going
            // backwards is nothing (it is a gap of zero).
            let mono = now.mono.saturating_sub(prev.mono);
            let wall = now.wall.saturating_sub(prev.wall);
            mono > GAP || wall > GAP
        });
        self.last_tick = Some(now);
        // Nothing to judge: a fresh start, so the first moment it can be judged is a full
        // FROZEN_AFTER away.
        if slept || !mounted || !shown || !self.visible {
            self.started = now.mono;
            return Verdict::Fine;
        }
        if now.mono.saturating_sub(self.started) >= FROZEN_AFTER {
            // Judged once: the reload that follows gets its own FROZEN_AFTER.
            self.started = now.mono;
            return Verdict::Frozen;
        }
        Verdict::Fine
    }
}

/// The frame to blame (pure): the one that was last active (its last bridge call, or its opening),
/// the most recently opened of those, then the smallest id, so the answer never depends on the order
/// the registry lists them in. `None` when no frame is mounted.
pub fn culprit(frames: &[MountedFrame]) -> Option<ModId> {
    frames
        .iter()
        .max_by(|a, b| {
            a.touched
                .cmp(&b.touched)
                .then(a.opened.cmp(&b.opened))
                // The smaller id wins a tie, so it is the "greater" under max_by.
                .then(b.mod_id.as_str().cmp(a.mod_id.as_str()))
        })
        .map(|f| f.mod_id.clone())
}

// ---- The managed state, the command and the loop ---------------------------------------------

/// The watchdog's state, managed by `setup()`.
pub struct Watchdog {
    start: Instant,
    watch: Mutex<Watch>,
}

impl Default for Watchdog {
    fn default() -> Self {
        let start = Instant::now();
        let wall = wall_now();
        Self {
            start,
            watch: Mutex::new(Watch::new(Clock {
                mono: Duration::ZERO,
                wall,
            })),
        }
    }
}

fn wall_now() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

impl Watchdog {
    fn clock(&self) -> Clock {
        Clock {
            mono: self.start.elapsed(),
            wall: wall_now(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Watch> {
        self.watch.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The page's heartbeat.
    pub fn beat(&self, visible: bool) {
        let now = self.clock();
        self.lock().beat(now, visible);
    }

    fn tick(&self, mounted: bool, shown: bool, focused: bool) -> Verdict {
        let now = self.clock();
        let mut watch = self.lock();
        if focused {
            watch.assume_visible();
        }
        watch.tick(now, mounted, shown)
    }
}

/// The page is alive: sent every [`BEAT_EVERY`] while a mod frame is mounted, and at once when the
/// page's visibility changes. `visible` is whether the page is on screen. Main window only. Cheap:
/// no store, no lock but the watchdog's own.
#[tauri::command]
pub fn mods_heartbeat(webview: Webview, visible: bool) -> ModsResult<()> {
    check_window(webview.label())?;
    if let Some(dog) = webview.app_handle().try_state::<Watchdog>() {
        dog.beat(visible);
    }
    Ok(())
}

/// Start the loop: a tick every [`TICK`] for the life of the app.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(TICK).await;
            let (Some(dog), Some(runtime)) =
                (app.try_state::<Watchdog>(), app.try_state::<ModRuntime>())
            else {
                continue;
            };
            let mounted = !runtime.mounted_frames().is_empty();
            let (shown, focused) = main_window(&app);
            if dog.tick(mounted, shown, focused) == Verdict::Frozen {
                let app = app.clone();
                // Rows and files: off the async runtime.
                tauri::async_runtime::spawn_blocking(move || {
                    recover(&app, "the page stopped responding")
                });
            }
        }
    });
}

/// What the OS says of the main window: `(shown, focused)`. Shown is visible and not minimised; a
/// window PM can't ask is taken to be shown (the page's own `visible` still counts), and not
/// focused.
fn main_window(app: &AppHandle) -> (bool, bool) {
    let Some(w) = app.get_webview_window(MAIN_LABEL) else {
        return (false, false);
    };
    (
        w.is_visible().unwrap_or(true) && !w.is_minimized().unwrap_or(false),
        w.is_focused().unwrap_or(false),
    )
}

/// The page is frozen, or its renderer died: pause the mod whose frame was last active, then reload
/// the main webview. Blocking.
fn recover(app: &AppHandle, why: &str) {
    if let Some(runtime) = app.try_state::<ModRuntime>() {
        if let Some(id) = culprit(&runtime.mounted_frames()) {
            eprintln!("mods: {why}; pausing {id}, the last mod active");
            let paused =
                ledger::with_active_ledger(app, Mint::IfRows, |l| l.pause_frozen(&id, Utc::now()));
            if !matches!(paused, Some(Ok(_))) {
                // The row couldn't be written (the store is closed or busy): the mod is still taken
                // out of the registry for this run, so the reload can't mount it again. The next
                // launch serves it as the row says.
                eprintln!("mods: {id}'s pause couldn't be recorded; it is off until PM restarts");
                runtime.pause(&id);
            }
            super::refresh::emit_changed(app, super::refresh::CHANGED_PAUSED);
        }
    }
    reload_pages(app);
}

/// Reload PM's pages. On Linux the web process is ended first (every PM window shares one, so the
/// pop-out Calendar is reloaded too): a reload asks the page's own thread, which is the one stuck.
fn reload_pages(app: &AppHandle) {
    #[cfg(target_os = "linux")]
    if let Some(main) = app.get_webview_window(MAIN_LABEL) {
        let _ = main.with_webview(|webview| {
            use webkit2gtk::WebViewExt;
            webview.inner().terminate_web_process();
        });
    }
    for (_, window) in app.webview_windows() {
        if let Err(e) = window.reload() {
            eprintln!("mods: a page couldn't be reloaded ({e})");
        }
    }
}

/// macOS (and iOS): the web content process died (`Builder::on_web_content_process_terminate`), which
/// a mod that crashes the renderer can cause: the same recovery as a frozen page. Only the main
/// window's. Not compiled on Linux (and so untested here).
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub fn on_renderer_terminated(webview: &tauri::Webview) {
    if webview.label() != MAIN_LABEL {
        return;
    }
    let app = webview.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || recover(&app, "the page's renderer ended"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> Clock {
        Clock {
            mono: Duration::from_secs(secs),
            wall: Duration::from_secs(1_000_000 + secs),
        }
    }

    /// A watch started at 0 whose page has beaten at 0, ticked once a second to `until` (a page
    /// that beats `every` seconds, if it does at all), with the given facts about the world.
    /// Returns the first second a verdict of `Frozen` came, if any.
    fn run(
        until: u64,
        beats_every: Option<u64>,
        mounted: impl Fn(u64) -> bool,
        shown: impl Fn(u64) -> bool,
        visible: impl Fn(u64) -> bool,
    ) -> Option<u64> {
        let mut w = Watch::new(at(0));
        for s in 1..=until {
            if beats_every.is_some_and(|e| s % e == 0) {
                w.beat(at(s), visible(s));
            }
            if w.tick(at(s), mounted(s), shown(s)) == Verdict::Frozen {
                return Some(s);
            }
        }
        None
    }

    const ALWAYS: fn(u64) -> bool = |_| true;

    #[test]
    fn a_page_that_keeps_beating_is_never_frozen() {
        assert_eq!(run(600, Some(2), ALWAYS, ALWAYS, ALWAYS), None);
        // Even a slow beat, as long as it is inside the threshold.
        assert_eq!(run(600, Some(9), ALWAYS, ALWAYS, ALWAYS), None);
    }

    #[test]
    fn a_page_that_stops_beating_with_a_frame_mounted_is_frozen_after_ten_seconds() {
        // The last beat was at 0: the tick that sees ten seconds is the one at 10.
        assert_eq!(run(60, None, ALWAYS, ALWAYS, ALWAYS), Some(10));
        // Beats that stop at 20: frozen at 30, not before.
        let mut w = Watch::new(at(0));
        for s in 1..=20 {
            w.beat(at(s), true);
            assert_eq!(w.tick(at(s), true, true), Verdict::Fine);
        }
        for s in 21..30 {
            assert_eq!(w.tick(at(s), true, true), Verdict::Fine, "{s}");
        }
        assert_eq!(w.tick(at(30), true, true), Verdict::Frozen);
    }

    #[test]
    fn nothing_mounted_is_never_frozen_and_the_clock_starts_when_a_frame_does() {
        assert_eq!(run(600, None, |_| false, ALWAYS, ALWAYS), None);
        // An hour with no frame and no beats, then a frame mounts at 3600: the page gets a full
        // threshold from then (the clock last restarted at the tick before, 3599), not an instant
        // verdict.
        let mounted = |s| s >= 3600;
        assert_eq!(run(4000, None, mounted, ALWAYS, ALWAYS), Some(3599 + 10));
    }

    #[test]
    fn a_hidden_page_or_window_is_never_frozen_and_the_clock_restarts_when_shown() {
        // The page said it is hidden (its timers are throttled, so no beats), for a long time.
        let mut w = Watch::new(at(0));
        w.beat(at(1), false);
        for s in 2..300 {
            assert_eq!(w.tick(at(s), true, true), Verdict::Fine, "{s}");
        }
        // It becomes visible and beats at once: a clean slate.
        w.beat(at(300), true);
        for s in 301..310 {
            assert_eq!(w.tick(at(s), true, true), Verdict::Fine, "{s}");
        }
        assert_eq!(w.tick(at(310), true, true), Verdict::Frozen);

        // The window itself is minimised or not shown (the page can't say so if it is frozen): the
        // same, and the first verdict is a threshold after it is shown again (the clock last
        // restarted at the tick before, 499).
        let shown = |s: u64| !(5..500).contains(&s);
        assert_eq!(run(900, None, ALWAYS, shown, ALWAYS), Some(499 + 10));

        // A page that said hidden and froze before it could say it is visible again: once the
        // window has the user's focus it is judged, a full threshold after the last tick that
        // couldn't judge it.
        let mut w = Watch::new(at(0));
        w.beat(at(1), false);
        for s in 2..100 {
            assert_eq!(w.tick(at(s), true, true), Verdict::Fine, "{s}");
        }
        w.assume_visible();
        for s in 100..109 {
            assert_eq!(w.tick(at(s), true, true), Verdict::Fine, "{s}");
        }
        assert_eq!(w.tick(at(109), true, true), Verdict::Frozen);
    }

    #[test]
    fn a_suspend_or_a_stall_gives_the_page_a_fresh_threshold_on_either_clock() {
        // The machine slept for an hour: the wall clock jumped, the monotonic one didn't (Linux).
        let mut w = Watch::new(at(0));
        w.beat(at(1), true);
        w.tick(at(2), true, true);
        let slept_wall = Clock {
            mono: Duration::from_secs(3),
            wall: at(3).wall + Duration::from_secs(3600),
        };
        assert_eq!(w.tick(slept_wall, true, true), Verdict::Fine);
        // The page wakes up and beats; or it doesn't, and it gets a full threshold, no less.
        let mut now = slept_wall;
        for _ in 0..9 {
            now.mono += Duration::from_secs(1);
            now.wall += Duration::from_secs(1);
            assert_eq!(w.tick(now, true, true), Verdict::Fine);
        }
        now.mono += Duration::from_secs(1);
        now.wall += Duration::from_secs(1);
        assert_eq!(w.tick(now, true, true), Verdict::Frozen);

        // The monotonic clock jumped and the wall clock didn't (macOS and Windows count differently,
        // or the process was stopped): the same.
        let mut w = Watch::new(at(0));
        w.tick(at(1), true, true);
        let stalled = Clock {
            mono: Duration::from_secs(1) + Duration::from_secs(120),
            wall: at(2).wall,
        };
        assert_eq!(w.tick(stalled, true, true), Verdict::Fine);
        // Just under the gap is an ordinary late tick: no grace is given for it.
        let mut w = Watch::new(at(0));
        assert_eq!(w.tick(at(5), true, true), Verdict::Fine);
        assert_eq!(w.tick(at(10), true, true), Verdict::Frozen);
        // The wall clock set back changes nothing (a gap of zero, not an underflow or a grace).
        let mut w = Watch::new(at(0));
        for s in 1..10 {
            assert_eq!(w.tick(at(s), true, true), Verdict::Fine);
        }
        let back = Clock {
            mono: Duration::from_secs(10),
            wall: Duration::from_secs(5),
        };
        assert_eq!(w.tick(back, true, true), Verdict::Frozen);
    }

    #[test]
    fn a_verdict_restarts_the_clock_so_the_reload_is_not_judged_at_once() {
        let mut w = Watch::new(at(0));
        for s in 1..10 {
            assert_eq!(w.tick(at(s), true, true), Verdict::Fine, "{s}");
        }
        assert_eq!(w.tick(at(10), true, true), Verdict::Frozen);
        for s in 11..20 {
            assert_eq!(w.tick(at(s), true, true), Verdict::Fine, "{s}");
        }
        assert_eq!(w.tick(at(20), true, true), Verdict::Frozen);
    }

    fn frame(id: &str, base: Instant, opened: u64, touched: u64) -> MountedFrame {
        MountedFrame {
            mod_id: ModId::parse(id).unwrap(),
            opened: base + Duration::from_secs(opened),
            touched: base + Duration::from_secs(touched),
        }
    }

    #[test]
    fn the_frame_last_active_is_blamed_then_the_latest_opened_then_the_smallest_id() {
        let t = Instant::now();
        assert_eq!(culprit(&[]), None);
        assert_eq!(
            culprit(&[frame("acme.one", t, 0, 0)]),
            Some(ModId::parse("acme.one").unwrap())
        );
        // The last call wins over an older opening.
        let frames = [
            frame("acme.one", t, 5, 50),
            frame("acme.two", t, 1, 20),
            frame("acme.three", t, 9, 9),
        ];
        assert_eq!(culprit(&frames).unwrap().as_str(), "acme.one");
        // Equal calls: the one opened later.
        let frames = [frame("acme.one", t, 1, 5), frame("acme.two", t, 3, 5)];
        assert_eq!(culprit(&frames).unwrap().as_str(), "acme.two");
        // Everything equal: the smallest id, in either listing order.
        let a = frame("acme.apple", t, 1, 1);
        let b = frame("acme.zebra", t, 1, 1);
        assert_eq!(
            culprit(&[a.clone(), b.clone()]).unwrap().as_str(),
            "acme.apple"
        );
        assert_eq!(culprit(&[b, a]).unwrap().as_str(), "acme.apple");
    }

    #[test]
    fn the_beat_is_cheaper_than_the_threshold_by_a_wide_margin() {
        // A page needs to miss several beats in a row before it is frozen.
        assert!(FROZEN_AFTER >= BEAT_EVERY * 4);
        assert!(GAP > TICK * 2);
    }

    #[test]
    fn the_heartbeat_is_a_main_window_command_that_takes_no_store() {
        let source = include_str!("watchdog.rs");
        let production = source.split("#[cfg(test)]\nmod tests").next().unwrap();
        let body = production
            .split("#[tauri::command]")
            .nth(1)
            .unwrap()
            .split("\n}\n")
            .next()
            .unwrap();
        assert!(body.contains("pub fn mods_heartbeat(webview: Webview"));
        assert!(body.contains("check_window(webview.label())"));
        assert!(!body.contains("state.conn") && !body.contains("AppState"));
        assert!(!"mods_heartbeat".starts_with("set_"));
        let lib = include_str!("../lib.rs");
        assert!(lib.contains("mods::watchdog::mods_heartbeat,"));
        // The loop is started and the state managed, so a registered command has something to beat.
        assert!(lib.contains("mods::watchdog::spawn("));
        assert!(lib.contains("mods::watchdog::Watchdog::default()"));
    }
}
