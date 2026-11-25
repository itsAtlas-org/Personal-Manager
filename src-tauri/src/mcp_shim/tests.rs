// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `PM --mcp` without a process: the pump over closures for the environment, the streams and the
//! connection, plus one run over a real socket.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::{
    run, socket_path, with_home_fallback, Link, SharedPending, AFTER_OPEN, CLOSED,
    MAX_TRACKED_LINE, NOT_RUNNING, NO_TOKEN, TOKEN_VAR, TOO_LARGE,
};
use crate::mods::mcp::jsonrpc::MAX_LINE_BYTES;

const TOKEN: &str = "pmcp_7_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Bytes written, readable by the test after the shim has them.
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Shared {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }

    fn lines(&self) -> Vec<String> {
        self.text().lines().map(str::to_owned).collect()
    }
}

/// PM's end as the shim sees it: some canned answer, then silence until the shim says it is done
/// (as the real server does, finishing what it has when the client stops), then end of stream.
struct Scripted {
    answer: Cursor<Vec<u8>>,
    done: mpsc::Receiver<()>,
    asked: Shared,
}

/// A real PM answers a request it has read, never before: hold a canned answer back until the shim has
/// written as many requests (lines with a method) as the answer has lines.
fn wait_for_request(answer: &Cursor<Vec<u8>>, asked: &Shared) {
    if answer.position() == 0 && !answer.get_ref().is_empty() {
        let wanted = answer.get_ref().iter().filter(|b| **b == b'\n').count();
        wait_until("the requests to answer", || {
            asked
                .lines()
                .iter()
                .filter(|l| l.contains("\"method\""))
                .count()
                >= wanted
        });
    }
}

impl Read for Scripted {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        wait_for_request(&self.answer, &self.asked);
        let n = self.answer.read(buf)?;
        if n > 0 {
            return Ok(n);
        }
        let _ = self.done.recv();
        Ok(0)
    }
}

/// A connection answering `answer`, and what the shim wrote to it.
fn link(answer: &str) -> (Link, Shared) {
    let (finish, done) = mpsc::channel();
    let written = Shared::default();
    let link = Link {
        read: Box::new(Scripted {
            answer: Cursor::new(answer.as_bytes().to_vec()),
            done,
            asked: written.clone(),
        }),
        write: Box::new(written.clone()),
        finish: Box::new(move || {
            let _ = finish.send(());
        }),
    };
    (link, written)
}

/// PM's end of a connection that answers and then closes by itself, as an idle timeout or a lock does.
/// `handled` is set when the shim's answer thread has dealt with the close completely: the reader is
/// dropped as that thread's last act, so this is a signal, not a guess at how long it takes.
fn closing_link(answer: &str, handled: &Arc<AtomicBool>) -> (Link, Shared) {
    let written = Shared::default();
    let link = Link {
        read: Box::new(Wire {
            wait_for: answer.bytes().filter(|b| *b == b'\n').count(),
            bytes: Cursor::new(answer.as_bytes().to_vec()),
            hold: None,
            asked: written.clone(),
            waited: false,
            handled: Arc::clone(handled),
        }),
        write: Box::new(written.clone()),
        finish: Box::new(|| {}),
    };
    (link, written)
}

/// PM's end of a connection, scripted: it waits to have been sent `wait_for` requests, says `bytes`,
/// then (with `hold`) stays open until released, and closes. Dropped (the answer thread's last act)
/// it sets `handled`.
struct Wire {
    wait_for: usize,
    bytes: Cursor<Vec<u8>>,
    hold: Option<mpsc::Receiver<()>>,
    asked: Shared,
    waited: bool,
    handled: Arc<AtomicBool>,
}

impl Read for Wire {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.waited {
            self.waited = true;
            let (asked, wanted) = (self.asked.clone(), self.wait_for);
            wait_until("the requests PM waits for", move || {
                asked
                    .lines()
                    .iter()
                    .filter(|l| l.contains("\"method\""))
                    .count()
                    >= wanted
            });
        }
        let n = self.bytes.read(buf)?;
        if n > 0 {
            return Ok(n);
        }
        if let Some(hold) = &self.hold {
            let _ = hold.recv();
        }
        Ok(0)
    }
}

impl Drop for Wire {
    fn drop(&mut self) {
        self.handled.store(true, Ordering::SeqCst);
    }
}

/// What a breaking connection runs when a write fails, given the means to let the read side end.
type OnFail = Box<dyn FnMut(&mpsc::Sender<()>) + Send>;

/// The shim's side of a connection that breaks: it takes `limit` bytes in all, then every write fails
/// with a broken pipe (after running `on_fail`, which is given the means to let the read side end and
/// may wait for the shim to have noticed).
struct Breaking {
    seen: Shared,
    limit: usize,
    taken: usize,
    release: mpsc::Sender<()>,
    on_fail: OnFail,
}

impl Write for Breaking {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let room = self.limit - self.taken;
        if room == 0 {
            (self.on_fail)(&self.release);
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        }
        let n = room.min(buf.len());
        self.taken += n;
        self.seen.write(&buf[..n])
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A scripted connection and the handles a test keeps on it.
struct Pm {
    link: Link,
    /// What the shim wrote to it.
    written: Shared,
    /// Set once the shim has dealt with PM's close of it.
    handled: Arc<AtomicBool>,
    /// Lets a held connection close.
    release: mpsc::Sender<()>,
}

impl Pm {
    /// PM says `answer` once it has been sent `wait_for` requests, then closes by itself.
    fn says(answer: impl AsRef<[u8]>, wait_for: usize) -> Self {
        Self::build(answer.as_ref(), wait_for, false, None)
    }

    /// The same, but it stays open until `release` is used (or the shim finishes its side).
    fn holds(answer: impl AsRef<[u8]>, wait_for: usize) -> Self {
        Self::build(answer.as_ref(), wait_for, true, None)
    }

    /// A held connection whose writes break after `limit` bytes, running `on_fail` first.
    fn breaks_after(limit: usize, on_fail: impl FnMut(&mpsc::Sender<()>) + Send + 'static) -> Self {
        Self::build(b"", 0, true, Some((limit, Box::new(on_fail))))
    }

    fn build(
        answer: &[u8],
        wait_for: usize,
        hold: bool,
        breaking: Option<(usize, OnFail)>,
    ) -> Self {
        let (release, held) = mpsc::channel();
        let written = Shared::default();
        let handled = Arc::new(AtomicBool::new(false));
        let write: Box<dyn Write + Send> = match breaking {
            Some((limit, on_fail)) => Box::new(Breaking {
                seen: written.clone(),
                limit,
                taken: 0,
                release: release.clone(),
                on_fail,
            }),
            None => Box::new(written.clone()),
        };
        let finish_release = release.clone();
        Self {
            link: Link {
                read: Box::new(Wire {
                    wait_for,
                    bytes: Cursor::new(answer.to_vec()),
                    hold: hold.then_some(held),
                    asked: written.clone(),
                    waited: false,
                    handled: Arc::clone(&handled),
                }),
                write,
                finish: Box::new(move || {
                    let _ = finish_release.send(());
                }),
            },
            written,
            handled,
            release,
        }
    }

    fn handled(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.handled)
    }
}

/// A connector that hands out `links` in turn and refuses once they are used up.
fn each(links: Vec<Link>) -> impl FnMut(&Path) -> io::Result<Link> {
    let mut links = links.into_iter();
    move |_| {
        links
            .next()
            .ok_or_else(|| io::Error::from(io::ErrorKind::ConnectionRefused))
    }
}

fn once(link: Link) -> impl FnMut(&Path) -> io::Result<Link> {
    each(vec![link])
}

/// The client's stdin, a line at a time as the test sends them; closing the sender is end of input.
struct Typed {
    lines: mpsc::Receiver<Vec<u8>>,
    pending: Cursor<Vec<u8>>,
}

impl Read for Typed {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let n = self.pending.read(buf)?;
            if n > 0 {
                return Ok(n);
            }
            match self.lines.recv() {
                Ok(next) => self.pending = Cursor::new(next),
                Err(_) => return Ok(0),
            }
        }
    }
}

fn typed() -> (mpsc::Sender<Vec<u8>>, io::BufReader<Typed>) {
    let (send, lines) = mpsc::channel();
    let stdin = Typed {
        lines,
        pending: Cursor::new(Vec::new()),
    };
    (send, io::BufReader::new(stdin))
}

/// Wait (briefly, for real) until `ready` holds.
fn wait_until(what: &str, ready: impl Fn() -> bool) {
    for _ in 0..500 {
        if ready() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("never happened: {what}");
}

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    move |name| map.get(name).map(OsString::from)
}

/// The usual environment: a token and a home to find the data folder from.
fn usual() -> impl Fn(&str) -> Option<OsString> {
    env(&[
        (TOKEN_VAR, TOKEN),
        ("HOME", "/home/someone"),
        ("XDG_DATA_HOME", "/xdg"),
    ])
}

fn reply(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn when_pm_is_not_running_the_request_is_answered_with_its_id_and_nothing_is_launched() {
    for (line, id) in [
        (
            r#"{"jsonrpc":"2.0","id":7,"method":"initialize"}"#,
            Value::from(7),
        ),
        (
            r#"{"jsonrpc":"2.0","id":"abc","method":"ping"}"#,
            Value::from("abc"),
        ),
        ("this is not json", Value::Null),
        (
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            Value::Null,
        ),
        (
            r#"{"jsonrpc":"2.0","id":{"nested":1},"method":"ping"}"#,
            Value::Null,
        ),
    ] {
        let out = Shared::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let code = run(
            usual(),
            Cursor::new(format!("{line}\n").into_bytes()),
            out.clone(),
            move |_path: &Path| -> io::Result<Link> {
                counted.fetch_add(1, Ordering::SeqCst);
                Err(io::Error::from(io::ErrorKind::ConnectionRefused))
            },
        );
        assert_eq!(code, 1, "{line}");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "connected once, no retry");
        let lines = out.lines();
        assert_eq!(lines.len(), 1);
        let answer = reply(&lines[0]);
        assert_eq!(answer["jsonrpc"], "2.0");
        assert_eq!(answer["id"], id, "{line}");
        assert_eq!(answer["error"]["code"], -32000);
        assert_eq!(answer["error"]["message"], NOT_RUNNING);
        assert_eq!(NOT_RUNNING, "PM isn't running. Open PM and try again.");
        assert!(!out.text().contains(TOKEN));
    }
}

#[test]
fn without_a_token_it_never_connects() {
    for environment in [
        env(&[("HOME", "/h")]),
        env(&[(TOKEN_VAR, ""), ("HOME", "/h")]),
    ] {
        let out = Shared::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let code = run(
            environment,
            Cursor::new(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n".to_vec()),
            out.clone(),
            move |_: &Path| -> io::Result<Link> {
                counted.fetch_add(1, Ordering::SeqCst);
                Err(io::Error::other("unreachable"))
            },
        );
        assert_eq!(code, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let answer = reply(&out.lines()[0]);
        assert_eq!(answer["id"], 1);
        assert_eq!(answer["error"]["message"], NO_TOKEN);
    }
}

#[test]
fn the_auth_frame_is_the_shims_own_and_the_clients_lines_follow_it() {
    let (connection, written) = link("{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}\n");
    let out = Shared::default();
    let seen_path = Arc::new(Mutex::new(PathBuf::new()));
    let remember = Arc::clone(&seen_path);
    let mut connection = Some(connection);
    // The client tries to authenticate itself: its line is only ever forwarded, after PM's.
    let stdin = concat!(
        "{\"pm_auth\":\"pmcp_1_forged\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n"
    );
    let code = run(
        usual(),
        Cursor::new(stdin.as_bytes().to_vec()),
        out.clone(),
        move |p: &Path| {
            *remember.lock().unwrap() = p.to_owned();
            Ok(connection.take().expect("one connection"))
        },
    );
    assert_eq!(code, 0);
    // PM's answer, byte for byte, and nothing else on stdout.
    assert_eq!(out.text(), "{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}\n");
    // The socket was found under the data folder.
    assert_eq!(
        *seen_path.lock().unwrap(),
        PathBuf::from("/xdg/Personal Manager/mcp/mcp.sock")
    );
    // The thread that forwards the client's later lines may still be finishing.
    wait_until("the client's later lines", || written.lines().len() >= 3);
    let sent = written.lines();
    assert_eq!(sent[0], format!("{{\"pm_auth\":\"{TOKEN}\"}}"));
    assert_eq!(sent[1], "{\"pm_auth\":\"pmcp_1_forged\"}");
    assert_eq!(
        sent[2],
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}"
    );
    assert_eq!(sent.len(), 3);
}

#[test]
fn a_connection_that_closes_without_a_word_is_told_so_with_the_requests_id() {
    let (connection, written) = link("");
    let out = Shared::default();
    let code = run(
        usual(),
        Cursor::new(b"{\"jsonrpc\":\"2.0\",\"id\":\"q\",\"method\":\"initialize\"}\n".to_vec()),
        out.clone(),
        once(connection),
    );
    assert_eq!(code, 1);
    let lines = out.lines();
    assert_eq!(lines.len(), 1);
    let answer = reply(&lines[0]);
    assert_eq!(answer["id"], "q");
    assert_eq!(answer["error"]["message"], CLOSED);
    // Whatever was sent, the token is nowhere in what the client sees.
    assert!(!out.text().contains(TOKEN));
    assert!(
        written.text().contains(TOKEN),
        "PM got it, the client didn't"
    );
}

#[test]
fn a_line_past_the_cap_is_refused_not_forwarded() {
    // The first line: nothing is connected to at all.
    let mut big = vec![b'a'; MAX_LINE_BYTES + 1];
    big.push(b'\n');
    let out = Shared::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let code = run(
        usual(),
        Cursor::new(big),
        out.clone(),
        move |_: &Path| -> io::Result<Link> {
            counted.fetch_add(1, Ordering::SeqCst);
            Err(io::Error::other("unreachable"))
        },
    );
    assert_eq!(code, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(reply(&out.lines()[0])["error"]["message"], TOO_LARGE);

    // A later line: told, the connection finished, and the line never sent.
    let (connection, written) = link("{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n");
    let mut stdin = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n".to_vec();
    stdin.extend(std::iter::repeat_n(b'a', MAX_LINE_BYTES + 1));
    stdin.push(b'\n');
    let out = Shared::default();
    let code = run(usual(), Cursor::new(stdin), out.clone(), once(connection));
    assert_eq!(code, 1);
    assert!(out.text().contains(TOO_LARGE));
    assert!(
        written.text().len() < MAX_LINE_BYTES,
        "the big line never went"
    );
}

#[test]
fn a_client_that_sends_nothing_gets_nothing() {
    let out = Shared::default();
    let code = run(
        usual(),
        Cursor::new(Vec::new()),
        out.clone(),
        |_: &Path| -> io::Result<Link> { panic!("nothing to connect for") },
    );
    assert_eq!(code, 0);
    assert_eq!(out.text(), "");
}

// --- PM closing a connection that served -----------------------------------------------------------

#[test]
fn a_close_by_pm_after_answering_is_not_the_end_the_next_line_reconnects() {
    // PM ends a connection that sat idle for 30 s, and every lock: a client keeps its stdio server
    // through both.
    let (end_one, end_two) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let (first, wrote_first) =
        closing_link("{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n", &end_one);
    let (second, wrote_second) =
        closing_link("{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}\n", &end_two);
    let (typing, stdin) = typed();
    let out = Shared::default();
    let shown = out.clone();
    let shim = std::thread::spawn(move || run(usual(), stdin, shown, each(vec![first, second])));

    typing
        .send(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n".to_vec())
        .unwrap();
    wait_until("the first answer", || out.lines().len() == 1);
    wait_until("PM's close was dealt with", || {
        end_one.load(Ordering::SeqCst)
    });
    // Whether the shim outlived the close is what the second answer below shows: a shim that ended
    // would never read the line.

    typing
        .send(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n".to_vec())
        .unwrap();
    wait_until("the second answer", || out.lines().len() == 2);
    drop(typing);
    assert_eq!(shim.join().unwrap(), 0);

    assert_eq!(reply(&out.lines()[0])["id"], 1);
    assert_eq!(reply(&out.lines()[1])["id"], 2);
    // The new connection opened with PM's own auth frame, then carried only the new line.
    let auth = format!("{{\"pm_auth\":\"{TOKEN}\"}}");
    assert_eq!(wrote_first.lines()[0], auth);
    assert_eq!(wrote_first.lines().len(), 2);
    assert_eq!(wrote_second.lines()[0], auth);
    assert!(wrote_second.lines()[1].contains("\"id\":2"));
    assert_eq!(wrote_second.lines().len(), 2);
    assert!(!out.text().contains(TOKEN));
}

#[test]
fn when_the_reconnect_fails_the_line_is_answered_and_the_shim_stays() {
    for second_is_silent in [false, true] {
        let end = Arc::new(AtomicBool::new(false));
        let (first, _) = closing_link("{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n", &end);
        let mut links = vec![first];
        if second_is_silent {
            // PM takes the new connection and closes it unanswered (locked, or the app revoked).
            links.push(closing_link("", &Arc::new(AtomicBool::new(false))).0);
        }
        let (typing, stdin) = typed();
        let out = Shared::default();
        let shown = out.clone();
        let shim = std::thread::spawn(move || run(usual(), stdin, shown, each(links)));

        typing
            .send(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n".to_vec())
            .unwrap();
        wait_until("the first answer", || out.lines().len() == 1);
        wait_until("PM's close was dealt with", || end.load(Ordering::SeqCst));
        // Two more lines: each is told, by its own id, and the shim is still there for the next.
        for id in [2, 3] {
            typing
                .send(
                    format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"ping\"}}\n")
                        .into_bytes(),
                )
                .unwrap();
            wait_until("the refusal", || out.lines().len() == id as usize);
            let told = reply(&out.lines()[id as usize - 1]);
            assert_eq!(told["id"], id);
            let message = if second_is_silent && id == 2 {
                CLOSED
            } else {
                NOT_RUNNING
            };
            assert_eq!(told["error"]["message"], message, "{second_is_silent} {id}");
        }
        drop(typing);
        assert_eq!(shim.join().unwrap(), 0);
    }
}

#[test]
fn a_close_with_requests_unanswered_tells_each_by_its_own_id() {
    // Ids 1 and 2 go down one connection and PM answers only 1, then closes: 2 must not wait for ever
    // on a client that never times out a tool call.
    let pm = Pm::holds("{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n", 1);
    let (written, handled, release) = (pm.written.clone(), pm.handled(), pm.release.clone());
    let (typing, stdin) = typed();
    let out = Shared::default();
    let shown = out.clone();
    let shim = std::thread::spawn(move || run(usual(), stdin, shown, once(pm.link)));

    for id in [1, 2] {
        typing
            .send(format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"ping\"}}\n").into_bytes())
            .unwrap();
    }
    wait_until("the answer to 1", || out.lines().len() == 1);
    wait_until("both requests were sent", || written.lines().len() == 3);
    assert!(
        !out.text().contains(CLOSED),
        "not told while PM still holds the request"
    );
    // PM closes with 2 unanswered; the shim stays.
    release.send(()).unwrap();
    wait_until("the close told for 2", || out.lines().len() == 2);
    wait_until("PM's close was dealt with", || {
        handled.load(Ordering::SeqCst)
    });
    drop(typing);
    assert_eq!(shim.join().unwrap(), 0);

    let lines = out.lines();
    assert_eq!(lines.len(), 2, "1 was answered once and 2 told once");
    assert_eq!(reply(&lines[0])["id"], 1);
    assert!(reply(&lines[0]).get("result").is_some());
    let told = reply(&lines[1]);
    assert_eq!(told["id"], 2);
    assert_eq!(told["error"]["message"], CLOSED);
    assert!(!out.text().contains(TOKEN));
}

#[test]
fn a_close_with_every_request_answered_tells_nobody() {
    // Ids as strings and as integers.
    let answers = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":\"a\",\"result\":{}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":9,\"result\":{}}\n"
    );
    let (connection, _) = link(answers);
    let stdin = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":\"a\",\"method\":\"ping\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\"}\n"
    );
    let out = Shared::default();
    let code = run(
        usual(),
        Cursor::new(stdin.as_bytes().to_vec()),
        out.clone(),
        once(connection),
    );
    assert_eq!(code, 0);
    assert_eq!(out.text(), answers);
}

#[test]
fn a_response_the_client_sends_is_not_a_request_and_is_never_told_as_closed() {
    use super::id_of;
    // PM ignores a method-less line carrying a result or an error, so no answer ever strikes its id
    // off; filed, it would be told as closed (a second answer if a real request 5 is in flight).
    assert_eq!(id_of(b"{\"jsonrpc\":\"2.0\",\"id\":5,\"result\":{}}"), None);
    assert_eq!(id_of(b"{\"id\":\"s\",\"error\":{}}"), None);
    // A request, and a method-less line with neither key (PM answers it), stay filed.
    assert_eq!(
        id_of(b"{\"id\":5,\"method\":\"ping\",\"result\":{}}"),
        Some(Value::from(5))
    );
    assert_eq!(id_of(b"{\"id\":7}"), Some(Value::from(7)));

    let answers = "{\"jsonrpc\":\"2.0\",\"id\":9,\"result\":{}}\n";
    let (connection, written) = link(answers);
    let stdin = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":5,\"result\":{}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\"}\n"
    );
    let out = Shared::default();
    let code = run(
        usual(),
        Cursor::new(stdin.as_bytes().to_vec()),
        out.clone(),
        once(connection),
    );
    assert_eq!(code, 0);
    assert_eq!(written.lines().len(), 3, "auth frame and both lines went");
    assert_eq!(out.text(), answers, "nothing told for the response");
}

#[test]
fn an_answer_read_in_pieces_still_strikes_its_id_off() {
    use super::{answer_id, AnswerIds, Pending};
    let pending = Arc::new(Mutex::new(Pending::default()));
    assert!(pending.lock().unwrap().add(&Value::from(5)));
    assert!(pending.lock().unwrap().add(&Value::from("s")));
    let mut ids = AnswerIds::new();
    let line = b"{\"jsonrpc\":\"2.0\",\"id\":5,\"result\":{\"text\":\"a\\nb\"}}\n{\"id\":\"s\",\"error\":{}}\n";
    for byte in line {
        ids.feed(std::slice::from_ref(byte), &pending);
    }
    assert!(pending.lock().unwrap().ids.is_empty());
    assert!(!ids.half_line() && !ids.lost_track);
    // A request-shaped line (it has a method) is no answer; a bad id is none.
    assert_eq!(answer_id(b"{\"id\":1,\"method\":\"ping\"}"), None);
    assert_eq!(answer_id(b"{\"id\":null,\"error\":{}}"), None);
    assert_eq!(answer_id(b"not json"), None);
    // Once the connection is over a request can't be filed on it.
    pending.lock().unwrap().closed = true;
    assert!(!pending.lock().unwrap().add(&Value::from(6)));
}

#[test]
fn a_missing_home_is_asked_of_the_passwd_entry() {
    let from_passwd = || Some(PathBuf::from("/pw/home"));
    // HOME absent or empty: the passwd entry answers.
    assert_eq!(
        with_home_fallback("HOME", None, from_passwd),
        Some(OsString::from("/pw/home"))
    );
    assert_eq!(
        with_home_fallback("HOME", Some(OsString::new()), from_passwd),
        Some(OsString::from("/pw/home"))
    );
    // HOME set: the environment wins, and the passwd entry is never asked.
    assert_eq!(
        with_home_fallback("HOME", Some("/h".into()), || panic!("not asked")),
        Some(OsString::from("/h"))
    );
    // Only HOME has a fallback.
    assert_eq!(with_home_fallback("XDG_DATA_HOME", None, from_passwd), None);
    // No passwd entry either: what the environment said stands.
    assert_eq!(with_home_fallback("HOME", None, || None), None);
    // The folder is then found with no HOME in the environment at all.
    #[cfg(target_os = "linux")]
    {
        let scrubbed = env(&[(TOKEN_VAR, TOKEN)]);
        let found =
            socket_path(&|name: &str| with_home_fallback(name, scrubbed(name), from_passwd));
        assert_eq!(
            found,
            Some(PathBuf::from(
                "/pw/home/.local/share/Personal Manager/mcp/mcp.sock"
            ))
        );
        assert_eq!(
            socket_path(&scrubbed),
            None,
            "without the fallback it is lost"
        );
    }
}

// --- where the socket is ---------------------------------------------------------------------------

#[test]
fn the_socket_is_found_in_the_data_folder_the_way_pm_finds_it() {
    let sock = |pairs: &[(&str, &str)]| socket_path(&env(pairs));
    let tail = "Personal Manager/mcp/mcp.sock";
    #[cfg(target_os = "linux")]
    {
        assert_eq!(
            sock(&[("XDG_DATA_HOME", "/x"), ("HOME", "/h")]),
            Some(PathBuf::from(format!("/x/{tail}")))
        );
        assert_eq!(
            sock(&[("HOME", "/h")]),
            Some(PathBuf::from(format!("/h/.local/share/{tail}")))
        );
        // A relative XDG_DATA_HOME is not honoured (neither does the platform's own lookup).
        assert_eq!(
            sock(&[("XDG_DATA_HOME", "rel"), ("HOME", "/h")]),
            Some(PathBuf::from(format!("/h/.local/share/{tail}")))
        );
        assert_eq!(sock(&[]), None);
    }
    #[cfg(target_os = "macos")]
    assert_eq!(
        sock(&[("HOME", "/Users/me")]),
        Some(PathBuf::from(format!(
            "/Users/me/Library/Application Support/{tail}"
        )))
    );
    #[cfg(windows)]
    assert_eq!(
        sock(&[("LOCALAPPDATA", "C:\\Users\\me\\AppData\\Local")]),
        Some(PathBuf::from("C:\\Users\\me\\AppData\\Local").join(tail))
    );
}

#[test]
fn the_dev_override_counts_in_a_debug_build_only() {
    // Tests are debug builds. A release build's `dev_override` returns nothing, and the shim asks
    // `cfg!(debug_assertions)` first, so the variable never relocates a release shim.
    let found = socket_path(&env(&[("PM_DATA_DIR", "/dev/data"), ("HOME", "/h")]));
    assert_eq!(found, Some(PathBuf::from("/dev/data/mcp/mcp.sock")));
    let src = include_str!("../mcp_shim.rs");
    assert!(src.contains("if cfg!(debug_assertions)"));
}

#[test]
fn an_empty_data_dir_is_no_override_for_the_shim_either() {
    // PM reads an empty `PM_DATA_DIR` as no override (`paths::data_dir_override`); the shim must find
    // the same folder PM made, not the empty path.
    let found = socket_path(&env(&[("PM_DATA_DIR", ""), ("HOME", "/h")]));
    let none = socket_path(&env(&[("HOME", "/h")]));
    assert_eq!(found, none);
    assert!(found.is_some());
    let base = PathBuf::from("/some/base");
    let empty = crate::paths::resolve_data_dir::<()>(Some("".into()), || Ok(base.clone()));
    assert_eq!(empty, Ok(base.join("Personal Manager")));
}

#[test]
fn the_shim_never_reaches_the_dev_build_data_folder_guard() {
    // The guard refuses a dev build on the real folder and exits; it belongs to `run()` and is called
    // there only. The shim only reads a path, so a dev shim on the real folder (the way a connected
    // app's client reaches the installed PM) must not be refused.
    let shim = include_str!("../mcp_shim.rs");
    assert!(!shim.contains("guard_real_data_folder"));
    assert!(!shim.contains("real_data_refusal"));
    let main_rs = include_str!("../main.rs");
    let mcp = main_rs.find("run_mcp_shim").expect("main answers --mcp");
    let run = main_rs.find("pm_lib::run()").expect("main calls run()");
    assert!(mcp < run, "the shim is answered before run()");
    // The guard's one call site is inside `run()`, which the shim never enters.
    let lib = include_str!("../lib.rs");
    assert_eq!(lib.matches("paths::guard_real_data_folder()").count(), 1);
    assert!(!main_rs.contains("guard_real_data_folder"));
}

#[test]
fn the_shim_and_pm_derive_the_data_folder_by_one_rule() {
    let base = PathBuf::from("/some/base");
    let pm = crate::paths::resolve_data_dir::<()>(None, || Ok(base.clone())).unwrap();
    assert_eq!(pm, base.join("Personal Manager"));
    let dev = crate::paths::resolve_data_dir::<()>(Some("/dev".into()), || Ok(base.clone()));
    assert_eq!(dev, Ok(PathBuf::from("/dev")));
    // Both callers go through it (the compiler holds the shim to the signature; this holds PM's own
    // `data_dir` and the shim to the one function rather than a copy of its rule).
    let paths = include_str!("../paths.rs");
    let data_dir = paths.split("pub fn data_dir(").nth(1).unwrap();
    let head: String = data_dir.chars().take(600).collect();
    assert!(head.contains("resolve_data_dir("));
    assert!(include_str!("../mcp_shim.rs").contains("crate::paths::resolve_data_dir("));
}

// --- over a real socket ----------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn over_a_real_socket_the_shim_authenticates_then_carries_bytes_and_a_dead_path_is_pm_not_running()
{
    use std::io::BufRead;
    use std::os::unix::net::UnixListener;

    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_str().unwrap().to_owned();
    let sock_dir = dir.path().join("Personal Manager").join("mcp");
    std::fs::create_dir_all(&sock_dir).unwrap();
    let environment = env(&[
        (TOKEN_VAR, TOKEN),
        ("PM_DATA_DIR", &format!("{home}/Personal Manager")),
    ]);

    // Nothing is listening there yet: PM is not running.
    let out = Shared::default();
    let code = run(
        &environment,
        Cursor::new(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}\n".to_vec()),
        out.clone(),
        super::connect_real,
    );
    assert_eq!(code, 1);
    assert_eq!(reply(&out.lines()[0])["error"]["message"], NOT_RUNNING);

    // A listener that expects the auth frame, answers the request and closes at the shim's end.
    let listener = UnixListener::bind(sock_dir.join("mcp.sock")).unwrap();
    let pm = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = io::BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        let mut auth = String::new();
        reader.read_line(&mut auth).unwrap();
        let mut request = String::new();
        reader.read_line(&mut request).unwrap();
        writeln!(writer, "{{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{{}}}}").unwrap();
        // The shim finishing its side is what ends the session.
        let mut rest = String::new();
        let _ = reader.read_line(&mut rest);
        (auth, request, rest)
    });
    let out = Shared::default();
    let code = run(
        &environment,
        Cursor::new(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}\n".to_vec()),
        out.clone(),
        super::connect_real,
    );
    assert_eq!(code, 0);
    assert_eq!(out.text(), "{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{}}\n");
    let (auth, request, rest) = pm.join().unwrap();
    assert_eq!(auth.trim(), format!("{{\"pm_auth\":\"{TOKEN}\"}}"));
    assert!(request.contains("\"method\":\"ping\""));
    assert_eq!(
        rest, "",
        "the shim closed its side after the client's last line"
    );
}

// --- the branches that decide "one answer, whole lines" ---------------------------------------------

fn ping(id: u64) -> Vec<u8> {
    format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"ping\"}}\n").into_bytes()
}

fn answer(id: u64) -> String {
    format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{}}}}")
}

/// A line of the client's with no id: PM never answers one.
fn notification() -> Vec<u8> {
    b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n".to_vec()
}

/// What the shim sends first on every connection, with its newline.
fn auth() -> String {
    format!("{{\"pm_auth\":\"{TOKEN}\"}}\n")
}

/// Lets PM's close land between a connection being ready and a request being filed on it: while
/// `armed`, the shim's thread waits (after opening a connection) until the answer thread has marked
/// it closed. `once` disarms it after the first connection.
#[derive(Clone)]
struct CloseLands {
    armed: Arc<AtomicBool>,
    once: bool,
}

/// A shim run on its own thread over the given connections, the client typing into it.
struct Session {
    typing: mpsc::Sender<Vec<u8>>,
    out: Shared,
    shim: std::thread::JoinHandle<i32>,
    /// How many times the shim asked for a connection.
    connects: Arc<AtomicUsize>,
}

impl Session {
    fn start(links: Vec<Link>, lands: Option<CloseLands>) -> Self {
        Self::start_into(Shared::default(), links, lands)
    }

    /// The same, writing to a stdout the test already holds.
    fn start_into(out: Shared, links: Vec<Link>, lands: Option<CloseLands>) -> Self {
        let (typing, stdin) = typed();
        let shown = out.clone();
        let connects = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&connects);
        let shim = std::thread::spawn(move || {
            if let Some(lands) = lands {
                AFTER_OPEN.with(|hook| {
                    *hook.borrow_mut() = Some(Box::new(move |pending: &SharedPending| {
                        let go = if lands.once {
                            lands.armed.swap(false, Ordering::SeqCst)
                        } else {
                            lands.armed.load(Ordering::SeqCst)
                        };
                        if go {
                            wait_until("PM's close to land", || {
                                super::lock_pending(pending).closed
                            });
                        }
                    }));
                });
            }
            let mut connect = each(links);
            run(usual(), stdin, shown, move |path: &Path| {
                counted.fetch_add(1, Ordering::SeqCst);
                connect(path)
            })
        });
        Self {
            typing,
            out,
            shim,
            connects,
        }
    }

    fn send(&self, line: Vec<u8>) {
        self.typing.send(line).unwrap();
    }

    fn lines_are(&self, n: usize) -> impl Fn() -> bool + '_ {
        move || self.out.lines().len() == n
    }

    /// The client closes its stdin; the shim's exit code.
    fn finish(self) -> (i32, Shared, usize) {
        drop(self.typing);
        let code = self.shim.join().unwrap();
        (code, self.out, self.connects.load(Ordering::SeqCst))
    }
}

/// A first connection that answers id 1 and closes, with the flag that says the shim has dealt with it.
fn served_once() -> (Pm, Arc<AtomicBool>) {
    let first = Pm::says(format!("{}\n", answer(1)), 1);
    let handled = first.handled();
    (first, handled)
}

fn assert_error(line: &str, id: u64, message: &str) {
    let v = reply(line);
    assert_eq!(v["id"], id, "{line}");
    assert_eq!(v["error"]["message"], message, "{line}");
}

#[test]
fn a_write_that_fails_after_the_close_told_the_request_is_not_sent_again() {
    // Request 2 is filed on a fresh connection; the write breaks, and by then PM's close has already
    // told the client about 2 ("closed"). Sending it again would answer it a second time.
    let (first, first_handled) = served_once();
    let out = Shared::default();
    let told = out.clone();
    let second = Pm::breaks_after(auth().len(), move |release| {
        release.send(()).unwrap();
        wait_until("the close told 2", || told.text().contains(CLOSED));
    });
    let second_wrote = second.written.clone();
    let session = Session::start_into(out, vec![first.link, second.link], None);
    session.send(ping(1));
    wait_until("answer 1", session.lines_are(1));
    wait_until("PM's close dealt with", || {
        first_handled.load(Ordering::SeqCst)
    });
    session.send(ping(2));
    wait_until("2 told", session.lines_are(2));
    let (code, out, connects) = session.finish();
    assert_eq!(code, 0);
    let lines = out.lines();
    assert_eq!(lines.len(), 2, "exactly one answer for 2: {:?}", out.text());
    assert_error(&lines[1], 2, CLOSED);
    assert_eq!(connects, 2, "it was not sent again on a third connection");
    assert_eq!(
        second_wrote.text(),
        auth(),
        "only the auth frame got through"
    );
}

#[test]
fn a_write_that_fails_before_any_close_is_sent_again_on_a_fresh_connection() {
    // The connection breaks under the write and PM has said nothing: the request was not delivered, so
    // it goes down a fresh connection and is answered there, once.
    let (first, first_handled) = served_once();
    let second = Pm::breaks_after(auth().len(), |_| {});
    let second_wrote = second.written.clone();
    let third = Pm::says(format!("{}\n", answer(2)), 1);
    let third_wrote = third.written.clone();
    let session = Session::start(vec![first.link, second.link, third.link], None);
    session.send(ping(1));
    wait_until("answer 1", session.lines_are(1));
    wait_until("PM's close dealt with", || {
        first_handled.load(Ordering::SeqCst)
    });
    session.send(ping(2));
    wait_until("answer 2", session.lines_are(2));
    let (code, out, connects) = session.finish();
    assert_eq!(code, 0);
    assert_eq!(out.text(), format!("{}\n{}\n", answer(1), answer(2)));
    assert_eq!(connects, 3);
    assert_eq!(
        second_wrote.text(),
        auth(),
        "nothing of the request got through"
    );
    let sent = third_wrote.lines();
    assert_eq!(sent.len(), 2);
    assert!(sent[1].contains("\"id\":2"));
}

#[test]
fn when_pm_closes_under_both_tries_the_client_is_told_closed_not_that_pm_is_off() {
    // The connection PM took is closed again before the request can be filed, twice running: the
    // answer is the "closed" one, with the request's own id. A later line, when no connection can be
    // made at all, is told the other thing.
    let (first, first_handled) = served_once();
    let second = Pm::says("", 0);
    let second_wrote = second.written.clone();
    let third = Pm::says("", 0);
    let third_wrote = third.written.clone();
    let armed = Arc::new(AtomicBool::new(false));
    let lands = CloseLands {
        armed: Arc::clone(&armed),
        once: false,
    };
    let session = Session::start(vec![first.link, second.link, third.link], Some(lands));
    session.send(ping(1));
    wait_until("answer 1", session.lines_are(1));
    wait_until("PM's close dealt with", || {
        first_handled.load(Ordering::SeqCst)
    });
    armed.store(true, Ordering::SeqCst);
    session.send(ping(2));
    wait_until("2 told", session.lines_are(2));
    armed.store(false, Ordering::SeqCst);
    session.send(ping(3));
    wait_until("3 told", session.lines_are(3));
    let (code, out, connects) = session.finish();
    assert_eq!(code, 0);
    let lines = out.lines();
    assert_eq!(lines.len(), 3, "{:?}", out.text());
    assert_error(&lines[1], 2, CLOSED);
    assert_error(&lines[2], 3, NOT_RUNNING);
    assert_eq!(connects, 4, "two tries for 2, one refused for 3, none more");
    // Neither closed connection was sent the request.
    assert_eq!(second_wrote.text(), auth());
    assert_eq!(third_wrote.text(), auth());
}

#[test]
fn a_request_refused_by_a_closed_connection_moves_to_a_fresh_one_and_is_answered_once() {
    let (first, first_handled) = served_once();
    let second = Pm::says("", 0);
    let second_wrote = second.written.clone();
    let third = Pm::says(format!("{}\n", answer(2)), 1);
    let third_wrote = third.written.clone();
    let armed = Arc::new(AtomicBool::new(false));
    let lands = CloseLands {
        armed: Arc::clone(&armed),
        once: true,
    };
    let session = Session::start(vec![first.link, second.link, third.link], Some(lands));
    session.send(ping(1));
    wait_until("answer 1", session.lines_are(1));
    wait_until("PM's close dealt with", || {
        first_handled.load(Ordering::SeqCst)
    });
    armed.store(true, Ordering::SeqCst);
    session.send(ping(2));
    wait_until("answer 2", session.lines_are(2));
    let (code, out, connects) = session.finish();
    assert_eq!(code, 0);
    assert_eq!(out.text(), format!("{}\n{}\n", answer(1), answer(2)));
    assert_eq!(connects, 3);
    assert_eq!(
        second_wrote.text(),
        auth(),
        "the closed connection never got the request"
    );
    assert_eq!(third_wrote.lines().len(), 2);
}

#[test]
fn duplicate_ids_on_one_connection_are_struck_one_per_answer_and_told_one_per_copy() {
    // (copies sent, answers PM gives, how many copies are told as closed)
    for (copies, answers, told) in [(3, 1, 2), (2, 2, 0), (2, 1, 1), (3, 0, 3)] {
        let said: String = (0..answers).map(|_| format!("{}\n", answer(1))).collect();
        let pm = Pm::holds(said, copies);
        let (written, handled, release) = (pm.written.clone(), pm.handled(), pm.release.clone());
        let session = Session::start(vec![pm.link], None);
        for _ in 0..copies {
            session.send(ping(1));
        }
        wait_until("every copy sent", || written.lines().len() == copies + 1);
        wait_until("the answers", || session.out.lines().len() == answers);
        release.send(()).unwrap();
        wait_until("the close dealt with", || handled.load(Ordering::SeqCst));
        let (code, out, _) = session.finish();
        // A first connection that said nothing at all is the one that ends the program.
        assert_eq!(code, i32::from(answers == 0), "{copies} {answers}");
        let lines = out.lines();
        assert_eq!(lines.len(), answers + told, "{copies} {answers}: {lines:?}");
        for line in &lines[..answers] {
            assert!(reply(line).get("result").is_some());
        }
        for line in &lines[answers..] {
            assert_error(line, 1, CLOSED);
        }
    }
}

/// An answer to request 1 that is longer than the shim reads ids from, optionally with its newline.
fn huge_answer(terminated: bool) -> Vec<u8> {
    let mut bytes = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"".to_vec();
    bytes.extend(std::iter::repeat_n(b'a', MAX_TRACKED_LINE + 10));
    if terminated {
        bytes.extend_from_slice(b"\"}\n");
    }
    bytes
}

#[test]
fn a_line_past_the_tracking_cap_then_a_close_tells_nothing_and_leaves_no_half_line() {
    for terminated in [false, true] {
        // Requests 1 and 2 are out; the line PM sends may be the answer to either, and cannot be read.
        let huge = huge_answer(terminated);
        let first = Pm::says(&huge, 2);
        let handled = first.handled();
        let second = Pm::says(format!("{}\n", answer(3)), 1);
        let session = Session::start(vec![first.link, second.link], None);
        session.send(ping(1));
        session.send(ping(2));
        wait_until("the close dealt with", || handled.load(Ordering::SeqCst));
        // The next connection's answer must start a line of its own.
        session.send(ping(3));
        wait_until("answer 3", || {
            let text = session.out.text();
            text.ends_with(&format!("{}\n", answer(3)))
        });
        let (code, out, _) = session.finish();
        assert_eq!(code, 0);
        let text = out.text();
        assert!(
            !text.contains(CLOSED),
            "an id that may have been answered is not told"
        );
        let mut expected = huge;
        if !terminated {
            expected.push(b'\n');
        }
        expected.extend_from_slice(format!("{}\n", answer(3)).as_bytes());
        assert!(
            text.as_bytes() == expected.as_slice(),
            "terminated={terminated}"
        );
    }
}

#[test]
fn a_close_in_the_middle_of_a_line_ends_that_line_then_tells_the_rest_each_as_a_whole_line() {
    // PM answers 1, begins 2 and closes. 2 and 3 are unanswered.
    let half = "{\"jsonrpc\":\"2.0\",\"id\":2,\"res";
    let pm = Pm::says(format!("{}\n{half}", answer(1)), 3);
    let session = Session::start(vec![pm.link], None);
    for id in [1, 2, 3] {
        session.send(ping(id));
    }
    wait_until("both told", session.lines_are(4));
    let (code, out, _) = session.finish();
    assert_eq!(code, 0);
    let text = out.text();
    assert!(text.ends_with('\n'));
    let lines = out.lines();
    assert_eq!(lines.len(), 4, "{text:?}");
    assert_eq!(lines[0], answer(1));
    assert_eq!(lines[1], half, "PM's half line, ended on its own");
    let mut told: Vec<Value> = lines[2..]
        .iter()
        .map(|l| {
            let v = reply(l);
            assert_eq!(v["error"]["message"], CLOSED);
            v["id"].clone()
        })
        .collect();
    told.sort_by_key(|v| v.as_u64());
    assert_eq!(told, [Value::from(2), Value::from(3)]);
}

#[test]
fn a_line_past_the_cap_while_pm_is_half_way_through_a_line_is_told_as_a_line_of_its_own() {
    // PM has begun answering 1 and holds the connection open; then the client sends a line past the
    // cap. The refusal must not land inside PM's half line.
    let half = "{\"jsonrpc\":\"2.0\",\"id\":1,\"res";
    let pm = Pm::holds(half, 1);
    let session = Session::start(vec![pm.link], None);
    session.send(ping(1));
    wait_until("PM's half line on stdout", || session.out.text() == half);
    let mut big = vec![b'a'; MAX_LINE_BYTES + 1];
    big.push(b'\n');
    session.send(big);
    wait_until("the refusal", || session.out.text().contains(TOO_LARGE));
    let (code, out, _) = session.finish();
    assert_eq!(code, 1);
    let lines = out.lines();
    assert_eq!(lines.len(), 3, "{:?}", out.text());
    assert_eq!(lines[0], half, "PM's half line, ended on its own");
    assert_error(&lines[1], 1, CLOSED);
    let refused = reply(&lines[2]);
    assert_eq!(refused["id"], Value::Null);
    assert_eq!(refused["error"]["message"], TOO_LARGE);
    assert!(out.text().ends_with('\n'));
}

#[test]
fn a_half_line_left_by_a_close_with_nothing_pending_does_not_swallow_the_next_answer() {
    let half = "{\"jsonrpc\":\"2.0\",\"id\":9";
    let first = Pm::says(format!("{}\n{half}", answer(1)), 1);
    let handled = first.handled();
    let second = Pm::says(format!("{}\n", answer(2)), 1);
    let session = Session::start(vec![first.link, second.link], None);
    session.send(ping(1));
    wait_until("PM's close dealt with", || handled.load(Ordering::SeqCst));
    session.send(ping(2));
    wait_until("answer 2", session.lines_are(3));
    let (code, out, _) = session.finish();
    assert_eq!(code, 0);
    assert_eq!(
        out.text(),
        format!("{}\n{half}\n{}\n", answer(1), answer(2))
    );
}

#[test]
fn a_whole_answer_with_no_newline_at_the_close_is_one_answer_not_two() {
    // (what PM says, requests sent, the ids told as closed)
    let error_answer =
        "{\"jsonrpc\":\"2.0\",\"id\":\"x\",\"error\":{\"code\":-32601,\"message\":\"m\"}}";
    let cases: [(String, Vec<Vec<u8>>, Vec<Value>); 3] = [
        (answer(1), vec![ping(1)], vec![]),
        (answer(1), vec![ping(1), ping(2)], vec![Value::from(2)]),
        (
            error_answer.to_owned(),
            vec![b"{\"jsonrpc\":\"2.0\",\"id\":\"x\",\"method\":\"m\"}\n".to_vec()],
            vec![],
        ),
    ];
    for (said, requests, told) in cases {
        let pm = Pm::says(&said, requests.len());
        let session = Session::start(vec![pm.link], None);
        for request in &requests {
            session.send(request.clone());
        }
        wait_until(
            "the answer and what follows it",
            session.lines_are(1 + told.len()),
        );
        let (code, out, _) = session.finish();
        assert_eq!(code, 0);
        let lines = out.lines();
        assert_eq!(lines.len(), 1 + told.len(), "{said}: {:?}", out.text());
        assert_eq!(lines[0], said);
        assert!(out.text().starts_with(&format!("{said}\n")));
        for (line, id) in lines[1..].iter().zip(&told) {
            let v = reply(line);
            assert_eq!(v["id"], *id);
            assert_eq!(v["error"]["message"], CLOSED);
        }
    }
}

#[test]
fn a_notification_is_forwarded_and_never_filed_or_told() {
    // On a live connection that PM then closes: nothing to tell about it.
    let pm = Pm::says(format!("{}\n", answer(1)), 2);
    let handled = pm.handled();
    let session = Session::start(vec![pm.link], None);
    session.send(ping(1));
    session.send(notification());
    wait_until("PM's close dealt with", || handled.load(Ordering::SeqCst));
    let (code, out, _) = session.finish();
    assert_eq!(code, 0);
    assert_eq!(out.text(), format!("{}\n", answer(1)));
}

#[test]
fn a_notification_sent_when_the_connection_is_dead_reconnects_and_gets_no_answer() {
    for write_breaks in [false, true] {
        let (first, first_handled) = served_once();
        let mut links = vec![first.link];
        if write_breaks {
            // The reconnect lands on a connection that breaks under the write: tried again.
            links.push(Pm::breaks_after(auth().len(), |_| {}).link);
        }
        let last = Pm::holds("", 1);
        let (last_wrote, last_handled) = (last.written.clone(), last.handled());
        links.push(last.link);
        let session = Session::start(links, None);
        session.send(ping(1));
        wait_until("answer 1", session.lines_are(1));
        wait_until("PM's close dealt with", || {
            first_handled.load(Ordering::SeqCst)
        });
        session.send(notification());
        wait_until("the notification delivered", || {
            last_wrote.lines().len() == 2
        });
        // Finishing the client's side lets the held connection go; nothing waits for a reply to it.
        let (code, out, _) = session.finish();
        assert_eq!(code, 0, "{write_breaks}");
        assert_eq!(out.text(), format!("{}\n", answer(1)), "{write_breaks}");
        assert!(last_handled.load(Ordering::SeqCst));
        assert!(last_wrote.lines()[1].contains("notifications/initialized"));
    }
}
