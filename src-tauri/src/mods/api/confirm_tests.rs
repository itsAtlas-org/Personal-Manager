// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The confirm registry end to end (PR C, K1): a handler's request, the page's answer
//! (`mod_confirm`), and every way the question dies, through a real store, a real runtime and a real
//! session. No call uses the registry until K2, so the question here is the test's own
//! ([`Action::Probe`]): it sets one of the mod's own `mod_kv` rows to `"confirmed"`, and is asked
//! about that row's current value.

use serde_json::{json, Value};

use super::confirm::{complete, request};
use super::limits::Family;
use super::permissions::Class;
use super::table::{Kind, Row};
use super::test_support::{insert_mod, live_session, serve, ModRow, TestHost};
use super::{Authorised, Code, Host, ModError, Reply};
use crate::mods::confirm::Action;
use crate::mods::ids::ModId;
use crate::mods::outbox::testing::discarding;

const PROBE: Row = Row {
    name: "test.confirmProbe",
    kind: Kind::Call,
    permission: Some("storage"),
    class: Class::Local,
    added: (1, 0),
    deprecated: None,
    removed: None,
    experimental: false,
    family: Family::General,
};

fn id(s: &str) -> ModId {
    ModId::parse(s).unwrap()
}

/// A first-party mod with one stored value, `note = "before"`, and its live session.
fn world() -> (TestHost, String) {
    let host = TestHost::new();
    host.sql(|c| {
        insert_mod(c, &ModRow::signed("atlas.pin", "first", &["storage"]));
        c.execute(
            "INSERT INTO mod_kv(mod_id, key, class, value) VALUES ('atlas.pin', 'note', 'content', '\"before\"')",
            [],
        )
        .unwrap();
    });
    serve(&host, "atlas.pin");
    let session = live_session(&host, "atlas.pin", discarding());
    (host, session)
}

fn note(host: &TestHost) -> String {
    host.sql(|c| {
        c.query_row(
            "SELECT value FROM mod_kv WHERE mod_id = 'atlas.pin' AND key = 'note'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    })
}

fn probe() -> Action {
    Action::Probe { key: "note".into() }
}

/// The call's ticket for `session`, as the dispatcher would have taken it.
fn auth(host: &TestHost, session: &str) -> Authorised {
    Authorised {
        ticket: host.runtime().begin_call(session).unwrap(),
        row: &PROBE,
    }
}

/// A handler's request for `action`, under the guard, as K2's handlers will make it: the error it
/// answers with.
fn ask(host: &TestHost, auth: &Authorised, action: Action) -> ModError {
    host.sql(|c| request(host, c, auth, action))
}

/// The token of a `MOD-2003`.
fn token_of(e: &ModError) -> String {
    assert_eq!(e.code, Code::NeedsConfirmation, "{e}");
    e.confirm.as_ref().expect("a question").token.clone()
}

async fn answer(host: &TestHost, token: &str, accept: bool) -> Result<Value, ModError> {
    match complete(host, "main", token, accept).await {
        Reply::Value { value } => Ok(value),
        Reply::Error { error } => Err(error),
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_carries_a_token_and_a_question_built_from_pms_data_and_accept_does_it_once() {
    let (host, s) = world();
    let e = ask(&host, &auth(&host, &s), probe());
    assert_eq!(e.code, Code::NeedsConfirmation);
    let question = e.confirm.clone().unwrap();
    // 128 bits as hex, and a question made from the stored value, not from anything the call said.
    assert_eq!(question.token.len(), 32);
    assert!(question.token.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(question.summary, "Let the test use \"\"before\"\"?");
    // Nothing happens until the answer.
    assert_eq!(note(&host), "\"before\"");
    assert_eq!(
        answer(&host, &question.token, true).await.unwrap(),
        json!({"done": true})
    );
    assert_eq!(note(&host), "\"confirmed\"");
    // Single use: the same token again does nothing, even when the world has gone back.
    host.sql(|c| {
        c.execute("UPDATE mod_kv SET value = '\"before\"'", [])
            .unwrap();
    });
    let again = answer(&host, &question.token, true).await.unwrap_err();
    assert_eq!(again.code, Code::Declined);
    assert_eq!(note(&host), "\"before\"", "a used token performed it again");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_decline_answers_declined_does_nothing_and_spends_the_token() {
    let (host, s) = world();
    let token = token_of(&ask(&host, &auth(&host, &s), probe()));
    let e = answer(&host, &token, false).await.unwrap_err();
    assert_eq!(
        (e.code, e.message),
        (Code::Declined, ModError::declined().message)
    );
    assert_eq!(note(&host), "\"before\"");
    // The no is final: the same token can't be turned into a yes.
    assert_eq!(
        answer(&host, &token, true).await.unwrap_err().code,
        Code::Declined
    );
    assert_eq!(note(&host), "\"before\"");
    // And the slot is free for the next question.
    token_of(&ask(&host, &auth(&host, &s), probe()));
}

/// A token the registry doesn't hold does nothing: never made, a near miss, or a forged page's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forged_or_stale_token_does_nothing() {
    let (host, s) = world();
    let token = token_of(&ask(&host, &auth(&host, &s), probe()));
    let mut near = token.clone();
    near.replace_range(..1, if near.starts_with('0') { "1" } else { "0" });
    for forged in ["", "0", &"0".repeat(32), &"f".repeat(64), near.as_str()] {
        let e = answer(&host, forged, true).await.unwrap_err();
        assert_eq!(e.code, Code::Declined, "{forged:?}");
    }
    assert_eq!(note(&host), "\"before\"");
    // The real one is untouched by the forgeries.
    assert!(answer(&host, &token, true).await.is_ok());
}

/// The answer comes from PM's main window only; another window's attempt is refused and spends
/// nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_main_window_may_answer_and_a_refused_window_spends_nothing() {
    let (host, s) = world();
    let token = token_of(&ask(&host, &auth(&host, &s), probe()));
    match complete(&host, "popout", &token, true).await {
        Reply::Error { error } => assert_eq!(error.code, Code::NotGranted),
        other => panic!("{other:?}"),
    }
    assert_eq!(note(&host), "\"before\"");
    assert!(answer(&host, &token, true).await.is_ok());
}

/// Each session has its own place, and a token acts for the session that asked, never another's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_token_belongs_to_its_session_and_each_session_has_its_own_place() {
    let (host, a) = world();
    host.sql(|c| {
        insert_mod(c, &ModRow::signed("atlas.other", "first", &["storage"]));
        c.execute(
            "INSERT INTO mod_kv(mod_id, key, class, value) VALUES ('atlas.other', 'note', 'content', '\"theirs\"')",
            [],
        )
        .unwrap();
    });
    serve(&host, "atlas.other");
    let b = live_session(&host, "atlas.other", discarding());
    let a2 = live_session(&host, "atlas.pin", discarding());
    // A is waiting; B and A's second window are not blocked by it.
    let ta = token_of(&ask(&host, &auth(&host, &a), probe()));
    let tb = token_of(&ask(&host, &auth(&host, &b), probe()));
    let ta2 = token_of(&ask(&host, &auth(&host, &a2), probe()));
    assert!(ta != tb && ta != ta2);
    // B's token acts on B's mod only.
    answer(&host, &tb, true).await.unwrap();
    let other: String = host.sql(|c| {
        c.query_row(
            "SELECT value FROM mod_kv WHERE mod_id = 'atlas.other'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    });
    assert_eq!(other, "\"confirmed\"");
    assert_eq!(note(&host), "\"before\"", "B's answer touched A's mod");
    // A new session over the same mod can't use A's old token once A has gone.
    host.runtime().close(&a);
    let a3 = live_session(&host, "atlas.pin", discarding());
    let _ = a3;
    assert_eq!(
        answer(&host, &ta, true).await.unwrap_err().code,
        Code::Declined
    );
    assert_eq!(note(&host), "\"before\"");
    // A's sibling window's own question is still good.
    answer(&host, &ta2, true).await.unwrap();
}

/// What the question was about changed while it was open: refused, nothing done.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_question_whose_target_changed_is_refused_and_nothing_is_done() {
    let (host, s) = world();
    let token = token_of(&ask(&host, &auth(&host, &s), probe()));
    host.sql(|c| {
        c.execute("UPDATE mod_kv SET value = '\"edited meanwhile\"'", [])
            .unwrap();
    });
    let e = answer(&host, &token, true).await.unwrap_err();
    assert_eq!(e.code, Code::StaleRevision);
    assert_eq!(note(&host), "\"edited meanwhile\"");
    // The token was spent by the attempt, so it can't be retried against the new state.
    assert_eq!(
        answer(&host, &token, true).await.unwrap_err().code,
        Code::Declined
    );
    // A target that is gone is gone, not a yes.
    let token = token_of(&ask(&host, &auth(&host, &s), probe()));
    host.sql(|c| {
        c.execute("DELETE FROM mod_kv", []).unwrap();
    });
    assert_eq!(
        answer(&host, &token, true).await.unwrap_err().code,
        Code::NotFound
    );
}

/// A second request while one is waiting is refused (no queue), and the first stays good.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_request_while_one_waits_is_refused_and_the_first_stays() {
    let (host, s) = world();
    let first = token_of(&ask(&host, &auth(&host, &s), probe()));
    let e = ask(&host, &auth(&host, &s), probe());
    assert_eq!(e.code, Code::RateLimited);
    assert!(e.confirm.is_none());
    assert_eq!(
        ask(&host, &auth(&host, &s), probe()).code,
        Code::RateLimited,
        "still refused, not queued"
    );
    assert!(answer(&host, &first, true).await.is_ok());
    assert_eq!(note(&host), "\"confirmed\"");
    // Answered, so the next one may be asked.
    token_of(&ask(&host, &auth(&host, &s), probe()));
}

/// A mod that asks again the moment it is told no stops being asked about: the third no in a row ends
/// the questions for a while, with no dialog (the call is answered rate limited), and a yes in between
/// keeps them coming.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mod_that_is_told_no_three_times_in_a_row_is_not_asked_about_again() {
    let (host, s) = world();
    for _ in 0..3 {
        let token = token_of(&ask(&host, &auth(&host, &s), probe()));
        assert_eq!(
            answer(&host, &token, false).await.unwrap_err().code,
            Code::Declined
        );
    }
    let e = ask(&host, &auth(&host, &s), probe());
    assert_eq!(e.code, Code::RateLimited, "no fourth question: {e}");
    assert!(e.confirm.is_none(), "and no token to draw a dialog from");
    assert_eq!(note(&host), "\"before\"");
    // Another session of the same mod has its own count.
    let other = live_session(&host, "atlas.pin", discarding());
    token_of(&ask(&host, &auth(&host, &other), probe()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_yes_between_the_noes_keeps_the_questions_coming() {
    let (host, s) = world();
    for accept in [false, false, true, false, false] {
        let token = token_of(&ask(&host, &auth(&host, &s), probe()));
        let _ = answer(&host, &token, accept).await;
    }
    token_of(&ask(&host, &auth(&host, &s), probe()));
}

/// The answer re-runs the dispatcher's checks: a permission turned off while the question was open
/// stops the action.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_answer_checks_the_call_again_and_a_withdrawn_grant_stops_it() {
    let (host, s) = world();
    let token = token_of(&ask(&host, &auth(&host, &s), probe()));
    host.sql(|c| {
        c.execute(
            "INSERT INTO mod_grants(mod_id, permission, decision) VALUES ('atlas.pin', 'storage', 'denied')",
            [],
        )
        .unwrap();
    });
    let e = answer(&host, &token, true).await.unwrap_err();
    assert_eq!(e.code, Code::NotGranted);
    assert_eq!(note(&host), "\"before\"");
    // The mod paused (its row) while the question was open: the same.
    host.sql(|c| {
        c.execute("DELETE FROM mod_grants", []).unwrap();
    });
    let token = token_of(&ask(&host, &auth(&host, &s), probe()));
    host.sql(|c| c.execute("UPDATE mods SET state = 'paused'", []).unwrap());
    assert_eq!(
        answer(&host, &token, true).await.unwrap_err().code,
        Code::Paused
    );
    assert_eq!(note(&host), "\"before\"");
}

/// A write call the question is asked for (the second-read table's own test call, in the gifts' family):
/// it names a target, so its answer must still find the reads held.
const WRITE_PROBE: Row = Row {
    name: super::second_read::PROBE_CALL,
    kind: Kind::Call,
    permission: Some("tasks.write"),
    class: Class::Write,
    added: (1, 0),
    deprecated: None,
    removed: None,
    experimental: false,
    family: Family::Gifts,
};

/// A first-party mod holding the write permission and the two reads the probe rule names, with one
/// stored value `note = "before"`, and its live session.
fn write_world() -> (TestHost, String) {
    let host = TestHost::new();
    host.sql(|c| {
        let perms = ["tasks.write", "tasks.read", "projects.read"];
        insert_mod(c, &ModRow::signed("atlas.pin", "first", &perms));
        for p in perms {
            c.execute(
                "INSERT INTO mod_grants(mod_id, permission, decision) VALUES ('atlas.pin', ?1, 'granted')",
                [p],
            )
            .unwrap();
        }
        c.execute(
            "INSERT INTO mod_kv(mod_id, key, class, value) VALUES ('atlas.pin', 'note', 'content', '\"before\"')",
            [],
        )
        .unwrap();
    });
    serve(&host, "atlas.pin");
    let session = live_session(&host, "atlas.pin", discarding());
    (host, session)
}

fn write_auth(host: &TestHost, session: &str) -> Authorised {
    Authorised {
        ticket: host.runtime().begin_call(session).unwrap(),
        row: &WRITE_PROBE,
    }
}

/// A write call's second reads are checked again at the answer: a read withdrawn while the question
/// was open stops the action, naming that read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_answer_checks_a_write_calls_second_reads_again() {
    let (host, s) = write_world();
    // Held: it goes through.
    let token = token_of(&ask(&host, &write_auth(&host, &s), probe()));
    assert_eq!(
        answer(&host, &token, true).await.unwrap(),
        json!({"done": true})
    );
    assert_eq!(note(&host), "\"confirmed\"");
    host.sql(|c| {
        c.execute("UPDATE mod_kv SET value = '\"before\"'", [])
            .unwrap();
    });
    // Withdrawn while the question is open: refused, and nothing is done.
    let token = token_of(&ask(&host, &write_auth(&host, &s), probe()));
    host.sql(|c| {
        c.execute(
            "UPDATE mod_grants SET decision = 'denied' WHERE permission = 'projects.read'",
            [],
        )
        .unwrap();
    });
    let e = answer(&host, &token, true).await.unwrap_err();
    assert_eq!(e.code, Code::NotGranted);
    assert_eq!(e.permission, Some("projects.read"));
    assert_eq!(note(&host), "\"before\"");
}

/// A mod's gift quota is checked again at the answer: it may have filled while the person decided, and
/// the action then does nothing. A mod with room (checked for the whole batch) is let through.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_answer_checks_the_gift_quota_again() {
    let (host, s) = write_world();
    let max = super::limits::MAX_GIFTS_PER_MOD as i64;
    // The mod has made all but one of its gifts.
    host.sql(|c| {
        c.execute(
            "INSERT INTO tasks(title) \
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?1) \
             SELECT 'gift ' || i FROM n",
            [max - 1],
        )
        .unwrap();
        c.execute(
            "INSERT INTO mod_ledger(mod_id, mod_name, tier, author, source_repo, signed, \
                                    install_id, task_id) \
             SELECT m.id, 'Test', m.tier, m.author, m.source_repo, m.signed, m.install_id, t.id \
             FROM mods m, tasks t WHERE m.id = 'atlas.pin'",
            [],
        )
        .unwrap();
    });
    let gifting = |adding| Action::Gifting {
        key: "note".into(),
        adding,
    };
    // Two more do not fit...
    let token = token_of(&ask(&host, &write_auth(&host, &s), gifting(2)));
    let e = answer(&host, &token, true).await.unwrap_err();
    assert_eq!(e.code, Code::QuotaExceeded);
    assert_eq!(note(&host), "\"before\"");
    // ...and one does.
    let token = token_of(&ask(&host, &write_auth(&host, &s), gifting(1)));
    answer(&host, &token, true).await.unwrap();
    assert_eq!(note(&host), "\"confirmed\"");
}

/// The family's cap on calls in flight holds at the answer too: with its slots taken, the answer is
/// refused and nothing is done, and the slot is given back when it ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_answer_is_held_to_the_familys_cap_on_calls_in_flight() {
    let (host, s) = write_world();
    let me = id("atlas.pin");
    let now = std::time::Instant::now;
    let token = token_of(&ask(&host, &write_auth(&host, &s), probe()));
    let held: Vec<_> = (0..2)
        .map(|_| {
            host.calls()
                .admit_family(&me, Family::Gifts, now())
                .unwrap()
        })
        .collect();
    let e = answer(&host, &token, true).await.unwrap_err();
    assert_eq!(e.code, Code::RateLimited);
    assert_eq!(note(&host), "\"before\"");
    drop(held);
    // Free again: the next question's answer runs, and the slot is given back.
    let token = token_of(&ask(&host, &write_auth(&host, &s), probe()));
    answer(&host, &token, true).await.unwrap();
    assert_eq!(note(&host), "\"confirmed\"");
    let again: Vec<_> = (0..2)
        .map(|_| {
            host.calls()
                .admit_family(&me, Family::Gifts, now())
                .unwrap()
        })
        .collect();
    drop(again);
}

/// A call's session ending between the check and the commit commits nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_that_ends_before_the_commit_commits_nothing() {
    let (host, s) = world();
    let token = token_of(&ask(&host, &auth(&host, &s), probe()));
    host.hook(|host, at| {
        if at == super::Point::BeforeCommit {
            host.runtime().close_all();
        }
    });
    let e = answer(&host, &token, true).await.unwrap_err();
    assert_eq!(e.code, Code::SessionEnded);
    assert_eq!(note(&host), "\"before\"");
}

/// A locked PM answers locked, and the question is dead anyway.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_locked_pm_answers_nothing() {
    let (host, s) = world();
    let token = token_of(&ask(&host, &auth(&host, &s), probe()));
    host.0
        .locked
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        answer(&host, &token, true).await.unwrap_err().code,
        Code::Locked
    );
    assert_eq!(note(&host), "\"before\"");
}

/// Every way the session ends takes its question with it: the token is gone from the registry, an
/// answer does nothing, and the slot is free for the session's next question.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_end_of_a_session_revokes_its_question() {
    type End = (&'static str, fn(&TestHost, &str));
    let ends: [End; 9] = [
        // The app lock: the session stays, at a new generation.
        ("lock", |host, _| host.runtime().bump_all()),
        // A store closed, or swapped for another vault's (the ledger clears the runtime).
        ("store close", |host, _| host.runtime().clear()),
        ("vault switch", |host, _| {
            let conn = host.store().take_conn().unwrap().unwrap();
            *host.store().db.lock().unwrap() = Some(conn);
            host.runtime().clear();
        }),
        ("uninstall", |host, _| {
            host.runtime().uninstall(&id("atlas.pin"))
        }),
        ("pause", |host, _| host.runtime().pause(&id("atlas.pin"))),
        // The session itself ends (a frame's unmount), and a frame retired by a tab switch is the
        // same close.
        ("session end", |host, s| host.runtime().close(s)),
        ("frame retirement", |host, s| host.runtime().close(s)),
        // PM's page reloaded.
        ("page reload", |host, _| host.runtime().close_all()),
        // The mod reinstalled over itself.
        ("reinstall", |host, _| {
            serve(host, "atlas.pin");
            // `serve` installs the same files: unchanged is kept, so change the entry.
            let root = host.dir().join("mods").join("atlas.pin");
            let page = b"<!doctype html><p>changed";
            std::fs::write(root.join("app/index.html"), page).unwrap();
            let file = crate::mods::runtime::ServedFile {
                sha256: <sha2::Sha256 as sha2::Digest>::digest(page).into(),
                size: page.len() as u64,
            };
            host.runtime().install(
                crate::mods::runtime::InstalledMod::from_ledger(
                    id("atlas.pin"),
                    &root,
                    "app/index.html",
                    [("app/index.html".into(), file)],
                    false,
                )
                .unwrap(),
            );
        }),
    ];
    for (what, end) in ends {
        let (host, s) = world();
        let token = token_of(&ask(&host, &auth(&host, &s), probe()));
        assert!(host.runtime().confirm_held(&s), "{what}: it was held");
        end(&host, &s);
        assert!(
            !host.runtime().confirm_held(&s),
            "{what}: the question survived"
        );
        let e = answer(&host, &token, true).await.unwrap_err();
        assert_eq!(e.code, Code::Declined, "{what}");
        assert_eq!(note(&host), "\"before\"", "{what}: it was done");
    }
}

/// After the lock (the one end the session survives), the session's next question is not held up by
/// the one the lock killed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_a_lock_the_session_may_ask_again() {
    let (host, s) = world();
    let old = token_of(&ask(&host, &auth(&host, &s), probe()));
    host.runtime().bump_all();
    let fresh = token_of(&ask(&host, &auth(&host, &s), probe()));
    assert_ne!(old, fresh);
    assert_eq!(
        answer(&host, &old, true).await.unwrap_err().code,
        Code::Declined
    );
    assert!(answer(&host, &fresh, true).await.is_ok());
}

/// A store swapped under an open question (the epoch moves, nothing cleared it yet): the token is
/// refused by its epoch, not by luck.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_token_minted_over_another_store_is_refused() {
    let (host, s) = world();
    let token = token_of(&ask(&host, &auth(&host, &s), probe()));
    let conn = host.store().take_conn().unwrap().unwrap();
    *host.store().db.lock().unwrap() = Some(conn);
    let e = answer(&host, &token, true).await.unwrap_err();
    assert_eq!(e.code, Code::Declined);
    assert_eq!(note(&host), "\"before\"");
}

/// A suspend's deadline (K5) revokes the question, and revoking nothing says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_suspend_deadline_revokes_the_question() {
    let (host, s) = world();
    assert!(!host.runtime().confirm_revoke(&s));
    let token = token_of(&ask(&host, &auth(&host, &s), probe()));
    assert!(host.runtime().confirm_revoke(&s));
    assert!(!host.runtime().confirm_held(&s));
    assert_eq!(
        answer(&host, &token, true).await.unwrap_err().code,
        Code::Declined
    );
    assert!(!host.runtime().confirm_revoke("no such session"));
}

/// A request from a call whose session has already moved on is refused as ended, and holds nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_from_a_stale_call_is_refused_as_ended() {
    let (host, s) = world();
    let stale = auth(&host, &s);
    host.runtime().bump_all();
    let e = ask(&host, &stale, probe());
    assert_eq!(e.code, Code::SessionEnded);
    assert!(!host.runtime().confirm_held(&s));
}

/// The IPC command is wired the way `mod_call` is: registered, main-window checked inside the
/// answer (the window label is passed to it), and wrapped in `ipc.ts`.
#[test]
fn mod_confirm_is_registered_wrapped_and_hands_its_window_to_the_answer() {
    let lib = include_str!("../../lib.rs");
    assert!(lib.contains("mods::mod_confirm,"));
    let source = include_str!("../mod.rs");
    let production = source.split("#[cfg(test)]\nmod tests").next().unwrap();
    let body = production
        .split("pub async fn mod_confirm(")
        .nth(1)
        .expect("the command");
    let body = body.split("\n}\n").next().unwrap();
    assert!(body.contains("api::confirm::complete(webview.app_handle(), webview.label()"));
    // The page's answer carries a token and a yes/no, and nothing a frame said.
    assert!(body.contains("token: String") && body.contains("accept: bool"));
    assert!(!body.contains("session") && !body.contains("method") && !body.contains("args"));
    let ipc = include_str!("../../../../src/lib/ipc.ts");
    assert!(ipc.contains("invoke<unknown>(\"mod_confirm\", { token, accept })"));
}
