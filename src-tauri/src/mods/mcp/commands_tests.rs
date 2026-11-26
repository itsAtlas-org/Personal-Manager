// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Settings edge: what is offered where, the state the page is told (never a token), connecting
//! (the scopes it refuses, the token and the snippet that carry it once), the switch and revoking.

use std::ffi::OsString;
use std::path::PathBuf;

use serde_json::Value;

use super::commands::*;
use super::test_support::open_vault;
use super::{clients, device};
use crate::mods::api::test_support::TestHost;
use crate::mods::devmode::Here;
use crate::mods::tab::error::Code;

fn rig() -> (TestHost, Here) {
    let host = TestHost::new();
    let here = open_vault(&host);
    (host, here)
}

fn scopes(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| (*s).to_owned()).collect()
}

const CMD: &str = "/opt/PM/Personal-Manager.AppImage";

fn rows(host: &TestHost) -> i64 {
    host.sql(|c| {
        c.query_row("SELECT count(*) FROM mcp_clients", [], |r| r.get(0))
            .unwrap()
    })
}

/// The state as the page receives it, as JSON text.
fn wire(host: &TestHost) -> String {
    serde_json::to_string(&read_state(host).unwrap()).unwrap()
}

// --- Where it is offered ------------------------------------------------------------------------

#[test]
fn a_debug_build_offers_it_and_a_release_build_only_on_a_listed_os() {
    assert!(offered_on("linux", true, &[]));
    assert!(offered_on("windows", true, &[]));
    // Release, nothing listed: hidden everywhere.
    for os in ["linux", "macos", "windows"] {
        assert!(!offered_on(os, false, &[]), "{os}");
    }
    // Release, Linux listed: Linux only.
    assert!(offered_on("linux", false, &["linux"]));
    assert!(!offered_on("macos", false, &["linux"]));
    assert!(!offered_on("windows", false, &["linux"]));
}

// --- The state ----------------------------------------------------------------------------------

#[test]
fn the_state_of_a_fresh_vault_is_off_with_no_apps() {
    let (host, _here) = rig();
    let state = read_state(&host).unwrap();
    assert!(state.offered);
    assert!(!state.on);
    assert!(state.clients.is_empty());
    assert!(!state.listening);
    assert_eq!(state.error, None);
}

#[test]
fn the_state_never_contains_a_token_or_its_hash() {
    let (host, here) = rig();
    let made = connect(&host, CMD, "Desk", &scopes(&["tasks.read"])).unwrap();
    let hash: String = host.sql(|c| {
        c.query_row("SELECT token_hash FROM mcp_clients", [], |r| r.get(0))
            .unwrap()
    });
    let body = made.token.strip_prefix("pmcp_").unwrap();
    for text in [wire(&host), format!("{:?}", read_state(&host).unwrap())] {
        assert!(!text.contains(&made.token));
        assert!(!text.contains(body), "the token's secret half");
        assert!(!text.contains(&hash), "the stored hash");
        assert!(!text.contains("pmcp_"));
        assert!(!text.contains("token"));
        assert!(!text.contains("scopes_hash"));
    }
    drop(here);
}

#[test]
fn the_state_lists_an_app_with_its_scopes_and_whether_this_device_holds_its_pairing() {
    let (host, _here) = rig();
    let made = connect(
        &host,
        CMD,
        "Desk",
        &scopes(&["tasks.read", "calendar.read"]),
    )
    .unwrap();
    // A row another computer made in the shared vault: no pairing here.
    host.sql(|c| {
        c.execute(
            "INSERT INTO mcp_clients(name, token_hash, scopes) VALUES ('Elsewhere', 'x', '[\"tasks.read\"]')",
            [],
        )
        .unwrap()
    });
    let state = read_state(&host).unwrap();
    assert_eq!(state.clients.len(), 2);
    let mine = &state.clients[0];
    assert_eq!(mine.id, made.client_id);
    assert_eq!(mine.name, "Desk");
    assert_eq!(mine.scopes, ["calendar.read", "tasks.read"]);
    assert!(mine.paired);
    assert_eq!(state.clients[1].name, "Elsewhere");
    assert!(!state.clients[1].paired, "needs re-pairing on this device");
}

#[test]
fn no_open_vault_is_the_no_store_refusal() {
    let host = TestHost::new();
    assert_eq!(read_state(&host).unwrap_err().code, Code::NoStore);
    assert_eq!(switch(&host, true).unwrap_err().code, Code::NoStore);
    assert_eq!(
        connect(&host, CMD, "Desk", &scopes(&["tasks.read"]))
            .unwrap_err()
            .code,
        Code::NoStore
    );
    assert_eq!(revoke(&host, 1).unwrap_err().code, Code::NoStore);
}

// --- The switch ---------------------------------------------------------------------------------

#[test]
fn the_switch_is_this_devices_file_and_off_keeps_the_apps() {
    let (host, here) = rig();
    connect(&host, CMD, "Desk", &scopes(&["tasks.read"])).unwrap();
    assert!(!read_state(&host).unwrap().on);
    switch(&host, true).unwrap();
    assert!(read_state(&host).unwrap().on);
    assert!(device::device_state(&here).on);
    switch(&host, false).unwrap();
    let state = read_state(&host).unwrap();
    assert!(!state.on);
    assert_eq!(state.clients.len(), 1, "off keeps the pairing");
    assert!(state.clients[0].paired);
    // The store is not where "on" lives: no settings key was written for it.
    let keys: i64 = host.sql(|c| {
        c.query_row(
            "SELECT count(*) FROM settings WHERE key LIKE '%mcp%'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    });
    assert_eq!(keys, 0);
}

// --- A build that does not offer it --------------------------------------------------------------
// The tests run under debug assertions, where `offered()` is always true, so a release build on
// Windows is stood in for by passing `offered = false` to what the commands pass `offered()` to.

#[test]
fn a_build_that_does_not_offer_it_answers_a_hidden_state_even_with_the_switch_on() {
    let (host, here) = rig();
    connect(&host, CMD, "Desk", &scopes(&["tasks.read"])).unwrap();
    switch(&host, true).unwrap();
    // A debug build on the same data folder wrote this: the file says on, an app is paired.
    assert!(device::device_state(&here).on);
    let state = read_state_if(&host, false).unwrap();
    assert!(!state.offered);
    assert!(!state.on);
    assert!(state.clients.is_empty());
    assert!(!state.listening);
    // No vault needed to say nothing.
    assert!(!read_state_if(&TestHost::new(), false).unwrap().offered);
}

#[test]
fn a_build_that_does_not_offer_it_refuses_on_and_connecting_but_always_allows_off_and_revoke() {
    let (host, here) = rig();
    let made = connect(&host, CMD, "Desk", &scopes(&["tasks.read"])).unwrap();
    // On is refused and writes nothing.
    let e = switch_checked(&host, true, false).unwrap_err();
    assert_eq!(e.code, Code::NotAllowed);
    assert!(!device::device_state(&here).on);
    // Connecting is refused whole: no row, no pairing, and the launcher is never asked.
    let e = connect_checked(&host, "Laptop", &scopes(&["tasks.read"]), false, || {
        panic!("the launcher is not asked where nothing is offered")
    })
    .unwrap_err();
    assert_eq!(e.code, Code::NotAllowed);
    assert_eq!(rows(&host), 1);
    assert_eq!(device::device_state(&here).clients.len(), 1);
    // Off is allowed: a switch that cannot be turned off is the worst failure.
    switch_checked(&host, true, true).unwrap();
    assert!(device::device_state(&here).on);
    switch_checked(&host, false, false).unwrap();
    assert!(!device::device_state(&here).on);
    // Revoking never asks whether it is offered.
    revoke(&host, made.client_id).unwrap();
    assert_eq!(rows(&host), 0);
}

#[test]
fn an_offering_build_connects_through_the_checked_path_and_a_missing_launcher_makes_nothing() {
    let (host, _here) = rig();
    let ok = connect_checked(&host, "Desk", &scopes(&["tasks.read"]), true, || {
        Some(CMD.to_owned())
    })
    .unwrap();
    assert!(ok.snippet.contains(CMD));
    let e = connect_checked(&host, "Laptop", &scopes(&["tasks.read"]), true, || None).unwrap_err();
    assert_eq!(e.code, Code::NotAllowed);
    assert_eq!(rows(&host), 1, "a refusal leaves no app nothing can start");
}

#[test]
fn every_command_passes_the_build_check_to_the_function_that_gates_on_it() {
    let commands = command_bodies();
    let body = |name: &str| {
        commands
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, b)| b.as_str())
            .unwrap_or_else(|| panic!("{name}"))
    };
    assert!(body("mcp_state").contains("read_state)"));
    assert!(include_str!("commands.rs").contains("    read_state_if(host, offered())\n"));
    assert!(body("mcp_set_on").contains("switch_checked(app, on, offered())"));
    assert!(body("mcp_connect_app").contains("offered(), this_launch_command"));
}

// --- Connecting ---------------------------------------------------------------------------------

#[test]
fn connecting_pairs_the_app_and_answers_with_the_token_inside_the_config() {
    let (host, here) = rig();
    let made = connect(
        &host,
        CMD,
        "Desk",
        &scopes(&["tasks.read", "projects.read", "calendar.read"]),
    )
    .unwrap();
    assert!(made.token.starts_with(&format!("pmcp_{}_", made.client_id)));
    assert_eq!(rows(&host), 1);
    assert!(device::device_state(&here)
        .clients
        .iter()
        .any(|c| c.id == made.client_id));

    // The snippet is the config an MCP client pastes: PM's own binary, `--mcp` alone in its
    // arguments, the token in the client's environment for the shim.
    let config: Value = serde_json::from_str(&made.snippet).unwrap();
    let server = &config["mcpServers"]["personal-manager"];
    assert_eq!(server["command"], CMD);
    assert_eq!(server["args"], serde_json::json!(["--mcp"]));
    assert_eq!(server["env"]["PM_MCP_TOKEN"], made.token.as_str());
    assert_eq!(server["env"].as_object().unwrap().len(), 1);
    assert_eq!(
        made.snippet.matches(&made.token).count(),
        1,
        "the token appears once, in the env"
    );
}

#[test]
fn connecting_refuses_calendar_details_unknown_scopes_and_no_scopes_and_makes_nothing() {
    let (host, here) = rig();
    for bad in [
        scopes(&["calendar.details"]),
        scopes(&["tasks.read", "calendar.details"]),
        scopes(&["tasks.write"]),
        scopes(&["ai"]),
        scopes(&[""]),
        scopes(&[]),
    ] {
        let e = connect(&host, CMD, "Desk", &bad).unwrap_err();
        assert_eq!(e.code, Code::BadRequest, "{bad:?}");
    }
    assert_eq!(rows(&host), 0);
    assert!(device::device_state(&here).clients.is_empty());
}

#[test]
fn connecting_refuses_a_blank_or_overlong_name() {
    let (host, _here) = rig();
    for bad in ["", "   ", &"x".repeat(61)] {
        let e = connect(&host, CMD, bad, &scopes(&["tasks.read"])).unwrap_err();
        assert_eq!(e.code, Code::BadRequest, "{bad:?}");
    }
    assert_eq!(rows(&host), 0);
    connect(&host, CMD, &"x".repeat(60), &scopes(&["tasks.read"])).unwrap();
}

#[test]
fn a_refusal_never_carries_a_token_or_a_path() {
    let (host, _here) = rig();
    let first = connect(&host, CMD, "Desk", &scopes(&["tasks.read"])).unwrap();
    let e = connect(&host, CMD, "Desk", &scopes(&["calendar.details"])).unwrap_err();
    let said = format!("{} {:?}", e.message, e);
    assert!(!said.contains("pmcp_"));
    assert!(!said.contains(&first.token));
    assert!(!said.contains(CMD));
}

#[test]
fn a_connected_apps_debug_text_leaves_the_token_out() {
    let (host, _here) = rig();
    let made = connect(&host, CMD, "Desk", &scopes(&["tasks.read"])).unwrap();
    let text = format!("{made:?}");
    assert!(!text.contains("pmcp_"));
    assert!(!text.contains(made.token.strip_prefix("pmcp_").unwrap()));
}

// --- Revoking -----------------------------------------------------------------------------------

#[test]
fn revoking_deletes_the_row_and_the_pairing_and_a_second_time_is_no_error() {
    let (host, here) = rig();
    let a = connect(&host, CMD, "Desk", &scopes(&["tasks.read"])).unwrap();
    let b = connect(&host, CMD, "Laptop", &scopes(&["tasks.read"])).unwrap();
    revoke(&host, a.client_id).unwrap();
    assert_eq!(rows(&host), 1);
    assert!(device::device_state(&here)
        .clients
        .iter()
        .all(|c| c.id != a.client_id));
    revoke(&host, a.client_id).unwrap();
    let state = read_state(&host).unwrap();
    assert_eq!(state.clients.len(), 1);
    assert_eq!(state.clients[0].id, b.client_id);
}

#[test]
fn revoking_a_row_this_device_never_paired_removes_it_from_the_store() {
    let (host, _here) = rig();
    host.sql(|c| {
        c.execute(
            "INSERT INTO mcp_clients(name, token_hash, scopes) VALUES ('Elsewhere', 'x', '[\"tasks.read\"]')",
            [],
        )
        .unwrap()
    });
    let id = read_state(&host).unwrap().clients[0].id;
    revoke(&host, id).unwrap();
    assert_eq!(rows(&host), 0);
}

#[test]
fn a_revoked_apps_token_no_longer_verifies() {
    let (host, here) = rig();
    let made = connect(&host, CMD, "Desk", &scopes(&["tasks.read"])).unwrap();
    switch(&host, true).unwrap();
    assert!(host
        .sql(|c| clients::verify(c, &here, &made.token))
        .is_some());
    revoke(&host, made.client_id).unwrap();
    assert!(host
        .sql(|c| clients::verify(c, &here, &made.token))
        .is_none());
}

// --- How an app starts PM -----------------------------------------------------------------------

fn os(s: &str) -> Option<OsString> {
    Some(OsString::from(s))
}

fn path(s: &str) -> Option<PathBuf> {
    Some(PathBuf::from(s))
}

#[test]
fn an_appimage_is_started_by_its_file_never_by_its_mount() {
    // Inside an AppImage the running binary lives in a temporary mount that is gone when PM quits.
    let mounted = path("/tmp/.mount_PersonXYZ/usr/bin/pm");
    assert_eq!(
        launch_command(os("/home/me/Apps/PM.AppImage"), mounted.clone()).as_deref(),
        Some("/home/me/Apps/PM.AppImage")
    );
    // No AppImage variable and only the mount to go on: nothing, rather than a path that dies.
    assert_eq!(launch_command(None, mounted.clone()), None);
    // A mount in the variable itself is not used either.
    assert_eq!(
        launch_command(os("/tmp/.mount_PersonXYZ/AppRun"), path("/usr/bin/pm")).as_deref(),
        Some("/usr/bin/pm")
    );
}

#[test]
fn an_installed_pm_is_started_by_its_binary() {
    assert_eq!(
        launch_command(None, path("/usr/bin/personal-manager")).as_deref(),
        Some("/usr/bin/personal-manager")
    );
    // An empty variable is no variable.
    assert_eq!(
        launch_command(os(""), path("/usr/bin/personal-manager")).as_deref(),
        Some("/usr/bin/personal-manager")
    );
}

#[test]
fn a_relative_or_missing_path_is_never_handed_to_an_app() {
    assert_eq!(launch_command(os("PM.AppImage"), None), None);
    assert_eq!(launch_command(None, path("pm")), None);
    assert_eq!(launch_command(None, None), None);
}

// --- The commands' source ------------------------------------------------------------------------

/// The commands' source, split at each `#[tauri::command]`: the function's name and its text.
fn command_bodies() -> Vec<(String, String)> {
    let source = include_str!("commands.rs");
    source
        .split("#[tauri::command]")
        .skip(1)
        .map(|chunk| {
            let name = chunk
                .split("fn ")
                .nth(1)
                .and_then(|rest| rest.split('(').next())
                .unwrap()
                .trim()
                .to_owned();
            (name, chunk.to_owned())
        })
        .collect()
}

#[test]
fn every_command_that_touches_pms_data_goes_through_the_main_window_gate() {
    let commands = command_bodies();
    assert_eq!(commands.len(), 5);
    for (name, body) in &commands {
        assert!(
            !name.starts_with("set_"),
            "{name}: the Settings tick keys on `set_`"
        );
        if name == "mcp_offered" {
            // A build-time yes or no: it reads no store and no setting.
            assert!(!body.contains("webview"));
            assert!(!body.contains("conn"));
            continue;
        }
        assert!(
            body.contains("webview: Webview") && body.contains("blocking(&webview"),
            "{name} doesn't go through the window and store gate"
        );
    }
}

#[test]
fn every_command_here_is_registered() {
    let lib = include_str!("../../lib.rs");
    for (name, _) in command_bodies() {
        assert!(
            lib.contains(&format!("mods::mcp::commands::{name},")),
            "{name} isn't in generate_handler!"
        );
    }
    assert_eq!(
        lib.matches("mods::mcp::commands::").count(),
        command_bodies().len()
    );
}

#[test]
fn the_token_is_only_ever_handed_back_by_connecting() {
    let source = include_str!("commands.rs");
    let production = source.split("#[cfg(test)]").next().unwrap();
    // Nothing here prints; the log lines are in `clients` and `device`, which never see a token text.
    assert!(!production.contains("println!"));
    // `McpState` has no field for it.
    let state = production
        .split("pub struct McpState")
        .nth(1)
        .unwrap()
        .split("\n}\n")
        .next()
        .unwrap();
    assert!(!state.to_lowercase().contains("token"));
    assert!(!state.contains("snippet"));
}
