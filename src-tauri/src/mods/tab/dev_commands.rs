// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Developer mode's commands and the unsigned file install's first step (Mods spec §1.15; slice 6,
//! part 3): the thin edge over `mods::devmode` (the per-device record), `ledger::dev_switch` (the
//! one function that switches it and carries the mods along) and `tab::unsigned` (the stark
//! install's two steps; the second is `install_mod_consented`, which hands an unsigned token to
//! [`finish_unsigned`]).
//!
//! - `dev_mode_state`: whether developer mode is on for THIS device in this vault, read by Rust
//!   from the store's record. The page's only authority for it. Any window may ask (the pop-out
//!   Calendar reads the same switch), once the store is open.
//! - `dev_mode_switch(on)`: main window only, like every mods command. Awaits the write and answers
//!   with what the switch did (which unsigned mods it paused or resumed); on a failure nothing
//!   changed and the page says so. It announces `mods://changed` (cause `dev-mode`) to the main
//!   window and `mods://dev-mode` to every window.
//! - `install_unsigned_file(path)`: the unsigned file's consent screen, nothing installed. Refused
//!   first, before the file is read, while developer mode is off for this device.

use std::path::Path;

use chrono::Utc;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Webview};

use super::commands::{blocking, with_ledger, with_token, View};
use super::consent::Answers;
use super::device;
use super::error::{Code, ModsError, Result};
use super::install::{self, ConsentScreen, Finished};
use super::stage::Staged;
use super::unsigned::{self, UnsignedEnv};
use crate::mods::api::Host;
use crate::mods::devmode;
use crate::mods::folder_watch;
use crate::mods::ledger::{self, DevSwitch, Mint};
use crate::mods::refresh;
use crate::AppState;

/// The event every window hears when developer mode is switched: `{on}`.
pub const DEV_MODE_EVENT: &str = "mods://dev-mode";

/// What `mods://dev-mode` carries.
#[derive(Debug, Clone, Serialize)]
pub struct DevModeChanged {
    pub on: bool,
}

/// Developer mode for this device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevModeState {
    pub on: bool,
    /// Why it reads as off when this device couldn't be named (the data folder), in words.
    pub problem: Option<String>,
}

pub(super) fn this_device(app: &AppHandle) -> Result<String> {
    let data_dir = crate::paths::data_dir(app).map_err(ModsError::internal)?;
    device::device_id(&data_dir).map_err(ModsError::internal)
}

/// The refusal an unsigned file gets while developer mode is off (pure).
pub fn dev_mode_off() -> ModsError {
    ModsError::new(
        Code::DevModeOff,
        "Developer mode is off on this computer. Unsigned mods install only while it is on \
         (Settings, Developer).",
    )
}

/// Developer mode on for `device` in the open store, or the typed refusal. Reads the store's record.
pub(super) fn require_dev_mode(app: &AppHandle, device: &str) -> Result<()> {
    let state = app.state::<AppState>();
    let conn = state.conn().map_err(|_| ModsError::no_store())?;
    if devmode::is_on(&conn, device).map_err(ModsError::internal)? {
        Ok(())
    } else {
        Err(dev_mode_off())
    }
}

/// Whether developer mode is on for this device in this vault. The page's authority for the
/// switch: its own copy is only a cache that avoids a flash on first paint.
#[tauri::command]
pub async fn dev_mode_state(webview: Webview) -> Result<DevModeState> {
    // Any window, once the store is open: a read-only yes or no, and the pop-out Calendar shows the
    // same developer surfaces.
    let app = webview.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        if state.active_vault_id().is_none() {
            return Err(ModsError::no_store());
        }
        let device = match this_device(&app) {
            Ok(d) => d,
            Err(_) => {
                return Ok(DevModeState {
                    on: false,
                    problem: Some("PM couldn't read this device's record.".to_owned()),
                })
            }
        };
        let conn = state.conn().map_err(|_| ModsError::no_store())?;
        let on = devmode::is_on(&conn, &device).map_err(ModsError::internal)?;
        Ok(DevModeState { on, problem: None })
    })
    .await
    .map_err(ModsError::internal)?
}

/// Switch developer mode for this device in this vault. Off pauses every unsigned mod (its
/// sessions end), on resumes exactly those it paused. The answer is what it is now: the page shows
/// that, not what it asked for.
#[tauri::command]
pub async fn dev_mode_switch(webview: Webview, on: bool) -> Result<DevSwitch> {
    blocking(&webview, move |app| {
        let view = View::read(app)?;
        let done = ledger::set_dev_mode_for_this_device(app, on, view.index())?;
        // Off unloaded every test folder: nothing is left to watch.
        folder_watch::resync(app);
        refresh::emit_changed(app, refresh::CHANGED_DEV_MODE);
        if let Err(e) = app.emit(DEV_MODE_EVENT, DevModeChanged { on: done.on }) {
            eprintln!("mods: a developer-mode notice wasn't delivered ({e})");
        }
        Ok(done)
    })
    .await
}

/// Read and verify the unsigned `.pmmod` at `path` (size-capped), run every check an install would,
/// judge D3 across the unsigned mods, and return the stark screen, *without installing*. Hand the
/// screen's `token`, the user's answers and the id they typed to `install_mod_consented`. Refused
/// at once while developer mode is off for this device.
#[tauri::command]
pub async fn install_unsigned_file(webview: Webview, path: String) -> Result<ConsentScreen> {
    blocking(&webview, move |app| {
        let device = this_device(app)?;
        // Before the file is read: a cheap refusal, and a hostile zip isn't parsed while it is off.
        // The ledger reads it again, as the authority.
        require_dev_mode(app, &device)?;
        let bytes = install::read_file(Path::new(&path))?;
        let view = View::read(app)?;
        view.for_install()?;
        let env = UnsignedEnv {
            now: Utc::now(),
            index: view.index(),
            engine_sealed: Host::engine_sealed(app),
            device: &device,
        };
        let (screen, staged) = with_ledger(app, Mint::Always, |ledger| {
            unsigned::prepare(ledger, bytes, &env)
        })?;
        with_token(app, screen, staged)
    })
    .await
}

/// The second step for an unsigned screen's token (called by `install_mod_consented`, on a blocking
/// thread): the typed id, the plan, the install and the grants.
pub(super) fn finish_unsigned(
    app: &AppHandle,
    view: &View,
    staged: Staged,
    decisions: &Answers,
    confirm_id: Option<&str>,
) -> Result<Finished> {
    let device = this_device(app)?;
    let env = UnsignedEnv {
        now: Utc::now(),
        index: view.index(),
        engine_sealed: Host::engine_sealed(app),
        device: &device,
    };
    let finished = with_ledger(app, Mint::Always, |ledger| {
        unsigned::finish(ledger, staged, decisions, confirm_id, &env)
    })?;
    // `install_mod_consented` announces `told` and the change.
    Ok(finished)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The commands' source, split at each `#[tauri::command]`: the function's name and its body.
    fn command_bodies() -> Vec<(String, String)> {
        let source = include_str!("dev_commands.rs");
        let production = source.split("#[cfg(test)]\nmod tests").next().unwrap();
        production
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
    fn the_commands_that_change_anything_go_through_the_main_window_gate() {
        let commands = command_bodies();
        assert_eq!(commands.len(), 3);
        for (name, body) in &commands {
            assert!(
                !name.starts_with("set_"),
                "{name}: the Settings tick keys on `set_`"
            );
            assert!(
                body.contains("webview: Webview"),
                "{name} must see its window"
            );
            if name == "dev_mode_state" {
                // The one read any window may make: it still refuses without an open store.
                assert!(body.contains("active_vault_id().is_none()"));
                assert!(!body.contains("devmode::write"));
            } else {
                assert!(
                    body.contains("blocking(&webview"),
                    "{name} doesn't go through the window and store gate"
                );
            }
        }
    }

    #[test]
    fn every_command_here_is_registered() {
        let lib = include_str!("../../lib.rs");
        for (name, _) in command_bodies() {
            assert!(
                lib.contains(&format!("mods::tab::dev_commands::{name},")),
                "{name} isn't in generate_handler!"
            );
        }
        assert_eq!(
            lib.matches("mods::tab::dev_commands::").count(),
            command_bodies().len()
        );
    }

    #[test]
    fn an_unsigned_file_while_developer_mode_is_off_is_the_typed_dev_mode_refusal() {
        let e = dev_mode_off();
        assert_eq!(e.code, Code::DevModeOff);
        assert!(e.message.contains("Developer mode is off"));
    }

    #[test]
    fn the_unsigned_install_checks_developer_mode_before_it_reads_the_file() {
        let (_, body) = command_bodies()
            .into_iter()
            .find(|(n, _)| n == "install_unsigned_file")
            .unwrap();
        let dev = body.find("require_dev_mode").unwrap();
        let read = body.find("read_file").unwrap();
        assert!(dev < read);
    }
}
