// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `PM --mcp`: the small program a connected app starts to talk to PM (Mods spec 1.16). The app's
//! MCP client runs it as a child and speaks the protocol on its stdin and stdout; this connects to the
//! socket of the PM that is already running, proves who it is, and then only carries bytes.
//!
//! It runs from `main`, before anything else in PM does: the single-instance plugin would otherwise
//! hand `--mcp` to the PM that is running and exit, and the window, the WebKit environment and every
//! other startup step are things a protocol pipe must never touch.
//!
//! **The token** comes from the environment variable `PM_MCP_TOKEN` of the client's own configuration.
//! It is read here and sent as PM's own first frame (`{"pm_auth":"<token>"}`, built by this program,
//! never taken from the client's lines), and is not written to argv, to a child's environment, to stdout
//! or to stderr.
//!
//! **If PM isn't running** it never starts PM. It reads the client's first message (so the answer can
//! carry its id), answers it with a fixed sentence as a JSON-RPC error and exits non-zero, which is
//! what a client shows its person.
//!
//! **PM may close a connection that served.** PM ends a connection that sat idle for 30 s, and every
//! lock does too, but a client that started this program expects it to stay for as long as the client
//! does (most clients never restart a stdio server that exited). So a close after answers is not an
//! end: the next line the client sends opens a fresh connection, authenticates it the same way and
//! goes down it. If that fails the line is answered with the fixed "PM isn't running" error (its id
//! echoed) and the program stays for the next one. Only a connection PM closes before saying anything
//! at all ends the program: the first one means the token was refused, the server is off or PM is
//! locked, and trying again would not change that. Because the program stays, it also owes the client
//! what a dying pipe used to give it: every request it forwarded on a connection is remembered until an
//! answer with that id goes by, and when PM closes the connection each one still unanswered is told
//! to the client as the fixed "PM closed the connection" error, so no call waits for a reply that can
//! no longer come.
//!
//! **Where the socket is** is found by working out PM's data folder the way PM does ([`paths`]'s one
//! rule, over this program's own base lookup), plus `mcp/mcp.sock`. No environment variable names the
//! socket: the only names a client sees are `--mcp` and `PM_MCP_TOKEN`.
//!
//! **Stdout is the protocol.** Nothing but PM's lines goes to it; logging, if any, is stderr.
//!
//! The pump is [`run`], over closures for the environment, the streams and the connection, so the
//! tests need no process and no socket. Windows: the release program is a GUI-subsystem executable
//! whose inherited standard streams are unverified, and the pipe isn't built (`mods::mcp::transport`):
//! there [`connect_real`] always fails, so the shim answers with the same fixed "PM isn't running"
//! sentence as when PM is closed (it does not say that Windows is unsupported).

use std::ffi::OsString;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use serde_json::{json, Value};

use crate::mods::mcp::jsonrpc::MAX_LINE_BYTES;
use crate::mods::mcp::transport::{DIR_NAME, SOCKET_NAME};

/// What the client is told when PM can't be reached. Fixed: no path, no operating system text.
const NOT_RUNNING: &str = "PM isn't running. Open PM and try again.";
/// What it is told when it has no token to present.
const NO_TOKEN: &str = "This app has no PM token. Connect the app again from PM's settings.";
/// What it is told when PM took the connection and closed it before answering.
const CLOSED: &str =
    "PM closed the connection. Check that this app is still connected in PM's settings.";
/// What it is told when the client sends a line past the cap.
const TOO_LARGE: &str = "The request is too large.";
/// JSON-RPC's implementation-defined range: PM refused.
const REFUSED: i64 = -32000;

/// The environment variable the client's configuration carries the token in.
pub const TOKEN_VAR: &str = "PM_MCP_TOKEN";

/// A connection to PM: what to read answers from, where to write requests, and how to say "no more
/// requests" so PM finishes the ones it has and closes.
pub struct Link {
    pub read: Box<dyn Read + Send>,
    pub write: Box<dyn Write + Send>,
    pub finish: Box<dyn FnOnce() + Send>,
}

/// The base folder the OS keeps per-user data in, from `env` (the shim runs before Tauri exists, so
/// it asks the environment the same way Tauri's own lookup does).
fn local_data_base(env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |v: OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
    #[cfg(target_os = "linux")]
    {
        env("XDG_DATA_HOME").and_then(absolute).or_else(|| {
            env("HOME")
                .and_then(absolute)
                .map(|h| h.join(".local/share"))
        })
    }
    #[cfg(target_os = "macos")]
    {
        env("HOME")
            .and_then(absolute)
            .map(|h| h.join("Library/Application Support"))
    }
    #[cfg(windows)]
    {
        env("LOCALAPPDATA").and_then(absolute)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = (env, absolute);
        None
    }
}

/// PM's data folder as the shim finds it: [`crate::paths::resolve_data_dir`] over the environment
/// (the `PM_DATA_DIR` override counts in a debug build only, like PM's own).
fn data_dir(env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let dev = if cfg!(debug_assertions) {
        env("PM_DATA_DIR")
    } else {
        None
    };
    crate::paths::resolve_data_dir(dev, || local_data_base(env).ok_or(())).ok()
}

/// Where PM's socket is, for this environment.
pub(crate) fn socket_path(env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    data_dir(env).map(|d| d.join(DIR_NAME).join(SOCKET_NAME))
}

/// One line as the shim's reader found it.
enum Line {
    Text(Vec<u8>),
    Eof,
    TooLong,
}

/// Read a line of at most `cap` bytes (its newline not counted), never holding more.
fn read_line_capped<R: BufRead>(r: &mut R, cap: usize) -> io::Result<Line> {
    let mut line = Vec::new();
    loop {
        let available = r.fill_buf()?;
        if available.is_empty() {
            return Ok(Line::Eof);
        }
        match available.iter().position(|b| *b == b'\n') {
            Some(end) => {
                if line.len() + end > cap {
                    return Ok(Line::TooLong);
                }
                line.extend_from_slice(&available[..end]);
                r.consume(end + 1);
                return Ok(Line::Text(line));
            }
            None => {
                if line.len() + available.len() > cap {
                    return Ok(Line::TooLong);
                }
                line.extend_from_slice(available);
                let n = available.len();
                r.consume(n);
            }
        }
    }
}

/// The id of a request line: its string or integer id, else `null` (garbage, a notification). A line
/// with no method that carries a result or an error is a response, which PM ignores and never
/// answers, so it is filed as nothing (it would be told as closed, or pile up, for ever).
fn id_of(line: &[u8]) -> Option<Value> {
    let v: Value = serde_json::from_slice(line).ok()?;
    if v.get("method").is_none() && (v.get("result").is_some() || v.get("error").is_some()) {
        return None;
    }
    let id = v.get("id")?;
    (id.is_string() || id.is_i64() || id.is_u64()).then(|| id.clone())
}

fn error_line(id: &Value, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": REFUSED, "message": message}}).to_string()
}

/// stdout, shared by the answer pump and the early error answers.
struct Out<W: Write>(Mutex<W>);

impl<W: Write> Out<W> {
    fn line(&self, text: &str) {
        let mut w = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = w.write_all(text.as_bytes());
        let _ = w.write_all(b"\n");
        let _ = w.flush();
    }

    fn bytes(&self, bytes: &[u8]) -> io::Result<()> {
        let mut w = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        w.write_all(bytes)?;
        w.flush()
    }
}

/// What the main loop waits for: the client's lines (read on a thread of their own, so a blocked read
/// of stdin never holds up an answer or the exit) and the news that a connection is not worth keeping
/// the program for.
enum Event {
    Line(Vec<u8>),
    TooLong,
    StdinEnd,
    Failed,
}

/// The requests sent down one connection that no answer has come back for, and whether the connection
/// is over. One lock holds both, so a request can't be filed after the answer thread has told the
/// client about the rest (it would wait for ever): the filing is refused and the caller moves to a
/// fresh connection instead.
#[derive(Default)]
struct Pending {
    ids: Vec<Value>,
    closed: bool,
}

impl Pending {
    /// File a request's id. `false` when the connection is already over.
    fn add(&mut self, id: &Value) -> bool {
        if self.closed {
            return false;
        }
        self.ids.push(id.clone());
        true
    }

    /// Strike an id off. `false` when it wasn't there (answered already, or told as closed).
    fn remove(&mut self, id: &Value) -> bool {
        match self.ids.iter().position(|p| p == id) {
            Some(at) => {
                self.ids.swap_remove(at);
                true
            }
            None => false,
        }
    }
}

type SharedPending = Arc<Mutex<Pending>>;

#[cfg(test)]
type AfterOpen = Box<dyn FnMut(&SharedPending)>;

#[cfg(test)]
thread_local! {
    /// Test hook, run on the shim's own thread between a connection being ready and a request being
    /// filed on it, so a test can let PM's close land in exactly that window.
    static AFTER_OPEN: std::cell::RefCell<Option<AfterOpen>> =
        const { std::cell::RefCell::new(None) };
}

fn lock_pending(pending: &SharedPending) -> std::sync::MutexGuard<'_, Pending> {
    pending.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The most of one answer line kept to read its id from. PM's lines are bounded well under this; a
/// longer one is still forwarded, but its id is not read, so the close then tells nothing (better
/// silent than a second answer for a request that did get one).
const MAX_TRACKED_LINE: usize = 16 << 20;

/// The id of the answer in `line`, when it is one (a result or an error carrying an id).
fn answer_id(line: &[u8]) -> Option<Value> {
    let v: Value = serde_json::from_slice(line).ok()?;
    if v.get("method").is_some() {
        return None;
    }
    let id = v.get("id")?;
    (id.is_string() || id.is_i64() || id.is_u64()).then(|| id.clone())
}

/// Reads the ids of the answers going by on a byte stream that arrives in pieces, striking each off the
/// pending set.
struct AnswerIds {
    line: Vec<u8>,
    /// The current line outgrew [`MAX_TRACKED_LINE`].
    overran: bool,
    /// Some line went by whose id could not be read, so the pending set may hold answered ids.
    lost_track: bool,
}

impl AnswerIds {
    fn new() -> Self {
        Self {
            line: Vec::new(),
            overran: false,
            lost_track: false,
        }
    }

    fn feed(&mut self, mut bytes: &[u8], pending: &SharedPending) {
        while !bytes.is_empty() {
            let end = bytes.iter().position(|b| *b == b'\n');
            let piece = &bytes[..end.unwrap_or(bytes.len())];
            if !self.overran {
                if self.line.len() + piece.len() > MAX_TRACKED_LINE {
                    self.overran = true;
                    self.lost_track = true;
                    self.line = Vec::new();
                } else {
                    self.line.extend_from_slice(piece);
                }
            }
            let Some(end) = end else { break };
            self.end_line(pending);
            bytes = &bytes[end + 1..];
        }
    }

    /// The current line is over: strike its id off if it is an answer, and start the next one.
    fn end_line(&mut self, pending: &SharedPending) {
        if !self.overran {
            if let Some(id) = answer_id(&self.line) {
                lock_pending(pending).remove(&id);
            }
        }
        self.line.clear();
        self.overran = false;
    }

    /// PM has closed. A line it began and did not finish is read as it stands, since a whole answer with
    /// no newline after it is still an answer and must not be told again as closed. `true` when there
    /// was such a line (the caller ends it on stdout).
    fn close(&mut self, pending: &SharedPending) -> bool {
        let half = self.half_line();
        self.end_line(pending);
        half
    }

    /// Bytes of a line PM began and did not finish.
    fn half_line(&self) -> bool {
        !self.line.is_empty() || self.overran
    }
}

/// One connection to PM, and the thread that carries its answers to stdout.
struct Conn {
    write: Box<dyn Write + Send>,
    finish: Option<Box<dyn FnOnce() + Send>>,
    /// The requests sent down it still waiting for an answer; marked closed by the answer thread once
    /// PM has closed (or the connection broke).
    pending: SharedPending,
    answers: JoinHandle<()>,
}

impl Conn {
    /// Whether PM has closed it.
    fn is_closed(&self) -> bool {
        lock_pending(&self.pending).closed
    }

    /// Say no more requests are coming and wait for PM to finish the ones it has and close.
    fn end(mut self) {
        if let Some(finish) = self.finish.take() {
            finish();
        }
        let _ = self.answers.join();
    }
}

/// PM's answers to stdout until PM closes. Then every request still waiting on this connection is
/// told to the client as [`CLOSED`] with its own id (a connection PM closed without a word leaves the
/// request that opened it among them); if it was the first connection and it said nothing the program
/// is also done, since the token being refused will not change.
fn carry_answers<W: Write + Send + 'static>(
    mut read: Box<dyn Read + Send>,
    out: &Out<W>,
    pending: &SharedPending,
    first: bool,
    failed: &AtomicBool,
    events: &SyncSender<Event>,
) {
    let mut received = 0usize;
    let mut buf = [0u8; 8 * 1024];
    let mut ids = AnswerIds::new();
    // stdout gone: nobody is left to answer.
    let mut stdout_gone = false;
    loop {
        match read.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                received += n;
                if out.bytes(&buf[..n]).is_err() {
                    stdout_gone = true;
                    break;
                }
                ids.feed(&buf[..n], pending);
            }
        }
    }
    // Before the leftovers are taken: an answer PM sent without its newline strikes its id off.
    let half_line = ids.close(pending);
    // Over and the leftovers taken in one step, so a request filed a moment later is refused (and
    // moved to a fresh connection) rather than lost.
    let unanswered = {
        let mut p = lock_pending(pending);
        p.closed = true;
        std::mem::take(&mut p.ids)
    };
    // PM closing without a word means the token was refused, the server was switched off, or PM
    // locked. It does not say which, and neither does this.
    if !stdout_gone {
        if half_line {
            // Whatever PM began and didn't finish must not run into the next line written, whether
            // that is told here or comes from the next connection.
            out.line("");
        }
        // A line past the cap was not read, so an id it answered may still be pending: tell nothing.
        if !ids.lost_track {
            for id in &unanswered {
                out.line(&error_line(id, CLOSED));
            }
        }
    }
    if stdout_gone || (received == 0 && first) {
        failed.store(true, Ordering::SeqCst);
        // A full queue means the main loop is busy and looks at `failed` itself next; a gone loop has
        // nothing left to wake.
        let _ = events.try_send(Event::Failed);
    }
}

/// The shim's side of the conversation: the connection it has now (if any) and what it takes to make
/// another.
struct Shim<W: Write + Send + 'static, C> {
    out: Arc<Out<W>>,
    token: String,
    path: PathBuf,
    connect: C,
    events: SyncSender<Event>,
    failed: Arc<AtomicBool>,
    conn: Option<Conn>,
    /// Whether any connection has been made yet (the first one's silent close ends the program).
    connected_before: bool,
}

impl<W, C> Shim<W, C>
where
    W: Write + Send + 'static,
    C: FnMut(&Path) -> io::Result<Link>,
{
    /// Connect and send PM's own first frame.
    fn open(&mut self) -> io::Result<()> {
        let Link {
            read,
            mut write,
            finish,
        } = (self.connect)(&self.path)?;
        let auth = json!({ "pm_auth": self.token }).to_string();
        write
            .write_all(auth.as_bytes())
            .and_then(|()| write.write_all(b"\n"))
            .and_then(|()| write.flush())?;
        let pending = SharedPending::default();
        let first = !self.connected_before;
        self.connected_before = true;
        let answers = {
            let out = Arc::clone(&self.out);
            let pending = Arc::clone(&pending);
            let failed = Arc::clone(&self.failed);
            let events = self.events.clone();
            std::thread::spawn(move || {
                carry_answers(read, &out, &pending, first, &failed, &events);
            })
        };
        self.conn = Some(Conn {
            write,
            finish: Some(finish),
            pending,
            answers,
        });
        Ok(())
    }

    /// Send one client line to PM, on a fresh connection if PM has closed the last one. A request is
    /// filed as pending before its bytes go, so an answer can never beat the filing. When the line
    /// can't be delivered, the client is told with the line's own id. `false` then.
    fn send(&mut self, line: &[u8]) -> bool {
        let id = id_of(line);
        // What the client is told if neither try delivers: a connection PM took and closed under the
        // request is the more telling thing to report, so once seen it stays.
        let mut told = NOT_RUNNING;
        // A second try only for a connection that turned out dead under the first write.
        for _ in 0..2 {
            if self.conn.as_ref().is_some_and(Conn::is_closed) {
                if let Some(old) = self.conn.take() {
                    old.end();
                }
            }
            if self.conn.is_none() && self.open().is_err() {
                break;
            }
            let Some(conn) = self.conn.as_mut() else {
                break;
            };
            #[cfg(test)]
            AFTER_OPEN.with(|hook| {
                if let Some(f) = hook.borrow_mut().as_mut() {
                    f(&conn.pending);
                }
            });
            let filed = id
                .as_ref()
                .is_none_or(|id| lock_pending(&conn.pending).add(id));
            if !filed {
                told = CLOSED;
            } else {
                let sent = conn
                    .write
                    .write_all(line)
                    .and_then(|()| conn.write.write_all(b"\n"))
                    .and_then(|()| conn.write.flush());
                if sent.is_ok() {
                    return true;
                }
                // The write failed. If the answer thread already told the client this request was
                // lost, it has been answered: sending it again elsewhere would answer it twice.
                if id
                    .as_ref()
                    .is_some_and(|id| !lock_pending(&conn.pending).remove(id))
                {
                    return true;
                }
            }
            if let Some(dead) = self.conn.take() {
                dead.end();
            }
        }
        self.out.line(&error_line(&id.unwrap_or(Value::Null), told));
        false
    }
}

/// Run the shim over `stdin` and `stdout`, finding the token in `env` and the connection through
/// `connect` (given the socket's path; called again whenever PM has closed the last one and the
/// client sends another line). Returns the process's exit code: 0 after a connection that served, 1
/// for every refusal.
pub fn run<R, W, E, C>(env: E, mut stdin: R, stdout: W, connect: C) -> i32
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
    E: Fn(&str) -> Option<OsString>,
    C: FnMut(&Path) -> io::Result<Link>,
{
    let out = Arc::new(Out(Mutex::new(stdout)));
    // The first message, read before anything is sent, so every early refusal can echo its id.
    let first = match read_line_capped(&mut stdin, MAX_LINE_BYTES) {
        Ok(Line::Text(line)) => line,
        Ok(Line::TooLong) => {
            out.line(&error_line(&Value::Null, TOO_LARGE));
            return 1;
        }
        // A client that sent nothing has nothing to be told.
        Ok(Line::Eof) | Err(_) => return 0,
    };
    let reply_id = id_of(&first).unwrap_or(Value::Null);

    let Some(token) = env(TOKEN_VAR)
        .and_then(|t| t.into_string().ok())
        .filter(|t| !t.is_empty())
    else {
        out.line(&error_line(&reply_id, NO_TOKEN));
        return 1;
    };
    let Some(path) = socket_path(&env) else {
        out.line(&error_line(&reply_id, NOT_RUNNING));
        return 1;
    };
    // A few lines can queue while PM is slow to take them; past that the thread reading stdin waits,
    // as a plain pipe would.
    let (events, queue) = sync_channel(8);
    let failed = Arc::new(AtomicBool::new(false));
    let mut shim = Shim {
        out: Arc::clone(&out),
        token,
        path,
        connect,
        events: events.clone(),
        failed: Arc::clone(&failed),
        conn: None,
        connected_before: false,
    };
    // The first message goes down the first connection: not being able to is the end of the program.
    if !shim.send(&first) {
        return 1;
    }

    let too_large = Arc::new(AtomicBool::new(false));
    {
        let too_large = Arc::clone(&too_large);
        std::thread::spawn(move || loop {
            let event = match read_line_capped(&mut stdin, MAX_LINE_BYTES) {
                Ok(Line::Text(line)) => Event::Line(line),
                // Told by `run` once the connection's answer thread is done: an answer thread
                // may be half way through a line of PM's, and this thread writing now would run
                // the error into it.
                Ok(Line::TooLong) => {
                    too_large.store(true, Ordering::SeqCst);
                    Event::TooLong
                }
                Ok(Line::Eof) | Err(_) => Event::StdinEnd,
            };
            let last = !matches!(event, Event::Line(_));
            if events.send(event).is_err() || last {
                break;
            }
        });
    }

    // The client's lines, one at a time, until it stops, breaks a rule or PM can't be kept.
    while let Ok(event) = queue.recv() {
        match event {
            Event::Line(line) => {
                // A line that can't be delivered was answered inside `send`.
                shim.send(&line);
            }
            Event::TooLong | Event::StdinEnd | Event::Failed => break,
        }
        if failed.load(Ordering::SeqCst) {
            break;
        }
    }

    // No more requests: PM finishes the ones it has, and its answers are all on stdout before this
    // returns.
    if let Some(conn) = shim.conn.take() {
        conn.end();
    }
    // Every answer thread is over, and each ended any half line of PM's, so this is a whole line of
    // its own.
    let too_large = too_large.load(Ordering::SeqCst);
    if too_large {
        out.line(&error_line(&Value::Null, TOO_LARGE));
    }
    i32::from(too_large || failed.load(Ordering::SeqCst))
}

/// The real connection: PM's socket.
#[cfg(unix)]
pub(crate) fn connect_real(path: &Path) -> io::Result<Link> {
    use std::net::Shutdown;
    use std::os::unix::net::UnixStream;
    let stream = UnixStream::connect(path)?;
    let read = stream.try_clone()?;
    let finisher = stream.try_clone()?;
    Ok(Link {
        read: Box::new(read),
        write: Box::new(stream),
        finish: Box::new(move || {
            let _ = finisher.shutdown(Shutdown::Write);
        }),
    })
}

/// Windows has no connection to make yet (module doc).
#[cfg(not(unix))]
pub(crate) fn connect_real(_path: &Path) -> io::Result<Link> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

/// What the environment says for `name`, except that a missing (or empty) `HOME` is asked of the
/// passwd entry through `passwd`: Tauri's own lookup (the `dirs` crate) does that, and a client that
/// starts this program with a scrubbed environment (a launcher, a sandbox, a systemd unit) must still
/// find the folder PM found.
fn with_home_fallback(
    name: &str,
    found: Option<OsString>,
    passwd: impl FnOnce() -> Option<PathBuf>,
) -> Option<OsString> {
    let missing = found.as_ref().is_none_or(|v| v.is_empty());
    if name == "HOME" && missing {
        passwd().map(PathBuf::into_os_string).or(found)
    } else {
        found
    }
}

/// `PM --mcp`'s whole body. The exit code is the process's.
pub fn run_mcp_shim() -> i32 {
    run(
        |name| {
            with_home_fallback(name, std::env::var_os(name), || {
                // On Unix this reads `HOME` and then the passwd entry, as `dirs` does.
                std::env::home_dir()
            })
        },
        io::BufReader::new(io::stdin()),
        io::stdout(),
        connect_real,
    )
}

#[cfg(test)]
mod tests;
