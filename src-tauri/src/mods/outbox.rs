// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The pipe from Rust to one frame's session (Mods spec §1.5 "Events"). When PM's page opens a frame
//! it hands Rust a Tauri `Channel`; Rust keeps it here, one [`Outbox`] per session, and every event
//! goes through it. Rust has already decided, per mod and per grant, who may be told what
//! (`api::events`), so the page and the frame never see an event the mod isn't granted.
//!
//! **Bounded.** An outbox holds at most [`limits::EVENT_WINDOW`] events nobody has acknowledged. Each
//! event carries a `seq`; the frame acknowledges what it has taken (`{ ack: seq }` over its port, which
//! PM's page relays as `mod_events_ack`), and the window slides. Past it an event is dropped and
//! counted, never queued, so Rust holds nothing for a slow frame; the next event that does go out says
//! how many were `missed`, and the frame asks again. The window covers every hop at once, the Tauri
//! channel, PM's page, and the frame's port, because an event is outstanding until the frame says it
//! has it. The goodbye notices ([`Ending`]) bypass the window, since they are the last things a
//! session is ever sent and there are at most two.
//!
//! **Closed means closed.** A session's outbox is closed when its session ends (uninstall, pause,
//! reinstall, the frame's own close, a store swap: `runtime`), after any goodbye. Delivery to a closed
//! outbox does nothing, and it is under the outbox's own lock, so once a close has returned no event
//! reaches that session, and none ever reaches a later session of the same mod (a later session has
//! its own outbox). A channel that fails to send (the page is gone) closes its outbox the same way.
//!
//! The transport is a trait so the logic is tested without a window.

use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::ipc::Channel;

use super::api::limits;

/// The two lifecycle events a session gets without holding any permission.
pub const MOD_PAUSED: &str = "mod.paused";
pub const SESSION_CLOSED: &str = "session.closed";

/// One event as the page receives it from the channel (`ipc.ts`'s `ModEventMessage`), and passes on
/// to the frame (`modBridge.ts`'s `readEvent`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventMessage {
    /// 1, 2, 3... per session, in the order sent. What the frame acknowledges.
    pub seq: u64,
    pub name: &'static str,
    pub payload: Value,
    /// How many events were dropped before this one (the window or the size cap), when any were.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missed: Option<u64>,
}

/// Where an outbox sends. `false` means the far end is gone, which closes the outbox.
pub trait EventSink: Send + Sync + 'static {
    fn deliver(&self, message: &EventMessage) -> bool;
}

impl EventSink for Channel<EventMessage> {
    fn deliver(&self, message: &EventMessage) -> bool {
        self.send(message.clone()).is_ok()
    }
}

/// Why a session ended, which decides what its frame is told on the way out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The frame closed it, the page reloaded, or the frame never loaded: nobody is left to tell.
    Silent,
    /// The mod was uninstalled.
    Uninstalled,
    /// The mod was installed again over itself (an update, a repair), which ends every frame of the
    /// old one.
    Replaced,
    /// The mod was paused or parked (the user's, the blacklist's).
    Paused,
}

impl Ending {
    /// What a live frame is told: `mod.paused` first when it was a pause, then `session.closed` with
    /// the reason. Neither carries anything but the reason.
    pub fn notices(self) -> Vec<(&'static str, Value)> {
        let (paused, reason) = match self {
            Self::Silent => return Vec::new(),
            Self::Uninstalled => (false, "uninstalled"),
            Self::Replaced => (false, "replaced"),
            Self::Paused => (true, "paused"),
        };
        let mut out = Vec::new();
        if paused {
            out.push((MOD_PAUSED, json!({})));
        }
        out.push((SESSION_CLOSED, json!({ "reason": reason })));
        out
    }
}

/// What happened to one event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Sent,
    /// Not sent: the window was full or the event too large. Counted.
    Dropped,
    /// Not sent: the outbox is closed.
    Closed,
}

/// An outbox's counters: what the Developer tab (slice 6) shows per session, and what tests read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(not(test), allow(dead_code))] // the Developer tab (slice 6)
pub struct Stats {
    pub sent: u64,
    pub acked: u64,
    pub dropped: u64,
    pub closed: bool,
}

struct State {
    /// `None` once closed: the sink is dropped, which drops the channel.
    sink: Option<Box<dyn EventSink>>,
    sent: u64,
    acked: u64,
    /// Dropped since the last event that went out.
    missed: u64,
    dropped: u64,
}

pub struct Outbox {
    state: Mutex<State>,
}

impl Outbox {
    pub fn new(sink: impl EventSink) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                sink: Some(Box::new(sink)),
                sent: 0,
                acked: 0,
                missed: 0,
                dropped: 0,
            }),
        })
    }

    /// The state is plain counters and a sink, so a panic in a sink leaves it consistent enough to
    /// read: a poisoned lock is taken all the same.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Send one event, unless the outbox is closed, the window is full or the payload is over the
    /// size cap. Never blocks on the frame.
    pub fn deliver(&self, name: &'static str, payload: Value) -> Delivery {
        let mut state = self.lock();
        if state.sink.is_none() {
            return Delivery::Closed;
        }
        let too_large = || {
            serde_json::to_vec(&payload).map_or(true, |bytes| bytes.len() > limits::MAX_EVENT_BYTES)
        };
        if state.sent - state.acked >= limits::EVENT_WINDOW || too_large() {
            state.missed += 1;
            state.dropped += 1;
            return Delivery::Dropped;
        }
        if state.send(name, payload) {
            Delivery::Sent
        } else {
            state.sink = None;
            Delivery::Closed
        }
    }

    /// The frame has taken everything up to `seq`. An acknowledgement of an event not yet sent counts
    /// for what was.
    pub fn ack(&self, seq: u64) {
        let mut state = self.lock();
        state.acked = state.acked.max(seq.min(state.sent));
    }

    /// Say goodbye (outside the window), then close. The notices stop at the first failure.
    pub fn finish(&self, notices: Vec<(&'static str, Value)>) {
        let mut state = self.lock();
        for (name, payload) in notices {
            if state.sink.is_none() || !state.send(name, payload) {
                break;
            }
        }
        state.sink = None;
    }

    /// Close with nothing more said (a test's; the runtime's endings all go through [`Self::finish`]).
    #[cfg(test)]
    pub fn close(&self) {
        self.lock().sink = None;
    }

    #[cfg_attr(not(test), allow(dead_code))] // the Developer tab (slice 6)
    pub fn stats(&self) -> Stats {
        let state = self.lock();
        Stats {
            sent: state.sent,
            acked: state.acked,
            dropped: state.dropped,
            closed: state.sink.is_none(),
        }
    }
}

impl State {
    /// Hand one event to the sink as the next in sequence. True when it went.
    fn send(&mut self, name: &'static str, payload: Value) -> bool {
        let Some(sink) = &self.sink else {
            return false;
        };
        let message = EventMessage {
            seq: self.sent + 1,
            name,
            payload,
            missed: (self.missed > 0).then_some(self.missed),
        };
        if !sink.deliver(&message) {
            return false;
        }
        self.sent = message.seq;
        self.missed = 0;
        true
    }
}

#[cfg(test)]
pub mod testing {
    //! A sink that records, and one that takes anything, for the tests of everything above the pipe.

    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(Clone, Default)]
    pub struct Recorder {
        pub messages: Arc<Mutex<Vec<EventMessage>>>,
        /// Flip to make the next sends fail, as a channel whose page is gone does.
        pub dead: Arc<AtomicBool>,
    }

    impl Recorder {
        pub fn outbox(&self) -> Arc<Outbox> {
            Outbox::new(self.clone())
        }

        pub fn names(&self) -> Vec<&'static str> {
            self.messages
                .lock()
                .unwrap()
                .iter()
                .map(|m| m.name)
                .collect()
        }

        pub fn all(&self) -> Vec<EventMessage> {
            self.messages.lock().unwrap().clone()
        }

        pub fn payloads(&self, name: &str) -> Vec<Value> {
            self.all()
                .into_iter()
                .filter(|m| m.name == name)
                .map(|m| m.payload)
                .collect()
        }
    }

    impl EventSink for Recorder {
        fn deliver(&self, message: &EventMessage) -> bool {
            if self.dead.load(Ordering::SeqCst) {
                return false;
            }
            self.messages.lock().unwrap().push(message.clone());
            true
        }
    }

    /// An outbox that accepts everything and keeps nothing.
    pub fn discarding() -> Arc<Outbox> {
        struct Discard;
        impl EventSink for Discard {
            fn deliver(&self, _: &EventMessage) -> bool {
                true
            }
        }
        Outbox::new(Discard)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Recorder;
    use super::*;
    use std::sync::atomic::Ordering;

    fn small() -> Value {
        json!({ "ids": [1, 2, 3] })
    }

    #[test]
    fn events_go_out_in_order_with_their_sequence() {
        let rec = Recorder::default();
        let outbox = rec.outbox();
        for n in 0..3 {
            assert_eq!(
                outbox.deliver("documents.deleted", json!({ "n": n })),
                Delivery::Sent
            );
        }
        let seqs: Vec<u64> = rec.all().iter().map(|m| m.seq).collect();
        assert_eq!(seqs, [1, 2, 3]);
        assert!(rec.all().iter().all(|m| m.missed.is_none()));
        assert_eq!(outbox.stats().sent, 3);
    }

    #[test]
    fn the_window_holds_a_frame_that_never_acknowledges_and_counts_what_it_drops() {
        let rec = Recorder::default();
        let outbox = rec.outbox();
        for _ in 0..limits::EVENT_WINDOW {
            assert_eq!(outbox.deliver("documents.deleted", small()), Delivery::Sent);
        }
        // The 257th and every one after it: dropped, nothing queued, nothing sent.
        for _ in 0..1000 {
            assert_eq!(
                outbox.deliver("documents.deleted", small()),
                Delivery::Dropped
            );
        }
        assert_eq!(rec.all().len() as u64, limits::EVENT_WINDOW);
        let stats = outbox.stats();
        assert_eq!((stats.sent, stats.acked, stats.dropped), (256, 0, 1000));

        // The frame catches up in part: the window slides by exactly that much, and the first event
        // after the gap says how many it missed.
        outbox.ack(10);
        for _ in 0..10 {
            assert_eq!(outbox.deliver("documents.deleted", small()), Delivery::Sent);
        }
        assert_eq!(
            outbox.deliver("documents.deleted", small()),
            Delivery::Dropped
        );
        let all = rec.all();
        assert_eq!((all[256].seq, all[256].missed), (257, Some(1000)));
        assert_eq!(
            all[257].missed, None,
            "the count is cleared once it is told"
        );
        // The one dropped since is reported by the next event that goes out.
        outbox.ack(300);
        assert_eq!(outbox.deliver("documents.deleted", small()), Delivery::Sent);
        let last = rec.all().last().cloned().unwrap();
        assert_eq!((last.seq, last.missed), (267, Some(1)));
    }

    #[test]
    fn an_acknowledgement_can_never_run_ahead_of_what_was_sent_or_go_back() {
        let outbox = Recorder::default().outbox();
        outbox.deliver("documents.deleted", small());
        outbox.deliver("documents.deleted", small());
        // A frame claiming events PM hasn't sent opens no room beyond the ones it has.
        outbox.ack(u64::MAX);
        assert_eq!(outbox.stats().acked, 2);
        outbox.deliver("documents.deleted", small());
        outbox.ack(1);
        assert_eq!(
            outbox.stats().acked,
            2,
            "an old acknowledgement changes nothing"
        );
        outbox.ack(0);
        assert_eq!(outbox.stats().acked, 2);
    }

    #[test]
    fn an_event_over_the_size_cap_is_dropped_and_counted_not_sent() {
        let rec = Recorder::default();
        let outbox = rec.outbox();
        let huge = json!({ "title": "x".repeat(limits::MAX_EVENT_BYTES) });
        assert_eq!(outbox.deliver("documents.added", huge), Delivery::Dropped);
        assert!(rec.all().is_empty());
        let fits = json!({ "title": "x".repeat(limits::MAX_EVENT_BYTES - 64) });
        assert_eq!(outbox.deliver("documents.added", fits), Delivery::Sent);
        assert_eq!(rec.all()[0].missed, Some(1));
    }

    #[test]
    fn a_closed_outbox_sends_nothing_ever_again() {
        let rec = Recorder::default();
        let outbox = rec.outbox();
        outbox.deliver("documents.deleted", small());
        outbox.close();
        assert!(outbox.stats().closed);
        for _ in 0..5 {
            assert_eq!(
                outbox.deliver("documents.deleted", small()),
                Delivery::Closed
            );
        }
        outbox.finish(Ending::Paused.notices());
        assert_eq!(rec.names(), ["documents.deleted"]);
    }

    #[test]
    fn a_channel_that_fails_closes_the_outbox() {
        let rec = Recorder::default();
        let outbox = rec.outbox();
        assert_eq!(outbox.deliver("documents.deleted", small()), Delivery::Sent);
        rec.dead.store(true, Ordering::SeqCst);
        assert_eq!(
            outbox.deliver("documents.deleted", small()),
            Delivery::Closed
        );
        rec.dead.store(false, Ordering::SeqCst);
        assert_eq!(
            outbox.deliver("documents.deleted", small()),
            Delivery::Closed,
            "a channel that failed once stays dropped"
        );
        assert_eq!(rec.all().len(), 1);
    }

    #[test]
    fn goodbyes_bypass_a_full_window_then_close() {
        let rec = Recorder::default();
        let outbox = rec.outbox();
        for _ in 0..limits::EVENT_WINDOW {
            outbox.deliver("documents.deleted", small());
        }
        assert_eq!(
            outbox.deliver("documents.deleted", small()),
            Delivery::Dropped
        );
        outbox.finish(Ending::Paused.notices());
        let names = rec.names();
        assert_eq!(&names[names.len() - 2..], [MOD_PAUSED, SESSION_CLOSED]);
        let reason = rec.payloads(SESSION_CLOSED);
        assert_eq!(reason, [json!({ "reason": "paused" })]);
        // The first goodbye reports what was dropped before it.
        let first = &rec.all()[rec.all().len() - 2];
        assert_eq!(first.missed, Some(1));
        assert!(outbox.stats().closed);
    }

    #[test]
    fn each_ending_says_what_it_says() {
        assert!(Ending::Silent.notices().is_empty());
        assert_eq!(
            Ending::Uninstalled.notices(),
            [(SESSION_CLOSED, json!({ "reason": "uninstalled" }))]
        );
        assert_eq!(
            Ending::Replaced.notices(),
            [(SESSION_CLOSED, json!({ "reason": "replaced" }))]
        );
        assert_eq!(
            Ending::Paused.notices(),
            [
                (MOD_PAUSED, json!({})),
                (SESSION_CLOSED, json!({ "reason": "paused" }))
            ]
        );
    }

    #[test]
    fn the_wire_form_is_camel_case_and_omits_a_missing_missed() {
        let plain = EventMessage {
            seq: 4,
            name: "theme.changed",
            payload: json!({}),
            missed: None,
        };
        assert_eq!(
            serde_json::to_value(&plain).unwrap(),
            json!({ "seq": 4, "name": "theme.changed", "payload": {} })
        );
        let gap = EventMessage {
            missed: Some(2),
            ..plain
        };
        assert_eq!(serde_json::to_value(&gap).unwrap()["missed"], json!(2));
    }
}
