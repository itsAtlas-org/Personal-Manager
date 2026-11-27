// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Whether a paid request may have LEFT this machine, for the one caller that has to know: a mod's
//! `ai.complete`, which books a worst-case price before the call and must give the booking back when
//! the request provably never reached the provider (an offline laptop, Local AI switched off, a
//! deadline hit while still queued behind a chat). Anything else is counted as spent: the cap errs
//! high.
//!
//! The caller makes a [`WireMark`] and runs the model call inside [`WireMark::scope`]. A send site
//! calls [`send`] instead of `.send()`; outside a scope that is plain `.send()`, so no other caller
//! of the same code notices. The mark is a pair of counts rather than a flag so that a failed first
//! leg followed by a refused second one still reads as "a request left".

use std::future::Future;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

tokio::task_local! {
    static MARK: WireMark;
}

#[derive(Default)]
struct Counts {
    /// Requests about to be sent.
    sending: AtomicU32,
    /// Sends that failed before a connection existed, so nothing was written to the provider.
    never_left: AtomicU32,
}

/// What one call's requests did. Cheap to clone; clones share the counts.
#[derive(Clone, Default)]
pub struct WireMark(Arc<Counts>);

impl WireMark {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `fut` with this mark current, so [`send`] inside it is recorded here. The mark must be
    /// scoped around the whole call and read from outside it: a call dropped at a deadline is still
    /// judged by what it recorded up to then.
    pub async fn scope<F: Future>(&self, fut: F) -> F::Output {
        MARK.scope(self.clone(), fut).await
    }

    /// A request is about to be written. Every request counts until its failure proves it did not
    /// leave ([`Self::never_left`]).
    pub fn sending(&self) {
        self.0.sending.fetch_add(1, Ordering::SeqCst);
    }

    /// The request just announced failed before a connection was made (DNS, refused, a connect
    /// timeout): nothing of it reached the provider.
    pub fn never_left(&self) {
        self.0.never_left.fetch_add(1, Ordering::SeqCst);
    }

    /// Whether any request may have reached the provider. `sending` is counted before `never_left`
    /// can be, so a call cut off between the two reads as sent.
    pub fn may_have_reached_the_wire(&self) -> bool {
        let never_left = self.0.never_left.load(Ordering::SeqCst);
        self.0.sending.load(Ordering::SeqCst) > never_left
    }
}

/// Whether a failed send proves the request never left: the connection was never made.
fn failed_before_connecting(e: &reqwest::Error) -> bool {
    e.is_connect()
}

/// `request.send()`, recorded on the current [`WireMark`] if one is scoped.
pub async fn send(request: reqwest::RequestBuilder) -> reqwest::Result<reqwest::Response> {
    let _ = MARK.try_with(WireMark::sending);
    let sent = request.send().await;
    if let Err(e) = &sent {
        if failed_before_connecting(e) {
            let _ = MARK.try_with(WireMark::never_left);
        }
    }
    sent
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A loopback port with nothing listening: the connect is refused, no external network.
    fn closed_port() -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    }

    async fn marked(url: String) -> WireMark {
        let mark = WireMark::new();
        let client = reqwest::Client::new();
        let _ = mark.scope(send(client.post(url))).await;
        mark
    }

    /// The paid request of a mod's call is announced: it is the one `ai.complete` runs on, and a
    /// plain `.send()` there would make every failure before the wire look unsent ("free"). The send
    /// lives in the shared OpenAI-dialect core (`cloud_openai::complete`), which every provider of
    /// that dialect goes through, and OpenRouter's completion is a call into it, not a send of its own.
    #[test]
    fn the_openrouter_completion_sends_through_the_mark() {
        let core = include_str!("cloud_openai.rs");
        let body = core
            .split("pub(crate) async fn complete(")
            .nth(1)
            .and_then(|rest| rest.split("#[cfg(test)]").next())
            .expect("complete comes before the tests");
        assert!(body.contains("crate::wire_mark::send("));
        assert!(!body.contains(".send()"));

        let src = include_str!("openrouter.rs");
        let body = src
            .split("pub async fn complete(")
            .nth(1)
            .and_then(|rest| rest.split("#[cfg(test)]").next())
            .expect("complete comes before the tests");
        assert!(body.contains("cloud_openai::complete("));
        assert!(!body.contains(".send()"));
    }

    /// The streamed request is announced too (hardening: no mod reaches a direct arm today, but if
    /// one ever does, its booking is only refunded correctly when the stream's send is marked), and
    /// your own OpenAI key's one-shot completion is a call into that stream, with no send of its
    /// own. The key check is not a paid request and stays a plain send outside these slices.
    #[test]
    fn the_stream_and_the_direct_completion_send_through_the_mark() {
        let core = include_str!("cloud_openai.rs");
        let stream = core
            .split("pub(crate) async fn stream<F>(")
            .nth(1)
            .and_then(|rest| rest.split("pub(crate) async fn complete(").next())
            .expect("stream comes before complete");
        assert!(stream.contains("crate::wire_mark::send("));
        assert!(!stream.contains(".send()"));

        let direct = include_str!("openai_direct.rs");
        let complete = direct
            .split("pub(crate) async fn complete(")
            .nth(1)
            .and_then(|rest| rest.split("pub(crate) async fn stream_chat").next())
            .expect("complete comes before stream_chat");
        assert!(complete.contains("cloud_openai::stream("));
        assert!(!complete.contains(".send()"));

        // Your own Gemini key's one-shot completion is the same call into the stream. Its key check
        // (the list and the probe) is a plain send outside this slice, for the same reason.
        let gemini = include_str!("gemini_direct.rs");
        let complete = gemini
            .split("pub(crate) async fn complete(")
            .nth(1)
            .and_then(|rest| rest.split("pub(crate) async fn stream_chat").next())
            .expect("complete comes before stream_chat");
        assert!(complete.contains("cloud_openai::stream("));
        assert!(!complete.contains(".send()"));
    }

    #[test]
    fn a_fresh_mark_has_sent_nothing() {
        assert!(!WireMark::new().may_have_reached_the_wire());
    }

    #[test]
    fn a_request_counts_until_its_failure_proves_it_did_not_leave() {
        let m = WireMark::new();
        m.sending();
        assert!(m.may_have_reached_the_wire());
        m.never_left();
        assert!(!m.may_have_reached_the_wire());
        // A second leg that did leave, after a first that did not.
        m.sending();
        assert!(m.may_have_reached_the_wire());
        // And a refused second leg does not erase a first that left.
        let m = WireMark::new();
        m.sending();
        m.sending();
        m.never_left();
        assert!(m.may_have_reached_the_wire());
    }

    #[tokio::test]
    async fn a_refused_connection_never_left() {
        let mark = marked(format!("http://127.0.0.1:{}/", closed_port())).await;
        assert!(!mark.may_have_reached_the_wire());
    }

    #[tokio::test]
    async fn a_connection_that_was_made_may_have_carried_the_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // Accept and hang up: the request was written to a live peer, and the reply never came.
        let peer = std::thread::spawn(move || drop(listener.accept()));
        let mark = marked(format!("http://127.0.0.1:{port}/")).await;
        peer.join().unwrap();
        assert!(mark.may_have_reached_the_wire());
    }

    #[tokio::test]
    async fn a_send_outside_a_scope_is_plain_and_records_nothing() {
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{}/", closed_port());
        assert!(send(client.post(url)).await.is_err());
        // Nothing to read back: the point is that it did not panic without a scope.
    }
}
