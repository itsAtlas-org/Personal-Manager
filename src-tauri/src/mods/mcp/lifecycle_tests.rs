// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! When the socket exists, on a real Unix socket and the API's test host: it comes up for the open
//! vault only when this device's file says on, goes with the vault, and every hook ends the
//! connections it concerns. Plus the source-level promises: no TCP anywhere in the MCP path, `--mcp`
//! answered before `run()`, and the hooks wired into the places that close a store.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::runtime::Handle;

use super::device;
use super::supervisor::{wanted_if, Lifecycle};
use super::test_support::{open_vault, pair_app, App};
use super::transport::{PlatformTransport, StartError, Transport};
use crate::mods::api::test_support::TestHost;
use crate::mods::api::Host;
use crate::mods::devmode::Here;

struct World {
    host: TestHost,
    here: Here,
    app: App,
}

impl World {
    /// A vault open, an app paired and the device's switch on. Nothing listens yet.
    fn on() -> Self {
        let host = TestHost::new();
        let here = open_vault(&host);
        let app = pair_app(&host, &here, &["tasks.read", "projects.read"]);
        Self { host, here, app }
    }

    fn life(&self) -> &Lifecycle {
        self.host.mcp().life()
    }

    fn reconcile(&self, writes: bool) {
        self.life()
            .reconcile(&self.host, &Handle::current(), writes);
    }

    fn folder(&self) -> PathBuf {
        self.host.dir().join("mcp")
    }

    fn socket(&self) -> PathBuf {
        self.folder().join("mcp.sock")
    }

    fn listening(&self) -> bool {
        self.life().status().listening
    }

    /// A connection that has authenticated as the world's app (answered once, to prove it).
    async fn connect(&self) -> Wire {
        let mut wire = Wire::open(&self.socket()).await;
        wire.send(&json!({"pm_auth": self.app.token}).to_string())
            .await;
        let reply = wire.ask(1, "ping").await;
        assert!(reply.get("result").is_some(), "{reply}");
        wire
    }
}

/// A client of the real socket.
struct Wire {
    read: BufReader<tokio::net::unix::OwnedReadHalf>,
    write: tokio::net::unix::OwnedWriteHalf,
}

impl Wire {
    async fn open(path: &Path) -> Self {
        let (read, write) = UnixStream::connect(path).await.unwrap().into_split();
        Self {
            read: BufReader::new(read),
            write,
        }
    }

    async fn send(&mut self, line: &str) {
        self.write.write_all(line.as_bytes()).await.unwrap();
        self.write.write_all(b"\n").await.unwrap();
    }

    async fn line(&mut self) -> Option<String> {
        let mut text = String::new();
        let n = tokio::time::timeout(Duration::from_secs(5), self.read.read_line(&mut text))
            .await
            .expect("the server answered or closed in time")
            .unwrap_or(0);
        (n > 0).then(|| text.trim_end().to_owned())
    }

    async fn ask(&mut self, id: u64, method: &str) -> Value {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method}).to_string())
            .await;
        serde_json::from_str(&self.line().await.expect("an answer")).unwrap()
    }

    async fn is_closed(&mut self) -> bool {
        self.line().await.is_none()
    }
}

/// A second vault in the host's folder, built as `open_vault` builds the first and made the open one.
/// With `copy_meta_of_first` it carries the first's id (a restored or copied folder).
fn second_vault(host: &TestHost, root_name: &str, copy_meta_of_first: bool) -> Here {
    let root = host.dir().join(root_name);
    std::fs::create_dir_all(&root).unwrap();
    if copy_meta_of_first {
        let name = crate::vault::META_FILENAME;
        std::fs::copy(host.dir().join("vault-root").join(name), root.join(name)).unwrap();
    }
    let meta = crate::vault::ensure_device_meta(&root).unwrap();
    let resolved = crate::vault::resolve_layout(&root, None);
    let master = crate::vault::master_from_db_key_hex(&"00".repeat(32)).unwrap();
    let runtime = crate::VaultRuntime::build(&resolved, &meta, &master);
    host.store().set_vault_runtime(runtime).unwrap();
    Here::open(host.dir(), &meta.vault_id, &root).unwrap()
}

// --- coming up and going down ---------------------------------------------------------------------

#[tokio::test]
async fn nothing_listens_and_nothing_is_made_until_the_device_says_on() {
    let host = TestHost::new();
    let _here = open_vault(&host);
    host.mcp().life().reconcile(&host, &Handle::current(), true);
    assert!(!host.mcp().life().status().listening);
    assert!(!host.dir().join("mcp").exists(), "not even the folder");
}

#[test]
fn a_build_that_does_not_offer_connected_apps_never_wants_a_listener_whatever_the_file_says() {
    // The device file is shared with builds that do offer them (a debug build on the same data
    // folder): on:true for this vault must not bring a socket up where the section is hidden and
    // could neither show it nor switch it off.
    let w = World::on();
    assert!(device::device_state(&w.here).on);
    assert!(wanted_if(&w.host, true, true).is_some());
    assert!(wanted_if(&w.host, true, false).is_none());
    assert!(wanted_if(&w.host, false, true).is_none());
    // And the real `wanted` (what `reconcile` asks) hands the build's own answer to it.
    let code = code_of(&source("src/mods/mcp/supervisor.rs"));
    assert!(code.contains("wanted_if(host, writes, super::commands::offered())"));
}

#[tokio::test]
async fn on_for_this_vault_it_listens_and_serves() {
    let w = World::on();
    w.reconcile(true);
    assert!(w.listening());
    let mode = std::fs::metadata(w.socket()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let mut c = w.connect().await;
    let listed = c.ask(2, "tools/list").await;
    let mut names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    // The scopes the app holds, and no others.
    assert_eq!(names, ["milestones", "projects", "tasks"]);
}

#[tokio::test]
async fn an_instance_that_does_not_write_serves_nothing() {
    let w = World::on();
    w.reconcile(false);
    assert!(!w.listening());
    assert!(!w.socket().exists());
    // And one that stepped back after serving takes the socket down.
    w.reconcile(true);
    let mut c = w.connect().await;
    w.reconcile(false);
    assert!(!w.listening());
    assert!(!w.socket().exists());
    assert!(c.is_closed().await);
}

#[tokio::test]
async fn switching_off_closes_the_connections_and_removes_the_socket() {
    let w = World::on();
    w.reconcile(true);
    let mut c = w.connect().await;
    device::set_on(&w.here, false).unwrap();
    w.reconcile(true);
    assert!(!w.listening());
    assert!(!w.socket().exists());
    assert!(c.is_closed().await);
    // On again, and it comes back.
    device::set_on(&w.here, true).unwrap();
    w.reconcile(true);
    assert!(w.listening());
    assert!(w.socket().exists());
}

#[tokio::test]
async fn reconciling_the_same_vault_again_keeps_its_connections() {
    let w = World::on();
    w.reconcile(true);
    let mut c = w.connect().await;
    w.reconcile(true);
    w.reconcile(true);
    assert!(c.ask(3, "ping").await.get("result").is_some());
}

#[tokio::test]
async fn after_a_stop_the_next_reconcile_starts_again() {
    let w = World::on();
    w.reconcile(true);
    w.life().stop();
    assert!(!w.listening());
    w.reconcile(true);
    assert!(w.listening());
    assert!(w
        .connect()
        .await
        .ask(2, "ping")
        .await
        .get("result")
        .is_some());
}

// --- the hooks ------------------------------------------------------------------------------------

#[tokio::test]
async fn a_closed_store_takes_the_listener_and_every_connection_down() {
    let w = World::on();
    w.reconcile(true);
    let mut a = w.connect().await;
    let mut unauthenticated = Wire::open(&w.socket()).await;
    w.life().stop();
    assert!(!w.listening());
    assert!(!w.socket().exists());
    assert!(!w.folder().join("mcp.pid").exists());
    assert!(a.is_closed().await);
    assert!(unauthenticated.is_closed().await);
    assert!(UnixStream::connect(w.socket()).await.is_err());
}

#[tokio::test]
async fn a_lock_ends_the_connections_and_the_unlocked_pm_is_reached_again() {
    let w = World::on();
    w.reconcile(true);
    let mut c = w.connect().await;
    w.host.0.locked.store(true, Ordering::SeqCst);
    w.life().close_all();
    assert!(c.is_closed().await);
    // The listener stays; while locked a new connection is closed at its auth.
    assert!(w.listening());
    let mut locked = Wire::open(&w.socket()).await;
    locked
        .send(&json!({"pm_auth": w.app.token}).to_string())
        .await;
    assert!(locked.is_closed().await);
    // Unlocked, the app connects again.
    w.host.0.locked.store(false, Ordering::SeqCst);
    assert!(w
        .connect()
        .await
        .ask(2, "ping")
        .await
        .get("result")
        .is_some());
}

#[tokio::test]
async fn a_revoked_app_loses_its_connections_and_no_one_elses() {
    let w = World::on();
    w.reconcile(true);
    let other = pair_app(&w.host, &w.here, &["tasks.read"]);
    let mut mine = w.connect().await;
    let mut theirs = Wire::open(&w.socket()).await;
    theirs
        .send(&json!({"pm_auth": other.token}).to_string())
        .await;
    assert!(theirs.ask(1, "ping").await.get("result").is_some());
    w.life().close_client(w.app.id);
    assert!(mine.is_closed().await);
    assert!(theirs.ask(2, "ping").await.get("result").is_some());
}

#[tokio::test]
async fn a_vault_switch_serves_the_new_vault_only_if_its_device_file_says_on() {
    let w = World::on();
    w.reconcile(true);
    let mut c = w.connect().await;
    // Another vault, off for this device: the listener goes, with its connection.
    let here2 = second_vault(&w.host, "vault-two", false);
    w.reconcile(true);
    assert!(!w.listening());
    assert!(!w.socket().exists());
    assert!(c.is_closed().await);
    // Switched on there, it comes up for that vault.
    device::set_on(&here2, true).unwrap();
    w.reconcile(true);
    assert!(w.listening());
    assert!(w.socket().exists());
}

#[tokio::test]
async fn a_copied_vault_comes_up_off() {
    let w = World::on();
    w.reconcile(true);
    assert!(w.listening());
    // The same vault id in another folder (a restore or a copied folder): this device never switched
    // it on there.
    let copy = second_vault(&w.host, "vault-copy", true);
    assert_eq!(copy.vault(), w.here.vault());
    assert!(!device::device_state(&copy).on);
    w.reconcile(true);
    assert!(!w.listening());
    assert!(!w.socket().exists());
}

#[tokio::test]
async fn a_folder_that_isnt_private_is_reported_and_nothing_listens() {
    let w = World::on();
    std::fs::create_dir(w.folder()).unwrap();
    std::fs::set_permissions(w.folder(), std::fs::Permissions::from_mode(0o755)).unwrap();
    w.reconcile(true);
    let status = w.life().status();
    assert!(!status.listening);
    assert_eq!(status.error, Some(StartError::UnsafeFolder));
    assert!(!w.socket().exists());
    // Put right, the next reconcile starts and the error is gone.
    std::fs::set_permissions(w.folder(), std::fs::Permissions::from_mode(0o700)).unwrap();
    w.reconcile(true);
    let status = w.life().status();
    assert!(status.listening);
    assert_eq!(status.error, None);
}

#[tokio::test]
async fn a_second_copy_does_not_take_over_a_running_listener() {
    let w = World::on();
    w.reconcile(true);
    // Another PM's lifecycle, over the same folder, starting while the first one serves.
    assert_eq!(
        PlatformTransport::new(w.host.dir()).bind().unwrap_err(),
        StartError::InUse
    );
    // The running one is not disturbed.
    assert!(w
        .connect()
        .await
        .ask(2, "ping")
        .await
        .get("result")
        .is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_shim_reaches_the_real_listener_end_to_end() {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    /// stdout as the test reads it.
    #[derive(Clone, Default)]
    struct Out(Arc<Mutex<Vec<u8>>>);
    impl Write for Out {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let w = World::on();
    w.reconcile(true);
    let out = Out::default();
    let sink = out.clone();
    let token = w.app.token.clone();
    // PM's own data folder as the shim derives it (the dev override, which these tests are built
    // with): the host's folder.
    let data = w.host.dir().to_str().unwrap().to_owned();
    let shim = tokio::task::spawn_blocking(move || {
        let stdin = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n"
        );
        crate::mcp_shim::run(
            move |name: &str| match name {
                "PM_MCP_TOKEN" => Some(token.clone().into()),
                "PM_DATA_DIR" => Some(data.clone().into()),
                _ => None,
            },
            std::io::Cursor::new(stdin.as_bytes().to_vec()),
            sink,
            crate::mcp_shim::connect_real,
        )
    });
    assert_eq!(shim.await.unwrap(), 0);
    let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
    let lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert_eq!(lines[0]["result"]["serverInfo"]["name"], "Personal Manager");
    assert_eq!(lines[1]["result"]["tools"].as_array().unwrap().len(), 3);
    assert!(!text.contains(&w.app.token));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_shim_outlives_pms_close_and_reconnects_on_the_next_line() {
    use std::io::{Cursor, Read, Write};
    use std::sync::{mpsc, Arc, Mutex};

    #[derive(Clone, Default)]
    struct Out(Arc<Mutex<Vec<u8>>>);
    impl Write for Out {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Out {
        fn answers(&self) -> usize {
            String::from_utf8(self.0.lock().unwrap().clone())
                .unwrap()
                .lines()
                .count()
        }
    }

    /// The client's stdin, a line at a time as the test types them.
    struct Typed(mpsc::Receiver<Vec<u8>>, Cursor<Vec<u8>>);
    impl Read for Typed {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            loop {
                let n = self.1.read(buf)?;
                if n > 0 {
                    return Ok(n);
                }
                match self.0.recv() {
                    Ok(next) => self.1 = Cursor::new(next),
                    Err(_) => return Ok(0),
                }
            }
        }
    }

    let w = World::on();
    w.reconcile(true);
    let (typing, lines) = mpsc::channel();
    let out = Out::default();
    let sink = out.clone();
    let token = w.app.token.clone();
    let data = w.host.dir().to_str().unwrap().to_owned();
    let shim = tokio::task::spawn_blocking(move || {
        crate::mcp_shim::run(
            move |name: &str| match name {
                "PM_MCP_TOKEN" => Some(token.clone().into()),
                "PM_DATA_DIR" => Some(data.clone().into()),
                _ => None,
            },
            std::io::BufReader::new(Typed(lines, Cursor::new(Vec::new()))),
            sink,
            crate::mcp_shim::connect_real,
        )
    });
    let wait = |count: usize| {
        let out = out.clone();
        async move {
            for _ in 0..500 {
                if out.answers() >= count {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("no answer number {count}");
        }
    };

    typing
        .send(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n".to_vec())
        .unwrap();
    wait(1).await;
    // PM ends the connection (the idle timeout and a lock do this); the shim must stay.
    w.life().close_all();
    for _ in 0..500 {
        if w.life().open_connections() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(w.life().open_connections(), 0);
    tokio::time::sleep(Duration::from_millis(150)).await;
    typing
        .send(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n".to_vec())
        .unwrap();
    wait(2).await;
    drop(typing);
    assert_eq!(shim.await.unwrap(), 0);
    let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
    let ids: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap()["id"].clone())
        .collect();
    assert_eq!(ids, [json!(1), json!(2)], "{text}");
    assert!(!text.contains(&w.app.token));
}

// --- the source-level promises --------------------------------------------------------------------

fn source(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Code lines of `text` (comments dropped), so a promise in a doc comment doesn't count as use.
fn code_of(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn no_tcp_anywhere_in_the_mcp_path() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/mods/mcp");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        // The tests name what they forbid.
        .filter(|p| {
            !p.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("_tests.rs")
        })
        .collect();
    files.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/mcp_shim.rs"));
    let scanned = files.len();
    for file in files {
        let text = code_of(&std::fs::read_to_string(&file).unwrap());
        for banned in [
            "TcpListener",
            "TcpStream",
            "UdpSocket",
            "0.0.0.0",
            "127.0.0.1",
        ] {
            assert!(!text.contains(banned), "{} uses {banned}", file.display());
        }
    }
    assert!(
        scanned >= 10,
        "the scan found the module's files ({scanned})"
    );
}

#[test]
fn main_answers_mcp_before_it_runs_pm() {
    let code = code_of(&source("src/main.rs"));
    let mcp = code.find("--mcp").expect("main checks --mcp");
    let shim = code.find("run_mcp_shim").expect("main runs the shim");
    let run = code.find("pm_lib::run()").expect("main runs PM");
    assert!(mcp < shim && shim < run, "the shim comes first");
}

/// The code of the top-level function that starts with `signature`, up to its closing brace at the left
/// margin: a call named anywhere else in the file does not count as being in it.
fn body_of<'a>(code: &'a str, signature: &str) -> &'a str {
    let start = code
        .find(signature)
        .unwrap_or_else(|| panic!("no function {signature}"));
    let rest = &code[start..];
    &rest[..rest.find("\n}\n").map_or(rest.len(), |end| end + 3)]
}

/// Whether `body` has `statement` as a whole line at `indent` spaces: at the top of its function or
/// block, not buried in a branch that might never run.
fn has_statement(body: &str, indent: usize, statement: &str) -> bool {
    body.contains(&format!("\n{}{statement}\n", " ".repeat(indent)))
}

#[test]
fn every_place_that_closes_a_store_reaches_the_socket() {
    let ledger = code_of(&source("src/mods/ledger.rs"));
    // `ledger::on_store_closed` is the one every close path calls (lock_session, wipe, vault switch,
    // migrate), `after_settle` is where an open lands, `quiesce` is the wipe's.
    let closed = body_of(&ledger, "pub fn on_store_closed(");
    assert!(has_statement(
        closed,
        4,
        "super::mcp::supervisor::on_store_closed(app);"
    ));
    let settle = body_of(&ledger, "pub fn after_settle(");
    assert!(has_statement(
        settle,
        4,
        "super::mcp::supervisor::after_settle(app, writes);"
    ));
    let quiesce = body_of(&ledger, "pub fn quiesce(");
    assert!(has_statement(
        quiesce,
        4,
        "super::mcp::supervisor::shutdown(app);"
    ));
    // The app lock (only when turning it on), and quitting by hand (nothing runs after
    // `RunEvent::Exit`).
    let settings = code_of(&source("src/settings.rs"));
    let set_lock = body_of(&settings, "pub fn set_app_lock(");
    let enabled = set_lock
        .split("\n    if enabled {\n")
        .nth(1)
        .expect("the lock-on branch");
    assert!(has_statement(
        enabled,
        8,
        "crate::mods::mcp::supervisor::on_lock(&app);"
    ));
    let lib = code_of(&source("src/lib.rs"));
    let exit = lib
        .split("\n            if matches!(event, tauri::RunEvent::Exit) {\n")
        .nth(1)
        .expect("the exit branch");
    let exit = &exit[..exit.find("\n            }\n").unwrap()];
    assert!(has_statement(
        exit,
        16,
        "mods::mcp::supervisor::shutdown(app);"
    ));
}

#[test]
fn each_hook_calls_the_lifecycle_method_it_is_named_for() {
    let code = code_of(&source("src/mods/mcp/supervisor.rs"));
    for (signature, call) in [
        (
            "pub fn after_settle(",
            "life.reconcile(app, &runtime(), writes);",
        ),
        ("pub fn on_store_closed(", "life.stop();"),
        ("pub fn on_lock(", "life.close_all();"),
        ("pub fn on_revoked(", "life.close_client(client);"),
        ("pub fn shutdown(", "life.stop();"),
    ] {
        let body = body_of(&code, signature);
        assert!(
            has_statement(body, 8, call),
            "{signature} calls {call} when the lifecycle exists"
        );
    }
    // The lock leaves the listener up and refusing: it must not take it down.
    assert!(!body_of(&code, "pub fn on_lock(").contains("stop()"));
}

#[test]
fn reconcile_reads_what_is_wanted_only_after_it_holds_the_server_lock() {
    // A stop that races a reconcile must be either wholly before it (the closed store is then what
    // `wanted` reads) or wholly after (it takes down what was started). Reading first and locking
    // after leaves a window for a listener for a closed store. The order itself is the guard.
    let code = code_of(&source("src/mods/mcp/supervisor.rs"));
    // A method: it closes at its own indent, not at the left margin.
    let body = code
        .split("pub fn reconcile<")
        .nth(1)
        .and_then(|rest| rest.split("\n    }\n").next())
        .expect("reconcile");
    let locked = body.find("lock(&self.server)").expect("reconcile locks");
    let asked = body.find("= wanted(host, writes)").expect("reconcile asks");
    assert!(locked < asked);
}

#[tokio::test]
async fn a_fifth_connection_is_closed_unanswered_and_the_first_four_stay() {
    let w = World::on();
    w.reconcile(true);
    let mut held = Vec::new();
    for n in 1..=4 {
        held.push(Wire::open(&w.socket()).await);
        for _ in 0..500 {
            if w.life().open_connections() == n {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(w.life().open_connections(), n, "connection {n} was taken");
    }
    // Nothing has authenticated, and the fifth is already turned away by the real accept loop.
    let mut fifth = Wire::open(&w.socket()).await;
    assert!(fifth.is_closed().await);
    assert_eq!(w.life().open_connections(), 4);
    // The four are untouched: one of them can still authenticate and be served.
    let mut first = held.remove(0);
    first
        .send(&json!({"pm_auth": w.app.token}).to_string())
        .await;
    assert!(first.ask(1, "ping").await.get("result").is_some());
}
