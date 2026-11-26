// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! When the socket exists (Mods spec 1.16): it is open while, and only while, this PM is the one
//! writing an open vault whose device file says the server is on, and it goes with that vault.
//!
//! [`Lifecycle`] is the one listener PM may have. [`Lifecycle::reconcile`] makes it match what is true
//! now (the vault open, whether this instance writes it, the device's switch) and is the single way
//! one starts, so every path that opens, switches or restores a vault ends in the same question.
//! A restored or copied vault asks the device file, finds no entry for its folder, and comes up off.
//!
//! **The hooks are tidying, not the guarantee.** A store closing, the app locking, an app being
//! revoked or PM quitting close the connections it concerns at once ([`Lifecycle::stop`],
//! [`Lifecycle::close_all`], [`Lifecycle::close_client`]), but every request checks the store, the lock
//! and the app again ([`super::dispatch`]), so a hook that never ran costs a connection that lingers
//! and is refused. That is why nothing here is load-bearing for who may read what.
//!
//! **Quitting.** `process::exit` follows `RunEvent::Exit`, so no destructor ever runs: [`shutdown`]
//! unlinks the socket and the pid file explicitly, and only if they are still the ones this run made
//! ([`super::transport::Claim`]).
//!
//! The entry points taking an `AppHandle` are thin: they find the [`Supervisor`](super::Supervisor)
//! (absent in a test that built no mods state: then nothing happens) and call the generic methods,
//! which the tests drive with the API's test host.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tokio::runtime::Handle;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::device;
use super::serve::{serve, Conns, Timing};
use super::transport::{Claim, Listener, PlatformTransport, StartError, Transport};
use crate::mods::api::Host;
use crate::mods::devmode::Here;

/// Which vault, in which folder, a listener was started for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Key {
    vault: String,
    place: String,
}

/// A listener that is up.
#[derive(Debug)]
struct Running {
    key: Key,
    claim: Claim,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl Running {
    /// Bind and start accepting. `handle` is the runtime the listener and its connections run on.
    fn start<H: Host>(
        host: H,
        handle: &Handle,
        data_dir: &Path,
        conns: Arc<Conns>,
        timing: Timing,
        key: Key,
    ) -> Result<Self, StartError> {
        // The listener registers with the runtime when it is made.
        let _enter = handle.enter();
        let listener = PlatformTransport::new(data_dir).bind()?;
        let claim = listener.claim();
        let (stop, stopped) = oneshot::channel();
        let task = handle.spawn(accept_loop(host, listener, conns, timing, stopped));
        Ok(Self {
            key,
            claim,
            stop: Some(stop),
            task,
        })
    }

    fn alive(&self) -> bool {
        !self.task.is_finished()
    }

    /// Stop accepting and take the socket and the pid file away. Synchronous: it never waits for the
    /// task, whose listener is dropped when the runtime next runs it.
    fn end(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.task.abort();
        self.claim.release();
    }
}

async fn accept_loop<H: Host>(
    host: H,
    listener: Listener,
    conns: Arc<Conns>,
    timing: Timing,
    mut stopped: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = &mut stopped => return,
            accepted = listener.accept() => match accepted {
                Ok(stream) => match conns.admit() {
                    Some(slot) => {
                        let host = host.clone();
                        tokio::spawn(async move { serve(&host, stream, slot, timing).await });
                    }
                    // Full: the connection is closed unanswered.
                    None => drop(stream),
                },
                // A failed accept (out of descriptors, say) must not spin.
                Err(_) => tokio::time::sleep(Duration::from_millis(200)).await,
            }
        }
    }
}

/// What the Settings screen shows of the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub listening: bool,
    /// PM's sentence for why the last start failed.
    pub error: Option<StartError>,
}

/// The one listener PM may have, and the connections it has accepted.
#[derive(Debug, Default)]
pub struct Lifecycle {
    server: Mutex<Option<Running>>,
    conns: Arc<Conns>,
    fault: Mutex<Option<StartError>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Lifecycle {
    pub fn status(&self) -> Status {
        Status {
            listening: lock(&self.server).as_ref().is_some_and(Running::alive),
            error: *lock(&self.fault),
        }
    }

    /// Make the listener match what is true now: up for the open vault when `writes` (this instance
    /// holds the store) and this device's file says on, else down. A listener already up for the same
    /// vault and folder is left alone, with its connections.
    ///
    /// What is wanted is read AFTER the server lock is taken: a [`stop`](Self::stop) racing this (a
    /// hand-off or the curtain closing the store on another thread) either ran first, so the store
    /// is already gone from what is read here, or runs after and takes down what this starts. Read
    /// before the lock, an answer from before the close could start a listener for a closed store
    /// that nothing would then tear down. Nothing under the lock waits on anything slow: the
    /// start's probe of a leftover socket never blocks (`transport::probe`), so a stalled foreign
    /// socket cannot hold up `status`, `stop` or a quit.
    pub fn reconcile<H: Host>(&self, host: &H, handle: &Handle, writes: bool) {
        let mut server = lock(&self.server);
        let wanted = wanted(host, writes);
        if let (Some(running), Some(key)) = (server.as_ref(), wanted.as_ref()) {
            if running.key == *key && running.alive() {
                return;
            }
        }
        // Anything running is for another vault, or is dead: its connections go with it.
        if let Some(running) = server.take() {
            running.end();
        }
        self.conns.close_all();
        *lock(&self.fault) = None;
        let Some(key) = wanted else {
            return;
        };
        match Running::start(
            host.clone(),
            handle,
            host.mcp().data_dir(),
            Arc::clone(&self.conns),
            Timing::default(),
            key,
        ) {
            Ok(running) => *server = Some(running),
            Err(e) => *lock(&self.fault) = Some(e),
        }
    }

    /// Connections open now, for the tests that wait on one ending or filling (`lifecycle_tests`,
    /// which are Unix-only).
    #[cfg(all(test, unix))]
    pub fn open_connections(&self) -> usize {
        self.conns.count()
    }

    /// Close every connection and take the listener down (the store closed, PM is quitting).
    pub fn stop(&self) {
        self.conns.close_all();
        if let Some(running) = lock(&self.server).take() {
            running.end();
        }
        *lock(&self.fault) = None;
    }

    /// Close every connection and leave the listener (the app locked: it stays, and refuses).
    pub fn close_all(&self) {
        self.conns.close_all();
    }

    /// Close one app's connections (it was revoked).
    pub fn close_client(&self, client: i64) {
        self.conns.close_client(client);
    }
}

/// The vault the listener should be up for, if any.
fn wanted<H: Host>(host: &H, writes: bool) -> Option<Key> {
    wanted_if(host, writes, super::commands::offered())
}

/// [`wanted`] with `offered` given. A build that does not offer connected apps never listens, whatever
/// the device file says: the file is shared with builds that do (a debug build on the same data
/// folder), and the hidden section could neither show the server running nor switch it off.
pub(super) fn wanted_if<H: Host>(host: &H, writes: bool, offered: bool) -> Option<Key> {
    if !offered || !writes || crate::paths::data_dir_is_purged() {
        return None;
    }
    let (vault, root) = host.store().active_vault_place()?;
    let here = Here::open(host.mcp().data_dir(), &vault, &root).ok()?;
    device::device_state(&here).on.then(|| Key {
        vault,
        place: here.place().to_owned(),
    })
}

/// The runtime the app's async work runs on.
fn runtime() -> Handle {
    tauri::async_runtime::handle().inner().clone()
}

fn lifecycle(app: &AppHandle) -> Option<&Lifecycle> {
    app.try_state::<super::Supervisor>()
        .map(|state| state.inner().life())
}

/// The lock session has settled and `writes` says whether this instance holds the open store
/// (`ledger::after_settle`): bring the listener in line.
pub fn after_settle(app: &AppHandle, writes: bool) {
    if let Some(life) = lifecycle(app) {
        life.reconcile(app, &runtime(), writes);
    }
}

/// The switch, a pairing or the vault changed (Settings): bring the listener in line, asking the
/// session whether this instance writes the store.
pub fn after_open(app: &AppHandle) {
    let state = app.state::<crate::AppState>();
    let writes = crate::lock_session::this_instance_writes(state.inner()) && state.is_unlocked();
    after_settle(app, writes);
}

/// The store closed under this instance: nothing is served, and no connection stays.
pub fn on_store_closed(app: &AppHandle) {
    if let Some(life) = lifecycle(app) {
        life.stop();
    }
}

/// The app locked: every connection ends (and a new one is refused until it is unlocked).
pub fn on_lock(app: &AppHandle) {
    if let Some(life) = lifecycle(app) {
        life.close_all();
    }
}

/// An app was revoked: its connections end.
pub fn on_revoked(app: &AppHandle, client: i64) {
    if let Some(life) = lifecycle(app) {
        life.close_client(client);
    }
}

/// PM is going away (or a wipe is about to remove the folder): connections end and the socket and
/// pid file are removed by hand, since no destructor runs after `RunEvent::Exit`.
pub fn shutdown(app: &AppHandle) {
    if let Some(life) = lifecycle(app) {
        life.stop();
    }
}
