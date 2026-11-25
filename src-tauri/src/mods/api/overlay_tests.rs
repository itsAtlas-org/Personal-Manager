// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `calendar.overlay.set` through the real dispatcher over a real store (PR C, K2): the caller's whole
//! set replaced in one transaction, never another mod's rows, the manifest's declaration as the gate,
//! and every limit refused (not repaired).

use serde_json::{json, Value};

use super::call_fixtures::*;
use super::test_support::TestHost;
use super::{limits, Code, Point};

const OVERLAY: &[&str] = &["calendar.overlay"];

fn entry(id: &str, date: &str, label: &str) -> Value {
    json!({"id": id, "date": date, "label": label})
}

/// A mod that declares the overlay (and needs no permission of its own beyond `storage`).
fn drawer() -> (TestHost, String) {
    world("atlas.drawer", true, "first", &["storage"], OVERLAY)
}

async fn set(host: &TestHost, s: &str, entries: Vec<Value>) -> super::Reply {
    call(host, s, "calendar.overlay.set", json!({"entries": entries})).await
}

fn rows(host: &TestHost, mod_id: &str) -> Vec<(String, String, String)> {
    host.sql(|c| {
        let mut stmt = c
            .prepare("SELECT id, date, label FROM mod_overlay WHERE mod_id = ?1 ORDER BY id")
            .unwrap();
        stmt.query_map([mod_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    })
}

fn row(id: &str, date: &str, label: &str) -> (String, String, String) {
    (id.into(), date.into(), label.into())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_callers_whole_set_is_replaced_and_another_mods_rows_are_left_alone() {
    let (host, s) = drawer();
    let other = add_mod(&host, "atlas.other", true, "first", &["storage"], OVERLAY);
    let v = value(
        set(
            &host,
            &s,
            vec![
                entry("w1:a", "2026-12-01", "Launch"),
                entry("w1:b", "2026-12-09", " Review "),
            ],
        )
        .await,
    );
    assert_eq!(v, json!({"count": 2}));
    value(set(&host, &other, vec![entry("x", "2026-01-01", "Theirs")]).await);
    assert_eq!(
        rows(&host, "atlas.drawer"),
        [
            row("w1:a", "2026-12-01", "Launch"),
            row("w1:b", "2026-12-09", "Review")
        ]
    );
    // A second set is the whole set: what is not in it is gone, what is in it is as sent.
    value(set(&host, &s, vec![entry("w1:b", "2027-01-01", "Moved")]).await);
    assert_eq!(
        rows(&host, "atlas.drawer"),
        [row("w1:b", "2027-01-01", "Moved")]
    );
    // And the other mod's rows were never touched, by either of this one's calls.
    assert_eq!(
        rows(&host, "atlas.other"),
        [row("x", "2026-01-01", "Theirs")]
    );
    // Two mods may use the same entry id: the key is the pair.
    value(set(&host, &other, vec![entry("w1:b", "2026-05-05", "Same id")]).await);
    assert_eq!(rows(&host, "atlas.drawer").len(), 1);
    assert_eq!(rows(&host, "atlas.other").len(), 1);
    // The empty set clears it.
    assert_eq!(value(set(&host, &s, vec![]).await), json!({"count": 0}));
    assert!(rows(&host, "atlas.drawer").is_empty());
    assert_eq!(rows(&host, "atlas.other").len(), 1);
}

/// "Mutation: let `overlay.set` skip the manifest check": a mod that never declared an overlay is
/// refused, whatever it holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mod_whose_manifest_declares_no_overlay_is_refused() {
    for contributions in [&[][..], &["nav.tab"][..], &["help"][..]] {
        let (host, s) = world("atlas.drawer", true, "first", &["storage"], contributions);
        let e = error(set(&host, &s, vec![entry("a", "2026-12-01", "x")]).await);
        assert_eq!(e.code, Code::NotGranted, "{contributions:?}");
        assert!(rows(&host, "atlas.drawer").is_empty());
    }
    // Declared by an update that dropped it: the INSTALLED manifest decides.
    let (host, s) = drawer();
    value(set(&host, &s, vec![entry("a", "2026-12-01", "x")]).await);
    host.sql(|c| {
        let mut manifest: Value = serde_json::from_str(
            &c.query_row(
                "SELECT manifest FROM mods WHERE id = 'atlas.drawer'",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap(),
        )
        .unwrap();
        manifest["contributions"] = json!({});
        c.execute(
            "UPDATE mods SET manifest = ?1 WHERE id = 'atlas.drawer'",
            [manifest.to_string()],
        )
        .unwrap();
    });
    assert_eq!(
        code(set(&host, &s, vec![entry("b", "2026-12-01", "x")]).await),
        Code::NotGranted
    );
    assert_eq!(rows(&host, "atlas.drawer").len(), 1, "what was there stays");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_limit_is_refused_and_a_refusal_leaves_the_old_set_alone() {
    let (host, s) = drawer();
    value(set(&host, &s, vec![entry("keep", "2026-12-01", "Kept")]).await);
    let refused = [
        // The cap.
        (0..=limits::OVERLAY_MAX_ENTRIES)
            .map(|i| entry(&format!("e{i}"), "2026-12-01", "x"))
            .collect::<Vec<_>>(),
        // Ids: too long, empty, hidden characters, a newline.
        vec![entry(
            &"i".repeat(limits::OVERLAY_ID_MAX_CHARS + 1),
            "2026-12-01",
            "x",
        )],
        vec![entry("", "2026-12-01", "x")],
        vec![entry("a\u{200b}b", "2026-12-01", "x")],
        vec![entry("a\nb", "2026-12-01", "x")],
        // Labels: too long, blank, a newline, a control character.
        vec![entry(
            "a",
            "2026-12-01",
            &"l".repeat(limits::OVERLAY_LABEL_MAX_CHARS + 1),
        )],
        vec![entry("a", "2026-12-01", "   ")],
        vec![entry("a", "2026-12-01", "two\nlines")],
        vec![entry("a", "2026-12-01", "bell\u{7}")],
        // Dates: not a real YYYY-MM-DD, or outside 1900-2200.
        vec![entry("a", "2026-1-5", "x")],
        vec![entry("a", "20261201", "x")],
        vec![entry("a", "2026-13-45", "x")],
        vec![entry("a", "2026-02-30", "x")],
        vec![entry("a", "2026-12-01T10:00", "x")],
        vec![entry("a", " 2026-12-01", "x")],
        vec![entry("a", "1899-12-31", "x")],
        vec![entry("a", "2201-01-01", "x")],
        vec![entry("a", "", "x")],
        // The same id twice.
        vec![entry("a", "2026-12-01", "x"), entry("a", "2026-12-02", "y")],
        // One bad entry among good ones refuses the whole set.
        vec![entry("a", "2026-12-01", "ok"), entry("b", "nope", "bad")],
    ];
    for (n, entries) in refused.into_iter().enumerate() {
        assert_eq!(
            code(set(&host, &s, entries).await),
            Code::BadArgs,
            "case {n}"
        );
    }
    for bad in [
        json!({}),
        json!({"entries": "x"}),
        json!({"entries": [], "x": 1}),
        json!({"entries": [{"id": "a"}]}),
        json!({"entries": [{"id": "a", "date": "2026-12-01", "label": "x", "colour": "red"}]}),
    ] {
        assert_eq!(
            code(call(&host, &s, "calendar.overlay.set", bad.clone()).await),
            Code::BadArgs,
            "{bad}"
        );
    }
    assert_eq!(
        rows(&host, "atlas.drawer"),
        [row("keep", "2026-12-01", "Kept")]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_limits_are_exactly_at_their_edges() {
    let (host, s) = drawer();
    let at_cap: Vec<Value> = (0..limits::OVERLAY_MAX_ENTRIES)
        .map(|i| entry(&format!("e{i}"), "2026-12-01", "x"))
        .collect();
    assert_eq!(
        value(set(&host, &s, at_cap).await),
        json!({"count": limits::OVERLAY_MAX_ENTRIES})
    );
    let edges = vec![
        entry(
            &"i".repeat(limits::OVERLAY_ID_MAX_CHARS),
            "1900-01-01",
            &"l".repeat(limits::OVERLAY_LABEL_MAX_CHARS),
        ),
        entry("end", "2200-12-31", "x"),
        entry("leap", "2028-02-29", "x"),
    ];
    assert_eq!(value(set(&host, &s, edges).await), json!({"count": 3}));
    assert!(code_of_refused_leap_day(&host, &s).await);
}

/// What the Pinboard really publishes: ids that are two random UUIDs (73 characters) and labels the
/// person typed, emoji and pasted no-break spaces among them. A mod that republishes its seeded rows
/// must not be refused for them (the Calendar's event ids, hidden sets and Runway key on the ids).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pinboards_real_ids_and_an_emoji_or_pasted_label_are_taken() {
    let (host, s) = drawer();
    let id = "3f2b8c1e-6a4d-4f0b-9d2e-1c7a5b8e9f01:9a7e5d3c-2b1f-4e8a-8c6d-0f4b2a1e7d35";
    assert_eq!(id.chars().count(), 73);
    let entries = vec![
        entry(id, "2026-12-01", "\u{26a0}\u{fe0f} deploy"),
        entry("b", "2026-12-02", "\u{1F468}\u{200d}\u{1F469} family"),
        entry("c", "2026-12-03", "two\u{a0}words"),
    ];
    assert_eq!(value(set(&host, &s, entries).await), json!({"count": 3}));
    assert_eq!(
        rows(&host, "atlas.drawer")[0],
        row(id, "2026-12-01", "\u{26a0}\u{fe0f} deploy")
    );
    // What stays refused: a control character or a hidden one in a label.
    for bad in [
        "tab\there",
        "zero\u{200b}width",
        "flip\u{202e}ed",
        "bell\u{7}",
        // A space of any kind at an end, nothing but an emoji's glue, accents stacked past four.
        "trail\u{a0}",
        "\u{a0}lead",
        "\u{200d}",
        "e\u{301}\u{301}\u{301}\u{301}\u{301}",
    ] {
        assert_eq!(
            code(set(&host, &s, vec![entry("x", "2026-12-01", bad)]).await),
            Code::BadArgs,
            "{bad:?}"
        );
    }
}

async fn code_of_refused_leap_day(host: &TestHost, s: &str) -> bool {
    // 2027 is not a leap year.
    matches!(
        set(host, s, vec![entry("a", "2027-02-29", "x")]).await,
        super::Reply::Error { error } if error.code == Code::BadArgs
    )
}

/// A call that stops being the mod's between the delete and the commit leaves the old set whole: the
/// replacement is one transaction.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_set_whose_call_ends_before_the_commit_changes_nothing() {
    let (host, s) = drawer();
    value(set(&host, &s, vec![entry("old", "2026-12-01", "Old")]).await);
    let session = s.clone();
    host.hook(move |h, point| {
        if point == Point::BeforeCommit {
            h.runtime().close(&session);
        }
    });
    assert_eq!(
        code(set(&host, &s, vec![entry("new", "2026-12-02", "New")]).await),
        Code::SessionEnded
    );
    assert_eq!(
        rows(&host, "atlas.drawer"),
        [row("old", "2026-12-01", "Old")]
    );
}

/// Uninstalling a mod removes its rows (the table cascades from the mod) and no one else's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uninstalling_the_mod_removes_its_rows_and_only_its_rows() {
    let (host, s) = drawer();
    let other = add_mod(&host, "atlas.other", true, "first", &["storage"], OVERLAY);
    value(set(&host, &s, vec![entry("a", "2026-12-01", "x")]).await);
    value(set(&host, &other, vec![entry("a", "2026-12-01", "y")]).await);
    host.sql(|c| {
        c.execute("DELETE FROM mods WHERE id = 'atlas.drawer'", [])
            .unwrap()
    });
    assert!(rows(&host, "atlas.drawer").is_empty());
    assert_eq!(rows(&host, "atlas.other").len(), 1);
}
