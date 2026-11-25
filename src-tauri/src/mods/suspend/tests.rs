// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The suspend handshake over real sessions and a recording channel: a frame is retired only after its
//! `flushed` or the deadline, one mute frame never holds the others (or a quit), and an answer nobody
//! asked for releases nothing.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::json;

use super::*;
use crate::mods::api::events::LIFECYCLE_SUSPEND;
use crate::mods::api::test_support::{insert_mod, live_session, serve, ModRow, TestHost};
use crate::mods::outbox::testing::Recorder;

/// A frame under test: its live session and what its channel heard.
struct Frame {
    session: String,
    rec: Recorder,
}

fn open(host: &TestHost, m: &'static str) -> Frame {
    host.sql(|c| insert_mod(c, &ModRow::signed(m, "second", &[])));
    serve(host, m);
    let rec = Recorder::default();
    let session = live_session(host, m, rec.outbox());
    Frame { session, rec }
}

fn id(s: &str) -> ModId {
    ModId::parse(s).unwrap()
}

/// A frame that answers `flushed` once it has been told (a frame whose port relays it to PM).
fn answers_when_told(host: &TestHost, frame: &Frame) -> tokio::task::JoinHandle<()> {
    let (host, session, rec) = (host.clone(), frame.session.clone(), frame.rec.clone());
    tokio::spawn(async move {
        for _ in 0..500 {
            if !rec.payloads(LIFECYCLE_SUSPEND).is_empty() {
                host.runtime().flushes().flushed(&session);
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
}

/// Run `suspend` with a guard far past the deadline, so a version that waits forever fails here
/// instead of hanging the run.
async fn run(
    host: &TestHost,
    audience: Audience,
    reason: SuspendReason,
    deadline: Duration,
) -> (Suspended, Duration) {
    let start = Instant::now();
    let done = tokio::time::timeout(
        Duration::from_secs(10),
        suspend(host, audience, reason, deadline),
    )
    .await
    .expect("the suspend never came back: PM would hang here");
    (done, start.elapsed())
}

#[tokio::test]
async fn a_frame_that_flushes_is_released_at_once_and_was_told_why() {
    let host = TestHost::new();
    let a = open(&host, "acme.a");
    let answer = answers_when_told(&host, &a);
    let (done, took) = run(
        &host,
        Audience::Mod(id("acme.a")),
        SuspendReason::Hidden,
        Duration::from_secs(5),
    )
    .await;
    answer.await.unwrap();
    assert_eq!(
        done,
        Suspended {
            flushed: 1,
            ..Default::default()
        }
    );
    assert!(took < Duration::from_secs(2), "{took:?}");
    assert_eq!(
        a.rec.payloads(LIFECYCLE_SUSPEND),
        [json!({ "reason": "hidden" })]
    );
    assert_eq!(
        host.runtime().flushes().waiting_on(),
        0,
        "nothing left waiting"
    );
}

#[tokio::test]
async fn a_mute_frame_is_taken_down_at_the_deadline_and_never_hangs_the_quit() {
    let host = TestHost::new();
    let mute = open(&host, "acme.mute");
    let (done, took) = run(
        &host,
        Audience::Everyone,
        SuspendReason::Quit,
        Duration::from_millis(150),
    )
    .await;
    assert_eq!(
        done,
        Suspended {
            timed_out: 1,
            ..Default::default()
        }
    );
    assert!(
        took >= Duration::from_millis(140),
        "waited for the deadline: {took:?}"
    );
    assert!(took < Duration::from_secs(3), "{took:?}");
    assert_eq!(
        mute.rec.payloads(LIFECYCLE_SUSPEND),
        [json!({ "reason": "quit" })]
    );
    assert_eq!(host.runtime().flushes().waiting_on(), 0);
}

#[tokio::test]
async fn one_deadline_covers_every_frame_and_one_mute_frame_does_not_hold_the_others() {
    let host = TestHost::new();
    let good = open(&host, "acme.good");
    let mute_a = open(&host, "acme.mutea");
    let _mute_b = open(&host, "acme.muteb");
    let answer = answers_when_told(&host, &good);
    let (done, took) = run(
        &host,
        Audience::Everyone,
        SuspendReason::Quit,
        Duration::from_millis(200),
    )
    .await;
    answer.await.unwrap();
    assert_eq!((done.flushed, done.timed_out), (1, 2));
    // Two mute frames waited on together, not one after the other.
    assert!(took < Duration::from_millis(390), "{took:?}");
    assert_eq!(mute_a.rec.payloads(LIFECYCLE_SUSPEND).len(), 1);
}

#[tokio::test]
async fn an_answer_nobody_asked_for_releases_nothing_and_one_frames_answer_is_its_own() {
    let host = TestHost::new();
    let a = open(&host, "acme.a");
    let b = open(&host, "acme.b");
    // An early `flushed`, before any suspend: nobody waits on it, so it counts for nothing later.
    assert!(!host.runtime().flushes().flushed(&a.session));
    assert!(!host.runtime().flushes().flushed("no-such-session"));
    let answer_b = answers_when_told(&host, &b);
    let (done, _) = run(
        &host,
        Audience::Everyone,
        SuspendReason::Updated,
        Duration::from_millis(200),
    )
    .await;
    answer_b.await.unwrap();
    // Only b answered; a's wait was not released by b's answer or by its own early one.
    assert_eq!((done.flushed, done.timed_out), (1, 1));
}

#[tokio::test]
async fn only_the_named_mods_frames_are_told() {
    let host = TestHost::new();
    let a = open(&host, "acme.a");
    let b = open(&host, "acme.b");
    let answer = answers_when_told(&host, &a);
    let (done, _) = run(
        &host,
        Audience::Mod(id("acme.a")),
        SuspendReason::Uninstalled,
        Duration::from_secs(5),
    )
    .await;
    answer.await.unwrap();
    assert_eq!(done.flushed, 1);
    assert!(b.rec.all().is_empty(), "another mod's frame is left alone");
}

#[tokio::test]
async fn a_frame_that_cannot_hear_it_is_not_waited_for() {
    let host = TestHost::new();
    let a = open(&host, "acme.a");
    // PM locked: the emitter tells no one, so nobody is waited on.
    host.0.locked.store(true, Ordering::SeqCst);
    let (done, took) = run(
        &host,
        Audience::Everyone,
        SuspendReason::Locked,
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(
        done,
        Suspended {
            unreachable: 1,
            ..Default::default()
        }
    );
    assert!(took < Duration::from_secs(2), "{took:?}");
    assert!(a.rec.all().is_empty());
}

#[tokio::test]
async fn with_no_frame_there_is_nothing_to_wait_for() {
    let host = TestHost::new();
    let (done, took) = run(
        &host,
        Audience::Everyone,
        SuspendReason::Quit,
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(done, Suspended::default());
    assert!(took < Duration::from_millis(500));
}

#[test]
fn the_page_can_ask_for_exactly_the_seven_reasons_a_frame_is_told() {
    for reason in SuspendReason::ALL {
        assert_eq!(SuspendReason::from_word(reason.word()), Some(reason));
    }
    for bad in ["", "Hidden", "hidden ", "quit\n", "retire", "paused,quit"] {
        assert!(SuspendReason::from_word(bad).is_none(), "{bad:?}");
    }
    assert_eq!(SUSPEND_DEADLINE, Duration::from_millis(1500));
}

/// Every path that takes a mod's frames down on its own goes through the handshake, and none of
/// PM's own quits calls `exit` itself (a plain exit would skip the wait and lose the last edits).
#[test]
fn every_way_a_frame_is_taken_down_by_the_user_asks_the_frame_first() {
    let commands = include_str!("../tab/commands.rs");
    for (command, reason) in [
        ("pub async fn pause_mod", "SuspendReason::Paused"),
        ("pub async fn uninstall_mod", "SuspendReason::Uninstalled"),
        ("pub async fn repair_mod", "SuspendReason::Updated"),
        (
            "pub async fn install_mod_consented",
            "SuspendReason::Updated",
        ),
    ] {
        let start = commands
            .find(command)
            .unwrap_or_else(|| panic!("{command}"));
        let body = &commands[start..];
        let end = body[command.len()..]
            .find("\n#[tauri::command]")
            .map_or(body.len(), |n| n + command.len());
        let body = &body[..end];
        assert!(
            body.contains("suspend::before_change") && body.contains(reason),
            "{command} must tell the frame ({reason}) before it changes the mod"
        );
    }
    let tray = include_str!("../../tray.rs");
    for (n, line) in tray.lines().enumerate() {
        let code = line.split("//").next().unwrap_or("");
        assert!(
            !code.contains("app.exit("),
            "tray.rs line {} quits without the handshake: {line}",
            n + 1
        );
    }
    assert!(tray.contains("crate::mods::suspend::quit(app)"));
}

/// A suspend never waits for the person: the question a frame holds is revoked as the wait ends, so a
/// yes that comes after does nothing (K1's rule, K5's deadline).
#[tokio::test]
async fn a_suspend_revokes_the_question_a_frame_is_holding_and_a_late_yes_does_nothing() {
    use crate::mods::api::limits::Family;
    use crate::mods::api::permissions::Class;
    use crate::mods::api::table::{Kind, Row};
    use crate::mods::confirm::{snapshot_hash, Action, Ask};
    const ROW: Row = Row {
        name: "test.confirm",
        kind: Kind::Call,
        permission: Some("storage"),
        class: Class::Local,
        added: (1, 0),
        deprecated: None,
        removed: None,
        experimental: false,
        family: Family::General,
    };
    let host = TestHost::new();
    let a = open(&host, "acme.a");
    let ticket = host.runtime().begin_call(&a.session).unwrap();
    let token = host
        .runtime()
        .confirm_mint(
            &ticket,
            Ask {
                row: &ROW,
                action: Action::Probe { key: "k".into() },
                snapshot: snapshot_hash("probe", "v"),
            },
        )
        .unwrap();
    assert!(host.runtime().confirm_held(&a.session));
    let (done, _) = run(
        &host,
        Audience::Mod(id("acme.a")),
        SuspendReason::Hidden,
        Duration::from_millis(100),
    )
    .await;
    assert_eq!(done.timed_out, 1);
    assert!(
        !host.runtime().confirm_held(&a.session),
        "the question went with the wait"
    );
    assert!(
        host.runtime()
            .confirm_take(&token, host.store().store_epoch())
            .is_none(),
        "a late yes finds nothing to act on"
    );
}

/// Two suspends on one frame, the first of which runs out of time while the second is still inside its
/// own deadline: the first one's end takes down its own wait only, so the frame's answer, late for the
/// first and in time for the second, still releases the second (a tab-leave wait must not cut a quit's
/// short).
#[tokio::test]
async fn a_suspend_that_times_out_does_not_cut_an_overlapping_one_short() {
    let host = TestHost::new();
    let a = open(&host, "acme.a");
    let first = {
        let host = host.clone();
        tokio::spawn(async move {
            suspend(
                &host,
                Audience::Mod(id("acme.a")),
                SuspendReason::Hidden,
                Duration::from_millis(150),
            )
            .await
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    let second = {
        let host = host.clone();
        tokio::spawn(async move {
            suspend(
                &host,
                Audience::Mod(id("acme.a")),
                SuspendReason::Quit,
                Duration::from_millis(900),
            )
            .await
        })
    };
    tokio::time::sleep(Duration::from_millis(150)).await;
    let first = first.await.unwrap();
    assert_eq!(first.timed_out, 1, "the first ran out of time: {first:?}");
    assert!(
        host.runtime().flushes().flushed(&a.session),
        "the second is still waiting when the frame answers"
    );
    let second = second.await.unwrap();
    assert_eq!(
        second,
        Suspended {
            flushed: 1,
            ..Default::default()
        }
    );
    assert_eq!(host.runtime().flushes().waiting_on(), 0);
}

/// Telling a frame goes through the DB mutex. A guard held for long delays the tell, never the end
/// of the wait: the deadline runs from the start, so a quit is not held by a busy store.
#[tokio::test]
async fn a_busy_store_does_not_push_the_deadline_out() {
    let host = TestHost::new();
    let a = open(&host, "acme.a");
    let guard = host.store().conn().unwrap();
    let (done, took) = run(
        &host,
        Audience::Everyone,
        SuspendReason::Quit,
        Duration::from_millis(200),
    )
    .await;
    drop(guard);
    assert_eq!(done.timed_out, 1, "{done:?}");
    assert!(
        took >= Duration::from_millis(190) && took < Duration::from_secs(2),
        "{took:?}"
    );
    assert_eq!(host.runtime().flushes().waiting_on(), 0);
    drop(a);
}

fn exits_counter() -> (
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
    impl FnOnce() + Send + 'static,
) {
    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = std::sync::Arc::clone(&count);
    (count, move || {
        seen.fetch_add(1, Ordering::SeqCst);
    })
}

/// A quit exits only after the frames' wait, and a second request (a double click on Quit, a second
/// close) neither exits ahead of it nor starts another.
#[tokio::test]
async fn a_quit_exits_after_the_wait_and_a_second_request_does_not_exit_early() {
    static QUITTING: AtomicBool = AtomicBool::new(false);
    let host = TestHost::new();
    let _mute = open(&host, "acme.mute");
    let (exits, exit) = exits_counter();
    let (_, second_exit) = exits_counter();
    let started = Instant::now();
    let deadline = Duration::from_millis(400);
    assert_eq!(
        request_quit(&host, &QUITTING, deadline, exit),
        QuitRequest::Waiting
    );
    let again = exits.clone();
    assert_eq!(
        request_quit(&host, &QUITTING, deadline, move || {
            again.fetch_add(1, Ordering::SeqCst);
            second_exit();
        }),
        QuitRequest::AlreadyWaiting
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(exits.load(Ordering::SeqCst), 0, "exited before the wait");
    for _ in 0..200 {
        if exits.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        started.elapsed() >= Duration::from_millis(390),
        "waited for the mute frame's deadline: {:?}",
        started.elapsed()
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(exits.load(Ordering::SeqCst), 1, "exactly one exit");
}

/// With nothing running there is nothing to wait for: PM exits at once.
#[tokio::test]
async fn a_quit_with_no_frame_running_exits_at_once() {
    static QUITTING: AtomicBool = AtomicBool::new(false);
    let host = TestHost::new();
    let (exits, exit) = exits_counter();
    assert_eq!(
        request_quit(&host, &QUITTING, Duration::from_secs(5), exit),
        QuitRequest::Exited
    );
    assert_eq!(exits.load(Ordering::SeqCst), 1);
}

/// Every way PM exits by itself waits for the frames first: the tray and the window close call
/// `quit`, the OS's own quit (no exit code) is held back for it, and the updater's restart asks the
/// frames to save before it relaunches.
#[test]
fn every_way_pm_exits_on_its_own_asks_the_frames_first() {
    let lib = include_str!("../../lib.rs");
    let handler = lib
        .find("RunEvent::ExitRequested")
        .expect("lib.rs handles the OS's own exit request");
    let handler = &lib[handler..handler + 400];
    assert!(
        handler.contains("code: None")
            && handler.contains("exit_needs_wait")
            && handler.contains("prevent_exit()")
            && handler.contains("suspend::quit(app)"),
        "{handler}"
    );
    let updater = include_str!("../../../../src/lib/useUpdater.ts");
    let ask = updater
        .find("suspendModFrames(\"quit\")")
        .expect("the updater asks the frames to save");
    let install = updater.find("update.install()").unwrap();
    assert!(ask < install, "before the installer can end the app");
}

/// Every function that closes the store (and so ends every frame's session) either asks the frames
/// to save first through the page (`ipc.ts`, `suspendModFrames`) or is on this list with the reason it
/// does not. A new store-closing path fails here until it is put on one side.
#[test]
fn every_path_that_closes_the_store_is_either_asked_first_or_has_a_reason() {
    // (file, function, why no handshake). `None` = the page asks the frames first (checked below).
    const CLOSERS: &[(&str, &str, Option<&str>)] = &[
        ("vault/migrate.rs", "migrate_vault", None), // make private, move, share
        ("commands/vaults.rs", "detach_from_shared_vault", None),
        (
            "lock_session.rs",
            "engage",
            Some("startup: no frame is running yet"),
        ),
        (
            "lock_session.rs",
            "close_store",
            // Not the app lock (that goes through the page). Reached from the vault-lock
            // watcher: a hand-off to another PM instance, a lost baton, a waiting start behind a
            // live holder, or a shared vault that stopped answering. Every one is a runtime
            // event with no page call to hang a handshake on, and the vault is no longer ours to
            // write, so a frame cannot keep saving anyway: only an edit still inside the frame's
            // save debounce is lost. Nothing already saved is lost and nothing waits.
            Some(
                "runtime hand-off or lost vault (lock watcher): not ours to write any more, \
                 so only an edit inside the frame's debounce is lost",
            ),
        ),
        (
            "commands/vaults.rs",
            "delete_shared_vault",
            Some("the vault and its data are being deleted: nothing a frame saves survives"),
        ),
        (
            "commands/vaults.rs",
            "acknowledge_deleted_shared_vault",
            Some("the vault is already deleted by its owner"),
        ),
        (
            "wipe.rs",
            "wipe_pm_data",
            Some("erases PM's data: nothing a frame saves survives"),
        ),
        (
            "wipe.rs",
            "reset_after_open_error",
            Some("the store never opened, so no frame runs"),
        ),
    ];
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found: Vec<(String, String)> = Vec::new();
    for file in [
        "vault/migrate.rs",
        "commands/vaults.rs",
        "lock_session.rs",
        "wipe.rs",
    ] {
        let text = std::fs::read_to_string(src.join(file)).unwrap();
        let mut current = String::new();
        for line in text.lines() {
            let code = line.trim_start();
            if let Some(at) = code.find("fn ") {
                let head = &code[..at];
                if head.is_empty()
                    || head == "pub "
                    || head == "async "
                    || head == "pub async "
                    || head.starts_with("pub(")
                {
                    let name = &code[at + 3..];
                    current = name
                        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .next()
                        .unwrap_or("")
                        .to_owned();
                }
            }
            if line.contains("ledger::on_store_closed(") && !code.starts_with("//") {
                found.push((file.to_owned(), current.clone()));
            }
        }
    }
    found.sort();
    found.dedup();
    let mut known: Vec<(String, String)> = CLOSERS
        .iter()
        .map(|(f, n, _)| ((*f).to_owned(), (*n).to_owned()))
        .collect();
    known.sort();
    assert_eq!(
        found, known,
        "a path that closes the store is new or gone: ask the frames first or say why not"
    );
    // The ones the page asks first: the page's wrappers exist for each command.
    let ipc = include_str!("../../../../src/lib/ipc.ts");
    for command in [
        "make_vault_private",
        "move_vault",
        "create_shareable_vault",
        "adopt_shared_vault",
        "detach_from_shared_vault",
        "switch_to_vault",
    ] {
        let at = ipc
            .find(&format!("\"{command}\""))
            .unwrap_or_else(|| panic!("{command}"));
        let before = &ipc[at.saturating_sub(300)..at];
        assert!(
            before.contains("suspendModFrames(\"vault\")"),
            "{command} is invoked without asking the frames first"
        );
    }
}
