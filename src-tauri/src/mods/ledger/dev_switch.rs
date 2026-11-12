// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Developer mode's switch, carried out against the mods (Mods spec §1.15; the record and its rules
//! are `mods::devmode`). [`set_dev_mode_for_this_device`] is the one function that switches it for
//! this device in this vault: it writes the record, then reloads the registry, and the load
//! (`policy::dev_mode_state`) does the rest:
//! - **off** pauses every active unsigned mod with the reason `dev-mode` and ends their sessions
//!   (a frame is told `mod.paused`, then `session.closed`);
//! - **on** resumes exactly the rows that reason paused, each one only if the cached safety list
//!   doesn't name it (one it names is handed to the safety list's own pause instead), its files
//!   check out and its row is otherwise as it was. A mod the user paused, the safety list paused,
//!   or that is parked for another reason stays as it is.
//!
//! The record is written first and the registry follows under the same ledger lock, so a write that
//! fails changes nothing (the page reverts its toggle and says so), and one that succeeds is
//! followed at once by the pause. The registry's own copy of the switch
//! ([`ModRuntime::set_unsigned_allowed`](crate::mods::runtime::ModRuntime)) is set between the two,
//! so every load, whoever runs it, follows the record: there is no path that reloads the registry
//! and serves an unsigned mod while developer mode is off.
//!
//! Switching it off for **every** device (the restore reset) is a plain write,
//! [`devmode::all_devices_off`], on the connection the restore holds: the load that follows the
//! restored store's open (`after_writer_open`) does the pausing, since the open reads the record
//! first.

use chrono::{DateTime, Utc};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use super::{read_rows, Ledger, LedgerError, LedgerStore, Mint, ServeCheck};
use crate::mods::devmode;
use crate::mods::runtime::ModRuntime;
use crate::mods::safety::SafetyCheck;
use crate::mods::tab::device;
use crate::AppState;

/// What switching developer mode did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevSwitch {
    /// Developer mode, for this device, after the switch.
    pub on: bool,
    /// Unsigned mods the switch paused (off).
    pub paused: Vec<String>,
    /// Unsigned mods the switch resumed (on).
    pub resumed: Vec<String>,
}

impl<S: LedgerStore> Ledger<'_, S> {
    /// Switch developer mode on or off for `device` in this vault and bring the mods along (module
    /// doc). `check` is the cached safety list's serve check. Blocking: a row write and a registry
    /// load, under the ledger lock.
    pub fn set_dev_mode_for_device(
        &self,
        device: &str,
        on: bool,
        check: &dyn ServeCheck,
        now: DateTime<Utc>,
    ) -> Result<DevSwitch, LedgerError> {
        let _ledger = self.runtime.ledger_lock();
        // The record first: a write that fails changes nothing.
        let now_on = self.with_conn(|c| Ok(devmode::write(c, device, on)?))?;
        self.runtime.set_unsigned_allowed(now_on);
        let rows = self.with_conn(|c| Ok(read_rows(c)?))?;
        let report = self.load(&rows, now, check);
        Ok(DevSwitch {
            on: now_on,
            paused: report.dev_paused,
            resumed: report.dev_resumed,
        })
    }
}

/// Whether developer mode is on for this device in the open store: the device's id (a file read,
/// done before the store is touched) against the store's record. Any failure is off, logged.
pub fn on_here(app: &AppHandle, state: &AppState) -> bool {
    let device = match crate::paths::data_dir(app)
        .map_err(|e| e.to_string())
        .and_then(|d| device::device_id(&d).map_err(|e| e.to_string()))
    {
        Ok(device) => device,
        Err(e) => {
            eprintln!("mods: this device couldn't be named, so developer mode is off ({e})");
            return false;
        }
    };
    let conn = match state.conn() {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("mods: developer mode couldn't be read, so it is off ({e})");
            return false;
        }
    };
    devmode::is_on(&conn, &device).unwrap_or_else(|e| {
        eprintln!("mods: developer mode couldn't be read, so it is off ({e})");
        false
    })
}

/// Switch developer mode for THIS device in the open vault: the one function the `dev_mode_switch`
/// command calls (module doc). A store with no mods folder (no mod was ever installed) has nothing
/// to pause or resume, so it only writes the record and sets the registry's copy. Blocking.
pub fn set_dev_mode_for_this_device(
    app: &AppHandle,
    on: bool,
    cached: Option<&crate::mods::index::Index>,
) -> Result<DevSwitch, LedgerError> {
    let data_dir = crate::paths::data_dir(app).map_err(|e| LedgerError::Store(e.to_string()))?;
    let device = device::device_id(&data_dir).map_err(LedgerError::Disk)?;
    let check = SafetyCheck::serve(cached);
    let switched = super::with_active_ledger(app, Mint::IfRows, |ledger| {
        ledger.set_dev_mode_for_device(&device, on, &check, Utc::now())
    });
    if let Some(done) = switched {
        return done;
    }
    // No mods folder: the record, and the registry's copy of it.
    let state = app.state::<AppState>();
    let runtime = app
        .try_state::<ModRuntime>()
        .ok_or_else(|| LedgerError::Store("mods aren't running".into()))?;
    let _ledger = runtime.ledger_lock();
    let mut conn = state
        .conn()
        .map_err(|e| LedgerError::Store(e.to_string()))?;
    let now_on = devmode::write(&mut conn, &device, on)?;
    runtime.set_unsigned_allowed(now_on);
    Ok(DevSwitch {
        on: now_on,
        paused: Vec::new(),
        resumed: Vec::new(),
    })
}
