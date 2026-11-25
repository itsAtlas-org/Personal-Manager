// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! One connection's life (Mods spec 1.16): who may connect, what a connection may send, how long it
//! may sit, and how it ends. Nothing here knows what a socket is: [`serve`] takes any pair of async
//! byte streams, so every rule is tested over `tokio::io::duplex`.
//!
//! **A connection earns nothing by connecting.** The first line must be PM's own auth frame,
//! `{"pm_auth":"<token>"}`, within [`Timing::auth`], or the connection is closed with no reply. The
//! frame is made by the `--mcp` shim from its environment, never from what the app's own client sends
//! (the client's lines only ever follow it). A token that doesn't verify closes the connection with no
//! word about why, so a guess learns nothing. Once verified the connection is [`Bound`] to its app and
//! the store it authenticated against, and [`jsonrpc::handle`] checks both again on every line.
//!
//! **Caps.** [`MAX_CONNECTIONS`] connections in all (one that hasn't authenticated yet counts: it is
//! the one an attacker would open) and [`MAX_PER_CLIENT`] for one app (known only after it
//! authenticates). A line is read into a buffer that never grows past [`MAX_LINE_BYTES`]: a longer one
//! closes the connection, the rest of it unread. A connection that sends nothing for [`Timing::idle`]
//! closes; the same allowance bounds one line's arrival and one answer's departure, so a client that
//! dribbles bytes or never reads cannot hold a slot.
//!
//! **Guessing is slow.** A well-shaped token this device never paired is turned away from the device's
//! own file before the store is touched, a failed one is held for [`Timing::fail`] before it is closed,
//! and [`AuthGate`] stops looking at attempts at all once too many have failed in a window.
//!
//! **Ending early.** Revoking an app closes its connections, and a lock, a vault switch or a closed
//! store closes all of them ([`Conns::close_client`], [`Conns::close_all`]). That is tidying: the
//! checks on every line are what stop a request ([`super`] module doc), and a connection that
//! somehow stayed open would still be refused.
//!
//! Nothing a client sent, and no text from the database or the disk, is ever put in a reply here: a
//! refusal is [`jsonrpc`]'s fixed sentence, and a protocol failure is a closed connection.

use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::watch;

use super::jsonrpc::{self, MAX_LINE_BYTES};
use super::{clients, here_of, open_and_unlocked, Bound};
use crate::mods::api::{blocking, Host, ModError};

/// Connections open at once, authenticated or not.
pub const MAX_CONNECTIONS: usize = 4;
/// Connections one app may hold.
pub const MAX_PER_CLIENT: usize = 2;
/// The longest the auth frame may be: a token is about 90 bytes.
const MAX_AUTH_BYTES: usize = 1024;

/// How long a connection may take over each step.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// From connecting to a complete auth frame.
    pub auth: Duration,
    /// Per line read, and per answer written.
    pub idle: Duration,
    /// How long a connection whose token failed to verify is held before it is closed.
    pub fail: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            auth: Duration::from_secs(5),
            idle: Duration::from_secs(30),
            fail: Duration::from_millis(250),
        }
    }
}

/// Failed authentications tolerated inside one [`AUTH_WINDOW`] before the next attempts are closed
/// unlooked-at.
pub const MAX_AUTH_FAILURES: u32 = 20;
/// The stretch [`MAX_AUTH_FAILURES`] is counted over.
const AUTH_WINDOW: Duration = Duration::from_secs(10);

/// How many wrong guesses at a token PM has taken lately. The caps bound how many connections are
/// open, not how fast they come and go, and each guess costs a file read and a turn at the store's
/// one guard: so a run of failures shuts the door for the rest of its window, whoever made them. A
/// right token is not counted, and a closed door is a closed connection (no word about why).
#[derive(Debug, Default)]
pub struct AuthGate {
    since: Option<Instant>,
    failures: u32,
}

impl AuthGate {
    /// Start a fresh window if the last has run out.
    fn roll(&mut self, now: Instant) {
        if self
            .since
            .is_none_or(|since| now.saturating_duration_since(since) >= AUTH_WINDOW)
        {
            self.since = Some(now);
            self.failures = 0;
        }
    }

    /// Whether an attempt at `now` may be looked at.
    pub fn open(&mut self, now: Instant) -> bool {
        self.roll(now);
        self.failures < MAX_AUTH_FAILURES
    }

    /// Note a failed attempt at `now`.
    pub fn failed(&mut self, now: Instant) {
        self.roll(now);
        self.failures = self.failures.saturating_add(1);
    }
}

/// The open connections: what the caps count and what a hook closes.
#[derive(Debug, Default)]
pub struct Conns {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    next: u64,
    entries: Vec<Entry>,
    gate: AuthGate,
}

#[derive(Debug)]
struct Entry {
    id: u64,
    client: Option<i64>,
    close: watch::Sender<bool>,
}

impl Conns {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A place for one more connection, or `None` when [`MAX_CONNECTIONS`] are open (the caller drops
    /// the connection at once).
    pub fn admit(self: &Arc<Self>) -> Option<Slot> {
        let mut inner = self.lock();
        if inner.entries.len() >= MAX_CONNECTIONS {
            return None;
        }
        inner.next += 1;
        let id = inner.next;
        let (close, closed) = watch::channel(false);
        inner.entries.push(Entry {
            id,
            client: None,
            close,
        });
        Some(Slot {
            conns: Arc::clone(self),
            id,
            closed,
        })
    }

    /// Close every connection (a lock, a vault switch, a closed store, quitting).
    pub fn close_all(&self) {
        for e in &self.lock().entries {
            let _ = e.close.send(true);
        }
    }

    /// Close one app's connections (it was revoked).
    pub fn close_client(&self, client: i64) {
        for e in &self.lock().entries {
            if e.client == Some(client) {
                let _ = e.close.send(true);
            }
        }
    }

    /// Connections open now.
    pub fn count(&self) -> usize {
        self.lock().entries.len()
    }
}

/// One connection's place in [`Conns`]; freed when dropped.
#[derive(Debug)]
pub struct Slot {
    conns: Arc<Conns>,
    id: u64,
    closed: watch::Receiver<bool>,
}

impl Slot {
    /// Whether a token may be looked at now ([`AuthGate`]).
    pub fn auth_open(&self) -> bool {
        self.conns.lock().gate.open(Instant::now())
    }

    /// Note that a token failed to verify ([`AuthGate`]).
    pub fn auth_failed(&self) {
        self.conns.lock().gate.failed(Instant::now());
    }

    /// Record that this connection authenticated as `client`. `false` when the app already holds
    /// [`MAX_PER_CLIENT`] other connections: this one must then be closed.
    pub fn bind_client(&self, client: i64) -> bool {
        let mut inner = self.conns.lock();
        let holding = inner
            .entries
            .iter()
            .filter(|e| e.client == Some(client) && e.id != self.id)
            .count();
        if holding >= MAX_PER_CLIENT {
            return false;
        }
        if let Some(entry) = inner.entries.iter_mut().find(|e| e.id == self.id) {
            entry.client = Some(client);
        }
        true
    }
}

/// Resolves when a hook closes the connection `closed` belongs to.
async fn wait_closed(mut closed: watch::Receiver<bool>) {
    // An `Err` is the sender gone, which only happens when the slot itself is.
    if closed.wait_for(|closed| *closed).await.is_err() {
        std::future::pending::<()>().await;
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.conns.lock().entries.retain(|e| e.id != self.id);
    }
}

/// One line as the reader found it.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    /// A complete line, without its newline (or a carriage return before it).
    Line(String),
    /// The stream ended (a half-written last line is dropped with it).
    Eof,
    /// The line passed its cap before a newline came; nothing past the cap was buffered.
    TooLong,
    /// A line that isn't UTF-8 text.
    NotText,
}

/// Read one line of at most `cap` bytes, never holding more than `cap` plus one read buffer.
pub async fn read_frame<R: AsyncBufRead + Unpin>(r: &mut R, cap: usize) -> io::Result<Frame> {
    let mut line: Vec<u8> = Vec::new();
    loop {
        let available = r.fill_buf().await?;
        if available.is_empty() {
            return Ok(Frame::Eof);
        }
        match available.iter().position(|b| *b == b'\n') {
            Some(end) => {
                if line.len() + end > cap {
                    return Ok(Frame::TooLong);
                }
                line.extend_from_slice(&available[..end]);
                r.consume(end + 1);
                break;
            }
            None => {
                if line.len() + available.len() > cap {
                    return Ok(Frame::TooLong);
                }
                line.extend_from_slice(available);
                let n = available.len();
                r.consume(n);
            }
        }
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Ok(match String::from_utf8(line) {
        Ok(text) => Frame::Line(text),
        Err(_) => Frame::NotText,
    })
}

/// PM's own first frame.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthFrame {
    pm_auth: String,
}

/// The token of an auth frame, or `None` for any other first line.
fn token_of(line: &str) -> Option<String> {
    serde_json::from_str::<AuthFrame>(line)
        .ok()
        .map(|f| f.pm_auth)
}

/// Verify `token` against the open vault and name the connection's binding: the app, and the store's
/// epoch read under the same DB guard as the check, so the two belong to one store. Blocking.
fn authenticate_blocking<H: Host>(host: &H, token: &str) -> Result<Bound, ModError> {
    // This device's own file first: a guess costs a small read, and the store's one guard (which
    // `open_and_unlocked` and the check below take, and every command of the app shares) only goes
    // to a token this device paired.
    let here = here_of(host)?;
    if !clients::pair_matches_token(&here, token) {
        return Err(ModError::locked());
    }
    open_and_unlocked(host)?;
    let conn = host.store().conn().map_err(|_| ModError::locked())?;
    let store_epoch = host.store().store_epoch();
    if !here.is_open_in(host.store()) {
        return Err(ModError::locked());
    }
    let client = clients::verify(&conn, &here, token).ok_or_else(ModError::locked)?;
    Ok(Bound {
        client: client.id,
        store_epoch,
    })
}

/// Serve one connection until it ends, is closed by a hook, or breaks a rule. Returns nothing: every
/// ending is a closed stream.
pub async fn serve<H: Host, S: AsyncRead + AsyncWrite + Unpin>(
    host: &H,
    stream: S,
    slot: Slot,
    timing: Timing,
) {
    tokio::select! {
        () = wait_closed(slot.closed.clone()) => {}
        () = session(host, stream, &slot, timing) => {}
    }
}

async fn session<H: Host, S: AsyncRead + AsyncWrite + Unpin>(
    host: &H,
    stream: S,
    slot: &Slot,
    timing: Timing,
) {
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::with_capacity(16 * 1024, read);
    // The first frame, and only PM's auth frame.
    let first = tokio::time::timeout(timing.auth, read_frame(&mut read, MAX_AUTH_BYTES)).await;
    let Ok(Ok(Frame::Line(line))) = first else {
        return;
    };
    let Some(token) = token_of(&line) else {
        return;
    };
    // A token that isn't shaped like one never costs the DB guard.
    if !clients::looks_like_token(&token) {
        return;
    }
    // Too many wrong guesses lately, from anyone: this one is not looked at.
    if !slot.auth_open() {
        return;
    }
    let bound = {
        let host = host.clone();
        match blocking(move || authenticate_blocking(&host, &token)).await {
            Ok(bound) => bound,
            Err(_) => {
                slot.auth_failed();
                // The guesser keeps its place for a moment (and the four places are all there are),
                // so a tight loop of them is a handful of attempts a second, not thousands.
                tokio::time::sleep(timing.fail).await;
                return;
            }
        }
    };
    if !slot.bind_client(bound.client) {
        return;
    }
    loop {
        let next = tokio::time::timeout(timing.idle, read_frame(&mut read, MAX_LINE_BYTES)).await;
        let Ok(Ok(Frame::Line(line))) = next else {
            return;
        };
        let Some(reply) = jsonrpc::handle(host, bound, &line).await else {
            continue;
        };
        let sent = tokio::time::timeout(timing.idle, async {
            write.write_all(reply.as_bytes()).await?;
            write.write_all(b"\n").await?;
            write.flush().await
        })
        .await;
        if !matches!(sent, Ok(Ok(()))) {
            return;
        }
    }
}
