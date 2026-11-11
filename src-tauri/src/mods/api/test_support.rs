// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the API's tests share: a real store (SQLCipher, every migration), `mods` rows written the
//! way the ledger writes them, and a [`TestHost`] that is the app as far as the dispatcher can tell.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};
use serde_json::json;
use sha2::{Digest, Sha256};
use tauri::http::Request;

use super::events::Hub;
use super::{Calls, Host, Point};
use crate::mods::engine::Platform;
use crate::mods::ids::ModId;
use crate::mods::outbox::Outbox;
use crate::mods::runtime::{InstalledMod, ModRuntime, ServedFile};
use crate::sidecar::{SidecarManager, SidecarPaths};
use crate::AppState;

/// A key made for tests only, as every other store test in the tree uses.
const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

/// A fresh, fully migrated store in a temporary folder.
pub fn store() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
    (dir, conn)
}

/// A manifest as the ledger stores it, reduced to what the dispatcher reads.
pub fn manifest(permissions: &[&str], keys: &[(&str, &str)]) -> String {
    json!({
        "permissions": permissions.iter().map(|p| json!({"id": p, "why": "test"})).collect::<Vec<_>>(),
        "data": {"keys": keys.iter().map(|(k, c)| json!({"key": k, "class": c})).collect::<Vec<_>>()},
        "entry": {"page": "app/index.html"},
    })
    .to_string()
}

/// One `mods` row.
pub struct ModRow {
    pub id: &'static str,
    pub signed: bool,
    pub tier: &'static str,
    pub permissions: Vec<&'static str>,
    pub keys: Vec<(&'static str, &'static str)>,
    pub state: &'static str,
    pub consented: bool,
}

impl ModRow {
    pub fn signed(id: &'static str, tier: &'static str, permissions: &[&'static str]) -> Self {
        Self {
            id,
            signed: true,
            tier,
            permissions: permissions.to_vec(),
            keys: Vec::new(),
            state: "active",
            consented: true,
        }
    }

    pub fn unsigned(id: &'static str, permissions: &[&'static str]) -> Self {
        Self {
            signed: false,
            ..Self::signed(id, "third", permissions)
        }
    }
}

pub fn insert_mod(conn: &Connection, row: &ModRow) {
    conn.execute(
        "INSERT INTO mods(id, name, version, tier, author, source_repo, source_commit, signed, \
                          install_id, installed_from, package_sha256, manifest, files, state, \
                          consented_at) \
         VALUES (?1, 'Test', '1.0.0', ?2, 'Test', 'test/test', ?3, ?4, '0123456789abcdef', \
                 'file', ?5, ?6, '[]', ?7, ?8)",
        params![
            row.id,
            row.tier,
            "0".repeat(40),
            row.signed,
            "a".repeat(64),
            manifest(&row.permissions, &row.keys),
            row.state,
            row.consented.then_some("2026-11-11T00:00:00.000Z"),
        ],
    )
    .unwrap();
}

/// Register `m`'s files with the runtime, as the ledger's startup load does for a row it serves.
pub fn serve(host: &TestHost, m: &str) {
    let id = ModId::parse(m).unwrap();
    let root = host.dir().join("mods").join(m);
    std::fs::create_dir_all(root.join("app")).unwrap();
    let page = b"<!doctype html><p>mod";
    std::fs::write(root.join("app/index.html"), page).unwrap();
    let file = ServedFile {
        sha256: Sha256::digest(page).into(),
        size: page.len() as u64,
    };
    let installed = InstalledMod::from_ledger(
        id,
        &root,
        "app/index.html",
        [("app/index.html".into(), file)],
    )
    .unwrap();
    host.runtime().install(installed);
}

/// A live frame session for `m` (served already) over the open store, its events going to `outbox`:
/// minted, and its entry page served.
pub fn live_session(host: &TestHost, m: &str, outbox: Arc<Outbox>) -> String {
    let opened = host
        .runtime()
        .open_frame_in(
            &ModId::parse(m).unwrap(),
            host.store().store_epoch(),
            outbox,
        )
        .unwrap();
    let req = Request::builder()
        .uri(opened.entry_url.as_str())
        .body(Vec::new())
        .unwrap();
    assert_eq!(host.runtime().serve("main", &req).status(), 200);
    opened.session
}

/// What a test hooks into a call in flight.
pub type Hook = Arc<dyn Fn(&TestHost, Point) + Send + Sync>;

pub struct Parts {
    dir: tempfile::TempDir,
    pub state: AppState,
    pub runtime: ModRuntime,
    pub calls: Calls,
    pub hub: Hub,
    pub sealed: AtomicBool,
    /// PM locked, as `app_locked_for_mods` would say on a platform with a lock.
    pub locked: AtomicBool,
    hook: Mutex<Option<Hook>>,
}

/// The app, as far as the dispatcher can tell: a real `AppState` over a real store, a real runtime,
/// real limits; the engine's seal and the app lock set by the test.
#[derive(Clone)]
pub struct TestHost(pub Arc<Parts>);

impl TestHost {
    /// A host whose events wait an hour for their interval, so a test drives each flush itself and
    /// no timer races it.
    pub fn new() -> Self {
        Self::with_interval(std::time::Duration::from_secs(3600))
    }

    /// A host whose events wait `interval` before they are flushed, as the real timer does.
    pub fn with_interval(interval: std::time::Duration) -> Self {
        let (dir, conn) = store();
        let sidecar = SidecarManager::new(SidecarPaths {
            source_dir: dir.path().join("sidecar"),
            venv_dir: dir.path().join("runtime").join("venv"),
        });
        Self(Arc::new(Parts {
            state: AppState::new(Some(conn), None, sidecar, None, None),
            runtime: ModRuntime::new(Platform::Linux, vec!["tauri://localhost".into()]),
            calls: Calls::default(),
            hub: Hub::with_delay(interval),
            sealed: AtomicBool::new(true),
            locked: AtomicBool::new(false),
            hook: Mutex::new(None),
            dir,
        }))
    }

    /// A folder of this host's own, for the mods' files.
    pub fn dir(&self) -> &std::path::Path {
        self.0.dir.path()
    }

    /// Run `f` on the store, the way a writer outside the dispatcher would (the guard dropped after).
    pub fn sql<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        let conn = self.0.state.conn().unwrap();
        f(&conn)
    }

    pub fn hook(&self, hook: impl Fn(&TestHost, Point) + Send + Sync + 'static) {
        *self.0.hook.lock().unwrap() = Some(Arc::new(hook));
    }
}

impl Host for TestHost {
    fn store(&self) -> &AppState {
        &self.0.state
    }

    fn runtime(&self) -> &ModRuntime {
        &self.0.runtime
    }

    fn calls(&self) -> &Calls {
        &self.0.calls
    }

    fn hub(&self) -> &Hub {
        &self.0.hub
    }

    fn engine_sealed(&self) -> bool {
        self.0.sealed.load(Ordering::SeqCst)
    }

    /// The real check (which errors on a closed store), then the test's lock on top.
    fn app_locked(&self) -> crate::error::Result<bool> {
        let real = crate::settings::app_locked_for_mods(&self.0.state)?;
        let answer = real || self.0.locked.load(Ordering::SeqCst);
        // After the read, before the caller acts on it: where a lock enabled "just now" lands.
        self.at(Point::LockRead);
        Ok(answer)
    }

    fn at(&self, point: Point) {
        // Cloned out so the slot isn't locked while the hook runs: a hook that waits (the
        // in-flight test's gate) must not hold every other call's hook up behind it.
        let hook = self.0.hook.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook(self, point);
        }
    }
}
