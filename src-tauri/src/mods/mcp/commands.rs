// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Settings screen's edge over the connected apps (Mods spec 1.16): is the section offered on this
//! computer, the state it draws, the switch, connecting an app and revoking one. Thin: the rules are
//! in [`clients`], [`device`] and [`supervisor`](super::supervisor); what is added here is the
//! window and store gate every Mods command passes, the sentences a person reads, and the config
//! snippet an app is given.
//!
//! - `mcp_offered`: whether this build shows the section at all ([`offered`]). No store is read.
//! - `mcp_state`: `{offered, on, clients, listening, error}`. Never a token: a token is shown once, by
//!   `mcp_connect_app`, and PM keeps only its hash.
//! - `mcp_set_on(on)`: the per-device switch (the device's file, not the store), then the listener is
//!   brought in line with it. Switching ON is refused where the section is not offered.
//! - `mcp_connect_app(name, scopes)`: makes the app, pairs it on this device, and answers with the
//!   token once, inside the config text an app's client pastes.
//! - `mcp_revoke(client_id)`: deletes the app's row and this device's pairing, and closes its
//!   connections.
//!
//! All four that touch PM's data go through [`blocking`]: the main window only, the store open and PM
//! not locked. Each reads the open vault's [`Here`] first and asks `is_open_in` once it holds the
//! store's connection, so a vault that closed in between is never written to.
//!
//! **The token's whole life outside the store.** It is made inside [`clients::mint`], turned into the
//! snippet in [`connect`], and returned. It is never logged here, never in a [`ModsError`] (every
//! refusal below is a fixed sentence), and [`Connected`]'s `Debug` leaves it out, so a stray `{:?}`
//! cannot print it either.

use std::ffi::OsString;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use rusqlite::Connection;
use serde::Serialize;
use serde_json::json;
use tauri::Webview;

use super::clients::{self, ClientView};
use super::device;
use super::supervisor;
use super::Fault;
use crate::mods::api::Host;
use crate::mods::devmode::Here;
use crate::mods::tab::commands::blocking;
use crate::mods::tab::error::{Code, ModsError, Result};

/// The operating systems where connecting apps is shown in a release build. Empty until Bobby has run
/// the live test (C-LT8) on one: the pipe's, the `PM --mcp` shim's and the launcher's behaviour there
/// is unverified, so the section stays out of sight rather than half-work. Widening it is this one
/// line. (A debug build always offers it, to be tested.)
const OFFERED_OSES: [&str; 0] = [];

/// Whether the section is offered on `os` (pure): a debug build, or an OS the list names.
pub(super) fn offered_on(os: &str, debug_build: bool, list: &[&str]) -> bool {
    debug_build || list.contains(&os)
}

/// Whether this build offers connecting apps at all.
pub fn offered() -> bool {
    offered_on(std::env::consts::OS, cfg!(debug_assertions), &OFFERED_OSES)
}

/// What the Settings section draws.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpState {
    /// Whether to show the section at all. When false nothing else is read: the rest is empty.
    pub offered: bool,
    /// This device's switch, for the open vault.
    pub on: bool,
    pub clients: Vec<ClientView>,
    /// Whether the socket is up now.
    pub listening: bool,
    /// PM's sentence for why it could not come up, if it could not.
    pub error: Option<String>,
}

impl McpState {
    /// What a computer that does not offer the section answers: nothing to show.
    fn hidden() -> Self {
        Self {
            offered: false,
            on: false,
            clients: Vec::new(),
            listening: false,
            error: None,
        }
    }
}

/// A freshly made app, as the page is told once. The only place its token leaves PM.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connected {
    pub client_id: i64,
    pub token: String,
    /// The config text an app's client pastes, with the token inside it.
    pub snippet: String,
}

// Not derived: a derived `Debug` would print the token and the snippet that holds it.
impl fmt::Debug for Connected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connected")
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

// ---- Sentences ------------------------------------------------------------------------------------

fn not_offered() -> ModsError {
    ModsError::new(
        Code::NotAllowed,
        "Connecting apps isn't available on this computer yet.",
    )
}

fn no_launcher() -> ModsError {
    ModsError::new(
        Code::NotAllowed,
        "PM couldn't work out how an app should start it, so it made no connection.",
    )
}

/// A refusal of the clients' or the device file's functions as the page reads it: PM's own sentence
/// (`Fault`'s `Display` never carries SQL, a path or the operating system's text).
fn fault(e: Fault) -> ModsError {
    let code = match e {
        Fault::Refused(_) => Code::BadRequest,
        Fault::Db(_) | Fault::Disk(_) => Code::Internal,
    };
    ModsError::new(code, e.to_string())
}

// ---- The vault and the store ---------------------------------------------------------------------

/// Run `work` with this device in the open vault and the store's connection, the vault checked again
/// once the connection is held.
fn with_here<H: Host, T>(
    host: &H,
    work: impl FnOnce(&Connection, &Here) -> Result<T>,
) -> Result<T> {
    let (vault, root) = host
        .store()
        .active_vault_place()
        .ok_or_else(ModsError::no_store)?;
    let here = Here::open(host.mcp().data_dir(), &vault, &root).map_err(ModsError::internal)?;
    let conn = host.store().conn().map_err(|_| ModsError::no_store())?;
    if !here.is_open_in(host.store()) {
        return Err(ModsError::no_store());
    }
    work(&conn, &here)
}

/// What the section shows now.
pub fn read_state<H: Host>(host: &H) -> Result<McpState> {
    read_state_if(host, offered())
}

/// [`read_state`] with `offered` given, so a test can stand in a build that hides the section.
pub(super) fn read_state_if<H: Host>(host: &H, offered: bool) -> Result<McpState> {
    if !offered {
        return Ok(McpState::hidden());
    }
    let (on, clients) = with_here(host, |conn, here| {
        Ok((device::device_state(here).on, clients::list(conn, here)))
    })?;
    let status = host.mcp().life().status();
    Ok(McpState {
        offered: true,
        on,
        clients,
        listening: status.listening,
        error: status.error.map(|e| e.to_string()),
    })
}

/// Switch the server on or off for this device in the open vault (the device's file). The caller
/// brings the listener in line.
pub(super) fn switch<H: Host>(host: &H, on: bool) -> Result<()> {
    with_here(host, |_conn, here| {
        device::set_on(here, on).map_err(|e| fault(e.into()))
    })
}

/// [`switch`] where the section may not be offered (`offered`): switching ON is refused there, and off
/// is always allowed, since a switch that cannot be turned off is the worst failure here.
pub(super) fn switch_checked<H: Host>(host: &H, on: bool, offered: bool) -> Result<()> {
    if on && !offered {
        return Err(not_offered());
    }
    switch(host, on)
}

/// Make an app holding `scopes`, paired on this device, and the config text that carries its token.
/// `command` is how an app's client starts PM. Nothing is made when `scopes` holds anything the app
/// may not be given (`calendar.details`, an unknown id): [`clients::mint`] refuses it whole.
pub(super) fn connect<H: Host>(
    host: &H,
    command: &str,
    name: &str,
    scopes: &[String],
) -> Result<Connected> {
    let (client_id, token) = with_here(host, |conn, here| {
        clients::mint(conn, here, name, scopes).map_err(fault)
    })?;
    let snippet = snippet(command, &token);
    Ok(Connected {
        client_id,
        token,
        snippet,
    })
}

/// [`connect`] where the section may not be offered (`offered`): nothing is made there, not even a
/// row, and the launcher is not asked.
pub(super) fn connect_checked<H: Host>(
    host: &H,
    name: &str,
    scopes: &[String],
    offered: bool,
    command: impl FnOnce() -> Option<String>,
) -> Result<Connected> {
    if !offered {
        return Err(not_offered());
    }
    // Worked out BEFORE anything is made: a refusal leaves no app that nothing can start.
    let command = command().ok_or_else(no_launcher)?;
    connect(host, &command, name, scopes)
}

/// Take an app away: its row, this device's pairing.
pub(super) fn revoke<H: Host>(host: &H, id: i64) -> Result<()> {
    with_here(host, |conn, here| {
        clients::revoke(conn, here, id).map_err(fault)
    })
}

// ---- The config an app pastes ---------------------------------------------------------------------

/// The config text for an app's MCP client: PM's own binary with `--mcp`, the token in the
/// environment the client gives the shim (never in its arguments, which any program can list).
fn snippet(command: &str, token: &str) -> String {
    let config = json!({
        "mcpServers": {
            "personal-manager": {
                "command": command,
                "args": ["--mcp"],
                "env": { "PM_MCP_TOKEN": token },
            }
        }
    });
    serde_json::to_string_pretty(&config).unwrap_or_default()
}

/// Whether `path` sits in an AppImage's temporary mount (`/tmp/.mount_XXXX/...`): a place that is
/// gone when PM quits, so a config naming it would stop working.
fn in_appimage_mount(path: &Path) -> bool {
    path.components().any(|c| match c {
        Component::Normal(part) => part.to_str().is_some_and(|p| p.starts_with(".mount_")),
        _ => false,
    })
}

/// How an app's client starts PM (pure): the AppImage file itself when PM runs from one (`appimage` is
/// its `APPIMAGE` variable), else the running binary. Never a path inside an AppImage's mount, never a
/// relative one, and `None` rather than a lossy guess when the path isn't text.
pub(super) fn launch_command(appimage: Option<OsString>, exe: Option<PathBuf>) -> Option<String> {
    let usable = |p: &Path| p.is_absolute() && !in_appimage_mount(p);
    appimage
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty() && usable(p))
        .or_else(|| exe.filter(|p| usable(p)))
        .and_then(|p| p.to_str().map(str::to_owned))
}

/// [`launch_command`] for this process.
fn this_launch_command() -> Option<String> {
    launch_command(std::env::var_os("APPIMAGE"), std::env::current_exe().ok())
}

// ---- Commands -------------------------------------------------------------------------------------

/// Whether this build shows connecting apps. Built in, so it needs no store and no window gate.
#[tauri::command]
pub fn mcp_offered() -> bool {
    offered()
}

/// What the Settings section draws: never a token.
#[tauri::command]
pub async fn mcp_state(webview: Webview) -> Result<McpState> {
    blocking(&webview, read_state).await
}

/// Switch the server for this device in this vault, then bring the socket in line. The answer is the
/// state as it is now, so a socket that could not come up is told in the same breath.
#[tauri::command]
pub async fn mcp_set_on(webview: Webview, on: bool) -> Result<McpState> {
    blocking(&webview, move |app| {
        switch_checked(app, on, offered())?;
        supervisor::after_open(app);
        read_state(app)
    })
    .await
}

/// Make an app and answer with its token, once.
#[tauri::command]
pub async fn mcp_connect_app(
    webview: Webview,
    name: String,
    scopes: Vec<String>,
) -> Result<Connected> {
    blocking(&webview, move |app| {
        connect_checked(app, &name, &scopes, offered(), this_launch_command)
    })
    .await
}

/// Take an app away and end its connections. The answer is the state as it is now.
#[tauri::command]
pub async fn mcp_revoke(webview: Webview, client_id: i64) -> Result<McpState> {
    blocking(&webview, move |app| {
        revoke(app, client_id)?;
        supervisor::on_revoked(app, client_id);
        read_state(app)
    })
    .await
}
