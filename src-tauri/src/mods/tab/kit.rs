// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Developer kit's readouts (Mods spec §1.15, plan decision 3; slice 6): one read-only command,
//! `dev_kit_info`, that answers everything the Developer tab's rail sections show, so the page
//! draws what PM says and keeps no copy that could drift:
//!
//! - **API version**: `MOD_API_VERSION`, the deprecation window (the oldest major still served) and
//!   which methods are experimental (D13: outside the freeze);
//! - **Endpoints**: every row of the dispatch table (`api::table::ROWS`), calls and events, with the
//!   permission it needs, its class, and when it arrived, was deprecated or removed;
//! - **Limits**: the build-time constants of `api::limits` (rate, concurrency, payload, storage and
//!   event limits);
//! - **Engine**: whether the web engine is sealed, why not, and how many connections reached the
//!   sinkhole;
//! - **Sessions**: each mounted frame's event counters (sent, acknowledged, dropped).
//!
//! **Not here: Test keys.** A test mod's keys are stored "like any mod secret" (§1.15) and typed into
//! PM's own field (`mod_secret_save`, the page's `ModSecretFields`), which reads the test mods from
//! the shared mod rows; none of it passes through this readout.
//!
//! Main window only, and only while developer mode is on for this device (the tab it feeds exists
//! only then). It reads, and changes nothing. [`build`] is pure.

use serde::Serialize;
use tauri::{Manager, Webview};

use super::commands::blocking;
use super::dev_commands::{require_dev_mode, this_device};
use super::error::Result;
use super::listing::class_word;
use crate::mods::api::limits;
use crate::mods::api::table::{self, Kind, Row};
use crate::mods::ids::ModId;
use crate::mods::outbox::Stats;
use crate::mods::policy::{MOD_API_OLDEST_MAJOR, MOD_API_VERSION};
use crate::mods::runtime::ModRuntime;
use crate::mods::EngineState;

/// Everything the Developer kit shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KitInfo {
    pub api: ApiInfo,
    pub endpoints: Vec<Endpoint>,
    pub limits: Limits,
    pub engine: EngineInfo,
    pub sessions: Vec<SessionInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiInfo {
    /// `M.m`, the mod API this PM serves.
    pub version: String,
    /// The oldest major still served: the deprecation window's far end.
    pub oldest_major: u16,
    /// The window, in words.
    pub window: String,
    /// The methods and events outside the API freeze (D13), by name.
    pub experimental: Vec<String>,
}

/// One row of the dispatch table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    pub name: String,
    /// `call` (a frame asks) or `event` (PM tells a frame).
    pub kind: &'static str,
    /// The permission it needs; `None` for what any session has.
    pub permission: Option<String>,
    /// `local`, `read`, `write`, `money` or `egress`.
    pub class: &'static str,
    pub added: String,
    pub deprecated: Option<String>,
    pub removed: Option<String>,
    pub experimental: bool,
}

/// The build-time limits, in the units their names say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    pub calls_per_second: u64,
    pub call_burst: u64,
    pub max_in_flight: usize,
    pub max_args_bytes: usize,
    pub max_result_bytes: usize,
    pub max_binary_result_bytes: usize,
    pub max_docs_per_call: usize,
    pub storage_quota_bytes: u64,
    pub storage_growth_per_second: u64,
    pub storage_growth_burst: u64,
    pub event_window: u64,
    pub max_event_bytes: usize,
    pub max_ids_per_event: usize,
    pub event_coalesce_ms: u64,
    pub max_pending_ids: usize,
    // The experimental calls' limits (PR B); times are in milliseconds.
    pub call_timeout_ms: u64,
    pub ai_max_in_flight: usize,
    pub ai_calls_per_minute: u64,
    pub ai_call_burst: u64,
    pub max_ai_prompt_bytes: usize,
    pub max_ai_messages: usize,
    pub max_ai_output_tokens: u32,
    pub net_max_in_flight: usize,
    pub net_fetches_per_second: u64,
    pub net_fetch_burst: u64,
    pub max_fetch_request_bytes: usize,
    pub max_fetch_response_bytes: usize,
    pub fetch_connect_timeout_ms: u64,
    pub fetch_total_timeout_ms: u64,
    pub max_fetch_redirects: usize,
    pub notify_max_chars: usize,
    pub notify_per_day: u32,
    pub secret_asks_per_session: u32,
}

/// A duration in whole milliseconds, held to what a `u64` says (every limit here is seconds).
fn millis(d: std::time::Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// What the web engine's seal says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineInfo {
    pub sealed: bool,
    /// Why not, one sentence each; empty when sealed.
    pub unsealed_reasons: Vec<String>,
    /// Connections that reached the sinkhole since launch.
    pub sinkhole_attempts: u64,
}

/// One mounted frame's event counters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub mod_id: String,
    pub sent: u64,
    pub acked: u64,
    pub dropped: u64,
    pub closed: bool,
}

/// `M.m` (pure).
fn version_word(v: (u16, u16)) -> String {
    format!("{}.{}", v.0, v.1)
}

/// The deprecation window in words (pure).
pub fn window_words(version: (u16, u16), oldest_major: u16) -> String {
    format!(
        "PM serves mod API {} and, for mods built for an older major, majors {} up to {}. A mod \
         built for an older major in that window runs, marked \"built for an older PM\"; one built \
         only for a major before it is parked until it is updated.",
        version_word(version),
        oldest_major,
        version.0
    )
}

fn endpoint(row: &Row) -> Endpoint {
    Endpoint {
        name: row.name.to_owned(),
        kind: match row.kind {
            Kind::Call => "call",
            Kind::Event => "event",
        },
        permission: row.permission.map(str::to_owned),
        class: class_word(row.class),
        added: version_word(row.added),
        deprecated: row.deprecated.map(version_word),
        removed: row.removed.map(version_word),
        experimental: row.experimental,
    }
}

/// The build-time limits (pure).
pub fn limits_now() -> Limits {
    Limits {
        calls_per_second: limits::CALLS_PER_SECOND,
        call_burst: limits::CALL_BURST,
        max_in_flight: limits::MAX_IN_FLIGHT,
        max_args_bytes: limits::MAX_ARGS_BYTES,
        max_result_bytes: limits::MAX_RESULT_BYTES,
        max_binary_result_bytes: limits::MAX_BINARY_RESULT_BYTES,
        max_docs_per_call: limits::MAX_DOCS_PER_CALL,
        storage_quota_bytes: limits::STORAGE_QUOTA_BYTES,
        storage_growth_per_second: limits::STORAGE_GROWTH_PER_SECOND,
        storage_growth_burst: limits::STORAGE_GROWTH_BURST,
        event_window: limits::EVENT_WINDOW,
        max_event_bytes: limits::MAX_EVENT_BYTES,
        max_ids_per_event: limits::MAX_IDS_PER_EVENT,
        event_coalesce_ms: limits::EVENT_COALESCE_MS,
        max_pending_ids: limits::MAX_PENDING_IDS,
        call_timeout_ms: millis(limits::CALL_TIMEOUT),
        ai_max_in_flight: limits::AI_MAX_IN_FLIGHT,
        ai_calls_per_minute: limits::AI_CALLS_PER_MINUTE,
        ai_call_burst: limits::AI_CALL_BURST,
        max_ai_prompt_bytes: limits::MAX_AI_PROMPT_BYTES,
        max_ai_messages: limits::MAX_AI_MESSAGES,
        max_ai_output_tokens: limits::MAX_AI_OUTPUT_TOKENS,
        net_max_in_flight: limits::NET_MAX_IN_FLIGHT,
        net_fetches_per_second: limits::NET_FETCHES_PER_SECOND,
        net_fetch_burst: limits::NET_FETCH_BURST,
        max_fetch_request_bytes: limits::MAX_FETCH_REQUEST_BYTES,
        max_fetch_response_bytes: limits::MAX_FETCH_RESPONSE_BYTES,
        fetch_connect_timeout_ms: millis(limits::FETCH_CONNECT_TIMEOUT),
        fetch_total_timeout_ms: millis(limits::FETCH_TOTAL_TIMEOUT),
        max_fetch_redirects: limits::MAX_FETCH_REDIRECTS,
        notify_max_chars: limits::NOTIFY_MAX_CHARS,
        notify_per_day: limits::NOTIFY_PER_DAY,
        secret_asks_per_session: limits::SECRET_ASKS_PER_SESSION,
    }
}

/// What the engine said, as the kit takes it.
pub struct EngineFacts {
    pub sealed: bool,
    pub reasons: Vec<String>,
    pub sinkhole_attempts: u64,
}

/// The kit's answer from the dispatch table `rows`, the engine's facts and the sessions' counters
/// (pure). Every row appears, in the table's order: the page lists what the table lists.
pub fn build(rows: &[Row], engine: EngineFacts, sessions: Vec<(ModId, Stats)>) -> KitInfo {
    KitInfo {
        api: ApiInfo {
            version: version_word(MOD_API_VERSION),
            oldest_major: MOD_API_OLDEST_MAJOR,
            window: window_words(MOD_API_VERSION, MOD_API_OLDEST_MAJOR),
            experimental: rows
                .iter()
                .filter(|r| r.experimental)
                .map(|r| r.name.to_owned())
                .collect(),
        },
        endpoints: rows.iter().map(endpoint).collect(),
        limits: limits_now(),
        engine: EngineInfo {
            sealed: engine.sealed,
            unsealed_reasons: engine.reasons,
            sinkhole_attempts: engine.sinkhole_attempts,
        },
        sessions: sessions
            .into_iter()
            .map(|(id, s)| SessionInfo {
                mod_id: id.as_str().to_owned(),
                sent: s.sent,
                acked: s.acked,
                dropped: s.dropped,
                closed: s.closed,
            })
            .collect(),
    }
}

/// The Developer kit's readouts (module doc). Rejects `dev-mode-off` while developer mode is off for
/// this device. Read-only.
#[tauri::command]
pub async fn dev_kit_info(webview: Webview) -> Result<KitInfo> {
    blocking(&webview, move |app| {
        let device = this_device(app)?;
        require_dev_mode(app, &device)?;
        // Unsealed until the engine has said otherwise, including before it is managed.
        let engine = match app.try_state::<EngineState>() {
            Some(state) => EngineFacts {
                sealed: state.engine_sealed(),
                reasons: state.unsealed_reasons(),
                sinkhole_attempts: state.sinkhole_attempts(),
            },
            None => EngineFacts {
                sealed: false,
                reasons: vec!["the web engine has not been set up yet".to_owned()],
                sinkhole_attempts: 0,
            },
        };
        let sessions = app
            .try_state::<ModRuntime>()
            .map(|runtime| runtime.outbox_stats())
            .unwrap_or_default();
        Ok(build(table::ROWS, engine, sessions))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn sealed() -> EngineFacts {
        EngineFacts {
            sealed: true,
            reasons: Vec::new(),
            sinkhole_attempts: 0,
        }
    }

    #[test]
    fn every_row_of_the_dispatch_table_is_in_the_answer_in_the_tables_order() {
        let info = build(table::ROWS, sealed(), Vec::new());
        assert_eq!(info.endpoints.len(), table::ROWS.len());
        for (row, shown) in table::ROWS.iter().zip(&info.endpoints) {
            assert_eq!(shown.name, row.name);
            assert_eq!(shown.permission.as_deref(), row.permission);
            assert_eq!(shown.class, class_word(row.class));
            assert_eq!(shown.experimental, row.experimental);
            assert_eq!(
                shown.kind,
                if row.kind == Kind::Call {
                    "call"
                } else {
                    "event"
                }
            );
        }
        // A call and an event are told apart by kind, not guessed from the name.
        assert!(info
            .endpoints
            .iter()
            .any(|e| e.kind == "call" && e.name == "storage.get"));
        assert!(info
            .endpoints
            .iter()
            .any(|e| e.kind == "event" && e.name == "session.closed" && e.permission.is_none()));
        let names: HashSet<_> = info.endpoints.iter().map(|e| (e.kind, &e.name)).collect();
        assert_eq!(names.len(), info.endpoints.len(), "no row twice");
    }

    #[test]
    fn deprecated_removed_and_experimental_rows_are_shown_as_the_table_has_them() {
        // The live table has none of the first two yet; a row that does is carried through.
        let rows: &'static [Row] = &[
            Row {
                name: "old.call",
                kind: Kind::Call,
                permission: Some("storage"),
                class: crate::mods::api::permissions::Class::Local,
                added: (1, 0),
                deprecated: Some((1, 3)),
                removed: Some((2, 0)),
                experimental: false,
                family: crate::mods::api::limits::Family::General,
            },
            Row {
                name: "new.call",
                kind: Kind::Call,
                permission: Some("ai"),
                class: crate::mods::api::permissions::Class::Money,
                added: (1, 1),
                deprecated: None,
                removed: None,
                experimental: true,
                family: crate::mods::api::limits::Family::Ai,
            },
        ];
        let info = build(rows, sealed(), Vec::new());
        assert_eq!(info.endpoints[0].deprecated.as_deref(), Some("1.3"));
        assert_eq!(info.endpoints[0].removed.as_deref(), Some("2.0"));
        assert_eq!(info.endpoints[1].added, "1.1");
        assert_eq!(info.endpoints[1].class, "money");
        assert_eq!(info.api.experimental, ["new.call"]);
    }

    #[test]
    fn the_version_the_window_and_the_limits_are_the_build_constants() {
        let info = build(table::ROWS, sealed(), Vec::new());
        assert_eq!(info.api.version, "1.0");
        assert_eq!(info.api.oldest_major, MOD_API_OLDEST_MAJOR);
        assert!(info.api.window.contains("1.0") && info.api.window.contains("older"));
        // The live table's experimental calls, in its order: PR B's four.
        assert_eq!(
            info.api.experimental,
            ["ai.complete", "net.fetch", "secrets.set", "notify"]
        );
        let l = &info.limits;
        assert_eq!(l.calls_per_second, limits::CALLS_PER_SECOND);
        assert_eq!(l.max_in_flight, limits::MAX_IN_FLIGHT);
        assert_eq!(l.max_args_bytes, limits::MAX_ARGS_BYTES);
        assert_eq!(l.storage_quota_bytes, limits::STORAGE_QUOTA_BYTES);
        assert_eq!(l.event_window, limits::EVENT_WINDOW);
        // The experimental calls' limits, times in whole milliseconds.
        assert_eq!(l.call_timeout_ms, 180_000);
        assert_eq!(l.fetch_connect_timeout_ms, 10_000);
        assert_eq!(l.fetch_total_timeout_ms, 30_000);
        assert_eq!(l.ai_max_in_flight, limits::AI_MAX_IN_FLIGHT);
        assert_eq!(l.net_max_in_flight, limits::NET_MAX_IN_FLIGHT);
        assert_eq!(l.max_fetch_response_bytes, limits::MAX_FETCH_RESPONSE_BYTES);
        assert_eq!(l.max_ai_output_tokens, limits::MAX_AI_OUTPUT_TOKENS);
        assert_eq!(l.notify_per_day, limits::NOTIFY_PER_DAY);
        // The JSON the page reads is camelCase and carries every limit.
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["limits"]["callTimeoutMs"], 180_000);
        assert_eq!(
            json["limits"]["maxFetchRedirects"],
            limits::MAX_FETCH_REDIRECTS
        );
        assert_eq!(json["limits"]["callsPerSecond"], limits::CALLS_PER_SECOND);
        assert_eq!(json["api"]["oldestMajor"], MOD_API_OLDEST_MAJOR);
        assert_eq!(json["engine"]["sinkholeAttempts"], 0);
    }

    #[test]
    fn an_unsealed_engine_says_why_and_the_sinkhole_count_is_carried() {
        let info = build(
            table::ROWS,
            EngineFacts {
                sealed: false,
                reasons: vec!["the sinkhole did not bind".into()],
                sinkhole_attempts: 3,
            },
            Vec::new(),
        );
        assert!(!info.engine.sealed);
        assert_eq!(info.engine.unsealed_reasons, ["the sinkhole did not bind"]);
        assert_eq!(info.engine.sinkhole_attempts, 3);
    }

    #[test]
    fn each_mounted_frames_counters_are_listed_by_mod() {
        let id = |s: &str| ModId::parse(s).unwrap();
        let info = build(
            table::ROWS,
            sealed(),
            vec![
                (
                    id("acme.one"),
                    Stats {
                        sent: 5,
                        acked: 4,
                        dropped: 1,
                        closed: false,
                    },
                ),
                (id("acme.two"), Stats::default()),
            ],
        );
        assert_eq!(info.sessions.len(), 2);
        assert_eq!(
            (
                info.sessions[0].mod_id.as_str(),
                info.sessions[0].sent,
                info.sessions[0].acked,
                info.sessions[0].dropped
            ),
            ("acme.one", 5, 4, 1)
        );
        assert!(!info.sessions[1].closed);
    }

    #[test]
    fn the_command_is_main_window_only_dev_mode_gated_read_only_and_registered() {
        let source = include_str!("kit.rs");
        let production = source.split("#[cfg(test)]\nmod tests").next().unwrap();
        let body = production.split("#[tauri::command]").nth(1).unwrap();
        assert!(body.contains("pub async fn dev_kit_info(webview: Webview)"));
        assert!(
            body.contains("blocking(&webview"),
            "the window and store gate"
        );
        assert!(
            body.find("require_dev_mode").unwrap() < body.find("EngineState").unwrap(),
            "developer mode is read before anything is answered"
        );
        assert!(!body.contains("devmode::write") && !body.contains("with_ledger"));
        assert!(!"dev_kit_info".starts_with("set_"));
        let lib = include_str!("../../lib.rs");
        assert!(lib.contains("mods::tab::kit::dev_kit_info,"));
    }
}
