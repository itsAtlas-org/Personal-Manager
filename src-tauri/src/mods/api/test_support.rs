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

use super::ai::ModelEdge;
use super::events::Hub;
use super::{Calls, Host, Point};
use crate::llm_gateway::{CallMeta, CloudArm, LlmOutcome, LocalArm, Provider, RoutePlan};
use crate::local_slot::EndpointClass;
use crate::mods::engine::Platform;
use crate::mods::ids::ModId;
use crate::mods::netfetch::fake::ScriptedHops;
use crate::mods::outbox::Outbox;
use crate::mods::runtime::{InstalledMod, ModRuntime, ServedFile};
use crate::openrouter::{ChatMessage, Completion, Usage};
use crate::secret::Secret;
use crate::sidecar::{SidecarManager, SidecarPaths};
use crate::wire_mark::WireMark;
use crate::AppState;

/// A key made for tests only, as every other store test in the tree uses.
pub const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

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

/// The ledger's store over a test host's open store, with no vault to compare (the host has none).
pub struct OverHost<'a>(pub &'a AppState);

impl crate::mods::ledger::LedgerStore for OverHost<'_> {
    fn with_conn<T>(
        &self,
        _layout: &crate::mods::disk::Layout,
        f: impl FnOnce(&mut Connection) -> Result<T, crate::mods::ledger::LedgerError>,
    ) -> Result<T, crate::mods::ledger::LedgerError> {
        let mut conn = self
            .0
            .conn()
            .map_err(|e| crate::mods::ledger::LedgerError::Store(e.to_string()))?;
        f(&mut conn)
    }

    fn holds_device(&self, _device: &crate::mods::devmode::Here) -> bool {
        true
    }
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
    /// The network, scripted: what `net.fetch` resolves and sends through.
    pub hops: ScriptedHops,
    /// The model, scripted: what `ai.complete` resolves, prices and calls.
    pub model: FakeModel,
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
            hops: ScriptedHops::new(),
            model: FakeModel::default(),
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

    type Hops = ScriptedHops;

    fn hops(&self) -> &ScriptedHops {
        &self.0.hops
    }

    type Model = FakeModel;

    fn model(&self) -> &FakeModel {
        &self.0.model
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

// --- a scripted model ---------------------------------------------------------------------------

/// The route the fake model resolves to (a fresh plan each time: a plan is not `Clone`).
#[derive(Clone, Copy, Debug)]
pub enum PlanSpec {
    /// Nothing can answer.
    None,
    /// The cloud arm alone, these models in order.
    Cloud(&'static [&'static str]),
    /// A local model alone.
    Local,
    /// The local model, falling back to these cloud models.
    LocalThenCloud(&'static [&'static str]),
    /// The On battery policy moved a local request to these cloud models.
    Power(&'static [&'static str]),
}

/// What the fake model answers the next calls with.
#[derive(Clone, Debug)]
pub enum Answer {
    /// A completion. `local` says a local model served it (settles at 0); `model` is the server's
    /// word for which model answered.
    Text {
        text: String,
        model: Option<&'static str>,
        cost: Option<f64>,
        tokens: Option<(i64, i64)>,
        truncated: bool,
        local: bool,
    },
    /// The route failed after its request went out, with provider text that must never reach a mod.
    Fail,
    /// The call never returns (a stalled model), its request already sent.
    Hang,
    /// The route refused before any request was made (Local AI switched off between the route
    /// and the call): nothing went out.
    FailBeforeWire,
    /// The call is still queued (behind a chat, say) and never reaches the wire.
    HangBeforeWire,
    /// The request could not connect (offline): it was announced, and failed before a connection.
    ConnectFail,
}

impl Answer {
    /// A cloud answer: `text`, reported to have cost `cost`.
    pub fn cloud(text: &str, model: &'static str, cost: f64) -> Self {
        Self::Text {
            text: text.into(),
            model: Some(model),
            cost: Some(cost),
            tokens: Some((10, 5)),
            truncated: false,
            local: false,
        }
    }

    /// A local answer.
    pub fn local(text: &str) -> Self {
        Self::Text {
            text: text.into(),
            model: None,
            cost: None,
            tokens: Some((10, 5)),
            truncated: false,
            local: true,
        }
    }
}

/// One call the fake model was asked to run.
#[derive(Clone, Debug, PartialEq)]
pub struct Sent {
    /// `(role, content)` of every message, as the model got them.
    pub messages: Vec<(String, String)>,
    pub max_tokens: u32,
    /// The models of the plan it ran on.
    pub models: Vec<String>,
}

/// The model edge of a [`TestHost`]: scripted routes and answers, and a record of what it was asked.
pub struct FakeModel {
    pub plan: Mutex<PlanSpec>,
    pub class: Mutex<Option<EndpointClass>>,
    pub answer: Mutex<Answer>,
    pub sent: Mutex<Vec<Sent>>,
    pub resolves: std::sync::atomic::AtomicUsize,
    pub class_lookups: std::sync::atomic::AtomicUsize,
    pub refreshes: std::sync::atomic::AtomicUsize,
    /// Calls that reached the model.
    pub entered: std::sync::atomic::AtomicUsize,
    /// When set, a call waits at the model until a permit is added to [`Self::gate`].
    pub gated: AtomicBool,
    pub gate: tokio::sync::Semaphore,
    /// Run when a call reaches the model, before it answers (a test changes the world mid-call).
    pub on_complete: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Run when the route is read: before the call is checked, priced and booked.
    pub on_resolve: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// The person's whole Background list, when a test sets one (otherwise the plan's own models).
    pub listed: Mutex<Option<Vec<String>>>,
    /// When set, the local endpoint's address lookup never returns.
    pub stall_class: AtomicBool,
    /// When set, the price refresh never returns.
    pub stall_prices: AtomicBool,
}

impl Default for FakeModel {
    fn default() -> Self {
        Self {
            plan: Mutex::new(PlanSpec::None),
            class: Mutex::new(Some(EndpointClass::Loopback)),
            answer: Mutex::new(Answer::local("ok")),
            sent: Mutex::new(Vec::new()),
            resolves: Default::default(),
            class_lookups: Default::default(),
            refreshes: Default::default(),
            entered: Default::default(),
            gated: AtomicBool::new(false),
            gate: tokio::sync::Semaphore::new(0),
            on_complete: Mutex::new(None),
            on_resolve: Mutex::new(None),
            listed: Mutex::new(None),
            stall_class: AtomicBool::new(false),
            stall_prices: AtomicBool::new(false),
        }
    }
}

impl FakeModel {
    pub fn set_plan(&self, plan: PlanSpec) {
        *self.plan.lock().unwrap() = plan;
    }

    pub fn set_listed_models(&self, listed: &[&str]) {
        *self.listed.lock().unwrap() = Some(listed.iter().map(|m| (*m).to_owned()).collect());
    }

    pub fn set_answer(&self, answer: Answer) {
        *self.answer.lock().unwrap() = answer;
    }

    pub fn set_class(&self, class: Option<EndpointClass>) {
        *self.class.lock().unwrap() = class;
    }

    pub fn sent(&self) -> Vec<Sent> {
        self.sent.lock().unwrap().clone()
    }

    pub fn entered(&self) -> usize {
        self.entered.load(Ordering::SeqCst)
    }

    pub fn refreshes(&self) -> usize {
        self.refreshes.load(Ordering::SeqCst)
    }

    pub fn resolves(&self) -> usize {
        self.resolves.load(Ordering::SeqCst)
    }

    fn build(spec: PlanSpec) -> Option<RoutePlan> {
        let cloud = |ids: &[&str]| CloudArm {
            key: Secret::from("k".to_string()),
            models: ids.iter().map(|s| s.to_string()).collect(),
        };
        let local = || LocalArm {
            base_url: "http://127.0.0.1:11434".into(),
            model: "llama".into(),
            token: None,
        };
        match spec {
            PlanSpec::None => None,
            PlanSpec::Cloud(ids) => Some(RoutePlan::Cloud(cloud(ids))),
            PlanSpec::Local => Some(RoutePlan::LocalOnly(local())),
            PlanSpec::LocalThenCloud(ids) => Some(RoutePlan::LocalThenCloud {
                local: local(),
                cloud: cloud(ids),
            }),
            PlanSpec::Power(ids) => Some(RoutePlan::CloudForPower {
                cloud: cloud(ids),
                displaced_local_model: "llama".into(),
            }),
        }
    }
}

impl ModelEdge for FakeModel {
    fn listed_models(&self) -> crate::error::Result<Vec<String>> {
        if let Some(listed) = self.listed.lock().unwrap().clone() {
            return Ok(listed);
        }
        // Unless a test sets the person's list, it is what the plan carries.
        Ok(match Self::build(*self.plan.lock().unwrap()) {
            Some(
                RoutePlan::Cloud(cloud)
                | RoutePlan::LocalThenCloud { cloud, .. }
                | RoutePlan::CloudForPower { cloud, .. },
            ) => cloud.models,
            _ => Vec::new(),
        })
    }

    fn resolve(&self) -> crate::error::Result<Option<RoutePlan>> {
        self.resolves.fetch_add(1, Ordering::SeqCst);
        let hook = self.on_resolve.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook();
        }
        Ok(Self::build(*self.plan.lock().unwrap()))
    }

    async fn local_class(&self, _base_url: &str) -> Option<EndpointClass> {
        self.class_lookups.fetch_add(1, Ordering::SeqCst);
        if self.stall_class.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        *self.class.lock().unwrap()
    }

    async fn refresh_prices(&self) {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        if self.stall_prices.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
    }

    async fn complete(
        &self,
        plan: &RoutePlan,
        messages: &[ChatMessage],
        max_tokens: u32,
        wire: &WireMark,
    ) -> crate::error::Result<LlmOutcome> {
        // A request goes out unless the script says it was stopped first.
        let answer = self.answer.lock().unwrap().clone();
        if !matches!(answer, Answer::FailBeforeWire | Answer::HangBeforeWire) {
            wire.sending();
        }
        self.sent.lock().unwrap().push(Sent {
            messages: messages
                .iter()
                .map(|m| (m.role.clone(), m.content.clone()))
                .collect(),
            max_tokens,
            models: plan.models().to_vec(),
        });
        self.entered.fetch_add(1, Ordering::SeqCst);
        let hook = self.on_complete.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook();
        }
        if self.gated.load(Ordering::SeqCst) {
            self.gate.acquire().await.unwrap().forget();
        }
        let answer = self.answer.lock().unwrap().clone();
        match answer {
            Answer::Text {
                text,
                model,
                cost,
                tokens,
                truncated,
                local,
            } => Ok(LlmOutcome {
                completion: Completion {
                    text,
                    model: model.map(str::to_string),
                    usage: Usage {
                        prompt_tokens: tokens.map(|t| t.0),
                        completion_tokens: tokens.map(|t| t.1),
                        cost,
                    },
                    truncated,
                },
                meta: CallMeta {
                    provider: if local {
                        Provider::Local
                    } else {
                        Provider::Cloud
                    },
                    latency_ms: 5,
                    fallback: None,
                    displaced_local_model: None,
                },
            }),
            Answer::Fail | Answer::FailBeforeWire => Err(crate::error::Error::Other(
                "PROVIDER-TEXT openrouter said: your prompt was refused".into(),
            )),
            Answer::ConnectFail => {
                wire.never_left();
                Err(crate::error::Error::Other(
                    "PROVIDER-TEXT error sending request".into(),
                ))
            }
            Answer::Hang | Answer::HangBeforeWire => std::future::pending().await,
        }
    }
}
