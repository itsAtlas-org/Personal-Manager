// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The suspend handshake (Mods spec §1.5, §4; PR C, K5): PM's word to a frame before it takes the
//! frame down, and how long PM waits for the answer.
//!
//! A frame that debounces its saves (the Pinboard keeps edits 500 ms before it writes them) would
//! lose them if PM destroyed it on a tab switch, a pause, an uninstall, an update, a vault switch, the
//! app lock or a quit. So before any of those PM tells the frame (`lifecycle.suspend {reason}`, to the
//! one session, no permission needed), the frame saves through the bridge as it always does and
//! answers `{ flushed: true }` on its port, PM's page relays that ([`mod_frame_flushed`]), and PM then
//! goes on. **PM never waits past [`SUSPEND_DEADLINE`]**: a frame that is mute, frozen or hostile costs
//! the person that long and no more, and a quit is never held by it.
//!
//! **PM waits only for a mod that said it answers.** Its manifest's `answersSuspend: true` is that
//! promise (`package.rs`). A frame whose mod made no such promise (the Document Map 1.1.0, which has
//! nothing to save) is taken down at once: before this field, PM waited the whole deadline for an
//! answer that was never coming, on every tab switch and every quit. It is still told, best effort:
//! when PM waits for nobody, the tell is not awaited, so a change that commits first (a pause, an
//! uninstall) can make the emitter refuse it, and the frame hears only its session's end. A mod that
//! promises and then doesn't answer costs the deadline, as any mute frame does.
//!
//! The wait is Rust's, not the page's, so every path that takes frames down asks the same function
//! ([`suspend`]): the commands that pause, uninstall and update a mod call it before they change the
//! row, the page asks for it before a tab's frame retires ([`mod_frames_suspend`]) and before the vault
//! changes or the app locks, and a quit goes through [`quit`]. A `flushed` answer counts only for a
//! session PM is waiting on, so a frame cannot answer early for a suspend nobody asked for, and
//! another frame's answer never releases a different session's wait.
//!
//! **A suspend never waits for the person.** A question PM is asking for the frame (`mods::confirm`)
//! is revoked when its wait ends (at once, for a frame PM does not wait for), whatever the frame
//! answered; the dialog is modal, so a frame is not normally retired with one open, and a late yes
//! then does nothing.
//!
//! An emergency stop (the frozen-page watchdog, the safety list's removal) is not a suspend: a mod PM
//! has just stopped for misbehaving is not given more time to run.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use tauri::{AppHandle, Manager, Webview};
use tokio::sync::oneshot;

use super::api::events::{lifecycle_suspend, SuspendReason};
use super::api::Host;
use super::ids::ModId;
use super::runtime::{Audience, ModRuntime};
use crate::error::{Error, Result};

/// The longest PM waits for a frame's `flushed`, in all: one deadline for every frame it asked, not
/// one each. A frame that has not answered by then is taken down anyway.
pub const SUSPEND_DEADLINE: Duration = Duration::from_millis(1500);

/// One wait, so that a suspend ending takes down its own wait and no other (`forget`).
type WaitId = u64;
type Waits = HashMap<String, Vec<(WaitId, oneshot::Sender<()>)>>;

/// Who PM is waiting on: per session, the waits that a `flushed` releases.
#[derive(Default)]
pub struct Flushes {
    waiting: Mutex<Waits>,
    next: AtomicU64,
}

impl Flushes {
    fn lock(&self) -> MutexGuard<'_, Waits> {
        self.waiting.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Start waiting on `session`'s answer. Made before the frame is told, so an answer that comes
    /// back at once cannot be missed.
    fn expect(&self, session: &str) -> (WaitId, oneshot::Receiver<()>) {
        let (tx, rx) = oneshot::channel();
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.lock()
            .entry(session.to_owned())
            .or_default()
            .push((id, tx));
        (id, rx)
    }

    /// The frame of `session` answered `flushed`. Releases every wait on it; true when one was
    /// waiting. An answer nobody asked for does nothing.
    pub fn flushed(&self, session: &str) -> bool {
        let waits = self.lock().remove(session).unwrap_or_default();
        let any = !waits.is_empty();
        for (_, tx) in waits {
            let _ = tx.send(());
        }
        any
    }

    /// Stop waiting on `session` for the wait `id`: that wait ended, by an answer or by its deadline.
    /// Only that wait: another suspend of the same frame that is still inside its own deadline keeps
    /// its sender, so one that timed out early cannot cut a later one short.
    fn forget(&self, session: &str, id: WaitId) {
        let mut waits = self.lock();
        if let Some(list) = waits.get_mut(session) {
            list.retain(|(wait, _)| *wait != id);
            if list.is_empty() {
                waits.remove(session);
            }
        }
    }

    /// Sessions somebody is waiting on now (for the tests).
    #[cfg(test)]
    pub fn waiting_on(&self) -> usize {
        self.lock().len()
    }
}

/// How one suspend ended.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Suspended {
    /// Frames that answered `flushed` in time.
    pub flushed: usize,
    /// Frames PM told that did not answer by the deadline.
    pub timed_out: usize,
    /// Frames PM could not tell (their mod may not hear it: paused, or the app is locked): not waited on.
    pub unreachable: usize,
    /// Frames whose mod does not say it answers (`answersSuspend`): told, and never waited on.
    pub not_answering: usize,
}

/// Tell every live frame of `audience` that PM is about to take it down for `reason`, and wait until
/// each frame whose mod answers suspend (`answersSuspend`) has answered `flushed` or `deadline` has
/// passed, whichever comes first. Never fails and never waits longer than `deadline`; with no live
/// frame, or none whose mod answers, it returns at once.
pub async fn suspend<H: Host>(
    host: &H,
    audience: Audience,
    reason: SuspendReason,
    deadline: Duration,
) -> Suspended {
    let targets = host.runtime().suspend_targets(&audience);
    if targets.is_empty() {
        return Suspended::default();
    }
    // The deadline runs from here: telling a frame goes through the DB mutex (`emit_with`), and a
    // guard held for long must not push the end of the wait, or a quit, out with it.
    let end = tokio::time::Instant::now() + deadline;
    // Waiting first, telling second: an answer that races the emission is still caught.
    let waits: Vec<(String, WaitId, oneshot::Receiver<()>)> = targets
        .iter()
        .filter(|(_, answers)| *answers)
        .map(|(s, _)| {
            let (id, rx) = host.runtime().flushes().expect(s);
            (s.clone(), id, rx)
        })
        .collect();
    let mut out = Suspended {
        not_answering: targets.len() - waits.len(),
        ..Suspended::default()
    };
    let tell = {
        let host = host.clone();
        let sessions: Vec<String> = targets.iter().map(|(s, _)| s.clone()).collect();
        tokio::task::spawn_blocking(move || {
            sessions
                .iter()
                .map(|s| (s.clone(), lifecycle_suspend(&host, s, reason).sent > 0))
                .collect::<HashMap<_, _>>()
        })
    };
    // A frame PM will not wait for: its wait ends now, so its question to the person ends now too
    // (K1, as for the others below), and nothing of the rest is about it.
    for (session, _) in targets.iter().filter(|(_, answers)| !answers) {
        host.runtime().confirm_revoke(session);
    }
    if waits.is_empty() {
        // Nobody to wait for: the frames are told on the blocking thread (an event that comes after
        // the session has ended is refused, never delivered late), and PM goes on now.
        drop(tell);
        return out;
    }
    // Past the deadline the tell is given up on (the blocking thread finishes whenever the mutex
    // frees): no frame is known to be unreachable, so each counts as one that did not answer.
    let told = match tokio::time::timeout_at(end, tell).await {
        Ok(done) => Some(done.unwrap_or_default()),
        Err(_) => None,
    };
    let answered = futures_util::future::join_all(waits.into_iter().map(|(session, id, rx)| {
        let reachable = told
            .as_ref()
            .is_none_or(|told| told.get(&session).copied().unwrap_or(false));
        async move {
            if !reachable {
                return (session, id, None);
            }
            let answered = matches!(tokio::time::timeout_at(end, rx).await, Ok(Ok(())));
            (session, id, Some(answered))
        }
    }))
    .await;
    for (session, id, answer) in answered {
        host.runtime().flushes().forget(&session, id);
        // A suspend never waits for a question to the person (K1): whatever one this session holds
        // is revoked as the wait ends, so a frame about to go cannot be answered for by a late yes.
        host.runtime().confirm_revoke(&session);
        match answer {
            None => out.unreachable += 1,
            Some(true) => out.flushed += 1,
            Some(false) => out.timed_out += 1,
        }
    }
    out
}

/// [`suspend`] with PM's own deadline, for one mod's frames, from a command that is about to change
/// that mod's row (pause, uninstall, update).
pub async fn before_change(app: &AppHandle, id: &ModId, reason: SuspendReason) {
    if app.try_state::<ModRuntime>().is_none() {
        return;
    }
    suspend(app, Audience::Mod(id.clone()), reason, SUSPEND_DEADLINE).await;
}

/// [`before_change`] for a caller on a blocking thread (the consented install, which learns the mod's id
/// only once it has taken the staged package): the wait runs on the async runtime and this thread waits
/// for it on a channel, never past the deadline and a moment more, so a runtime that does not answer
/// cannot hold the install.
pub fn before_change_blocking(app: &AppHandle, id: &ModId, reason: SuspendReason) {
    if app
        .try_state::<ModRuntime>()
        .is_none_or(|runtime| !runtime.any_live())
    {
        return;
    }
    let (done, wait) = std::sync::mpsc::channel();
    let (app, id) = (app.clone(), id.clone());
    drop(tauri::async_runtime::spawn(async move {
        before_change(&app, &id, reason).await;
        let _ = done.send(());
    }));
    let _ = wait.recv_timeout(SUSPEND_DEADLINE + Duration::from_millis(500));
}

/// The page asks PM to tell the frames of `mod_id` (or every frame, with none) that they are about to
/// retire, and answers when those whose mod answers suspend have flushed or the deadline passed (at
/// once, when none of them does). `reason` is one of the words
/// the frame is told (`hidden`, `locked`, `paused`, `uninstalled`, `updated`, `vault`, `quit`).
#[tauri::command]
pub async fn mod_frames_suspend(
    webview: Webview,
    reason: String,
    mod_id: Option<String>,
) -> Result<()> {
    super::main_only(&webview)?;
    let reason = SuspendReason::from_word(&reason)
        .ok_or_else(|| Error::Other("That isn't a reason a frame can be told.".into()))?;
    let audience = match mod_id {
        Some(id) => Audience::Mod(ModId::parse(&id).map_err(|e| Error::Other(e.to_string()))?),
        None => Audience::Everyone,
    };
    let app = webview.app_handle().clone();
    if app.try_state::<ModRuntime>().is_none() {
        return Ok(());
    }
    suspend(&app, audience, reason, SUSPEND_DEADLINE).await;
    Ok(())
}

/// A frame's `{ flushed: true }`, relayed by PM's page. Counts only for a session PM asked.
#[tauri::command]
pub fn mod_frame_flushed(
    webview: Webview,
    runtime: tauri::State<'_, ModRuntime>,
    session: String,
) -> Result<()> {
    super::main_only(&webview)?;
    runtime.flushes().flushed(&session);
    Ok(())
}

/// Set once a quit has begun: a second request to quit (a double click on the tray's Quit, a close
/// while one is under way) leaves the first to finish, and never exits ahead of it.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// What a request to quit did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitRequest {
    /// No frame was running: PM exited at once.
    Exited,
    /// Frames are being asked to save; PM exits when those whose mod answers suspend have answered
    /// or the deadline passes (at once, when none of them does).
    Waiting,
    /// A quit is already under way; this request changed nothing.
    AlreadyWaiting,
}

/// Quit PM through `exit`, after giving every running frame its chance to save (`quit`), and never
/// longer than `deadline` (plus a short grace for the tell) for it. `quitting` is the flag a second
/// request finds set. Returns at once: the wait is on the async runtime, so the main thread keeps
/// serving the page the frames need to save through.
fn request_quit<H: Host>(
    host: &H,
    quitting: &'static AtomicBool,
    deadline: Duration,
    exit: impl FnOnce() + Send + 'static,
) -> QuitRequest {
    if quitting.swap(true, Ordering::SeqCst) {
        // The first request's wait ends within its deadline and then exits; exiting here would
        // skip it, and a double click on Quit would lose the frame's last edit.
        return QuitRequest::AlreadyWaiting;
    }
    if !host.runtime().any_live() {
        exit();
        return QuitRequest::Exited;
    }
    let host = host.clone();
    drop(tauri::async_runtime::spawn(async move {
        // `suspend` is bounded by `deadline` itself; this outer bound is the belt to its braces, so
        // that nothing in it can hold the quit.
        let _ = tokio::time::timeout(
            deadline + Duration::from_millis(500),
            suspend(&host, Audience::Everyone, SuspendReason::Quit, deadline),
        )
        .await;
        exit();
    }));
    QuitRequest::Waiting
}

/// Quit PM, after giving every running frame its chance to save (`quit`), and never longer than
/// [`SUSPEND_DEADLINE`] for it; only a frame whose mod says `answersSuspend` is waited for, so with
/// none of those running PM exits at once. Every way PM quits by itself (the tray's Quit, the main window's
/// close, the OS's own quit through [`exit_needs_wait`]) calls this, not `AppHandle::exit`.
pub fn quit(app: &AppHandle) {
    if app.try_state::<ModRuntime>().is_none() {
        QUITTING.store(true, Ordering::SeqCst);
        app.exit(0);
        return;
    }
    let exiting = app.clone();
    request_quit(app, &QUITTING, SUSPEND_DEADLINE, move || exiting.exit(0));
}

/// The OS asked PM to exit with no code (macOS Cmd+Q and Dock Quit, the last window gone): true when
/// the exit must be held back so [`quit`] can run, that is when a frame is running or a quit is
/// already waiting on one. A requested exit with a code is `quit`'s own and passes.
pub fn exit_needs_wait(app: &AppHandle) -> bool {
    QUITTING.load(Ordering::SeqCst)
        || app
            .try_state::<ModRuntime>()
            .is_some_and(|runtime| runtime.any_live())
}

#[cfg(test)]
mod tests;
