// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `PM --mcp` is the pipe a connected app's MCP client talks through (Mods spec 1.16). It must be
    // answered here, BEFORE `run()`: the single-instance plugin would hand the flag to the running PM
    // and exit, and `run()` sets the WebKit environment and builds a window, which a protocol pipe
    // must never do.
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--mcp")) {
        std::process::exit(pm_lib::run_mcp_shim());
    }
    pm_lib::run()
}
