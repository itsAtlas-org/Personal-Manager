// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A scripted HTTP server on loopback, for the tests of a provider's own wire. Plain std sockets on
//! threads (never a fork, never the outside network): it serves a list of replies in order, one
//! connection each, records what each request carried, and can stream a reply in chunks with a flush
//! and a short pause between them (so a multi-byte character really does split across reads) or hang
//! up in the middle of a body.
//!
//! Test-only. A test hands the server's URL to the code under test; no test reads a real key, the
//! real network or the host time zone.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

/// One scripted answer.
pub(crate) enum Reply {
    /// A status and a JSON body.
    Http(u16, String),
    /// A status, extra headers (name, value) and a JSON body.
    HttpWith(u16, Vec<(String, String)>, String),
    /// A 200 event stream written in these chunks, each flushed, with a short pause between.
    Sse(Vec<Vec<u8>>),
    /// The same, but the server promises more bytes than it sends and then hangs up: a connection
    /// dropped in the middle of a body.
    SseCut(Vec<Vec<u8>>),
    /// Accept the connection and hang up with no answer at all.
    Drop,
}

/// What one request carried.
#[derive(Clone, Debug)]
pub(crate) struct Request {
    pub path: String,
    pub body: String,
    /// Header (lowercased name, value), in order.
    pub headers: Vec<(String, String)>,
}

impl Request {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).expect("a JSON body")
    }
}

/// Every request the server saw, in order.
pub(crate) type Seen = Arc<Mutex<Vec<Request>>>;

/// Serve `replies` in order, one connection each. Returns the base URL (`http://127.0.0.1:<port>`)
/// and what the server saw.
pub(crate) fn serve(replies: Vec<Reply>) -> (String, Seen) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("an address").port();
    let seen: Seen = Arc::default();
    let log = seen.clone();
    std::thread::spawn(move || {
        for reply in replies {
            let Ok((mut sock, _)) = listener.accept() else {
                return;
            };
            log.lock().unwrap().push(read_request(&mut sock));
            match reply {
                Reply::Http(status, body) => write_http(&mut sock, status, &[], &body),
                Reply::HttpWith(status, headers, body) => {
                    write_http(&mut sock, status, &headers, &body)
                }
                Reply::Sse(chunks) => write_sse(&mut sock, &chunks, false),
                Reply::SseCut(chunks) => write_sse(&mut sock, &chunks, true),
                Reply::Drop => drop(sock),
            }
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

/// Read one request through its body, so the client is never answered before it has finished sending.
fn read_request(sock: &mut std::net::TcpStream) -> Request {
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    let (head_end, want) = loop {
        let n = sock.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break (raw.len(), 0);
        }
        raw.extend_from_slice(&buf[..n]);
        if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&raw[..i]).to_lowercase();
            let len = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            break (i + 4, len);
        }
    };
    while raw.len() < head_end + want {
        let n = sock.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
    }
    let head = String::from_utf8_lossy(&raw[..head_end.min(raw.len())]).to_string();
    let path = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("")
        .to_string();
    let headers = head
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_lowercase(), v.trim().to_string()))
        .collect();
    Request {
        path,
        body: String::from_utf8_lossy(&raw[head_end.min(raw.len())..]).to_string(),
        headers,
    }
}

fn write_http(
    sock: &mut std::net::TcpStream,
    status: u16,
    headers: &[(String, String)],
    body: &str,
) {
    let extra: String = headers
        .iter()
        .map(|(n, v)| format!("{n}: {v}\r\n"))
        .collect();
    let head = format!(
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    let _ = sock.write_all(head.as_bytes());
    let _ = sock.write_all(body.as_bytes());
    let _ = sock.flush();
}

fn write_sse(sock: &mut std::net::TcpStream, chunks: &[Vec<u8>], cut: bool) {
    let total: usize = chunks.iter().map(Vec::len).sum();
    // A cut stream promises more than it sends, so the client sees the connection closed mid-body.
    let promised = if cut { total + 4096 } else { total };
    let head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {promised}\r\nconnection: close\r\n\r\n"
    );
    let _ = sock.write_all(head.as_bytes());
    for chunk in chunks {
        let _ = sock.write_all(chunk);
        let _ = sock.flush();
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    let _ = sock.flush();
}
