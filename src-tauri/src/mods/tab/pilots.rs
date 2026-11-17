// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The three commands the pilots' auto-install gives the page (`mods::autoinstall`, PR B plan B8):
//!
//! - `retry_auto_install`, the "Try now" under an "Installing" tab: fetch the safety list now and let
//!   its pass run the install (offline, the index already held may still do it);
//! - `mods_handover_state`, one read: which pilots landed in this vault, and whether the Map's
//!   device-local view prefs still wait to be handed over (both false in a store with no pilot, which
//!   is every store until a migration writes a marker);
//! - `adopt_map_prefs`, which carries those device-local prefs into the mod's storage, once, writer
//!   only, and only while the handover's flag stands.
//!
//! Each is main-window only through [`blocking`], and none is named `set_*`.

use serde::Serialize;
use tauri::{Manager, Webview};

use super::commands::blocking;
use super::error::{Code, ModsError, Result};
use crate::mods::autoinstall::{self, AdoptRefusal, HandoverState};
use crate::mods::refresh::{self, RefreshError};
use crate::AppState;

/// How "Try now" went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryResult {
    /// The safety list was fetched and taken, so the pass ran against a fresh index.
    pub fetched: bool,
    /// When it wasn't, why, in words.
    pub problem: Option<String>,
}

/// The answer from how the fetch went (pure).
pub fn retry_outcome(fetch: std::result::Result<(), &RefreshError>) -> RetryResult {
    let problem = match fetch {
        Ok(()) => None,
        Err(RefreshError::Fetch(_)) => {
            Some("PM couldn't reach the internet just now. It tries again on its own.".to_owned())
        }
        Err(e) => Some(format!("PM's safety list wasn't taken: {e}.")),
    };
    RetryResult {
        fetched: problem.is_none(),
        problem,
    }
}

/// "Try now": fetch the safety list and let the pass run (it installs what a marker is owed). When
/// the fetch fails, a pass still runs against the index already held. Nothing changes for a store
/// with no marker.
#[tauri::command]
pub async fn retry_auto_install(webview: Webview) -> Result<RetryResult> {
    // The window and the store gate, before any traffic.
    blocking(&webview, |_| Ok(())).await?;
    let app = webview.app_handle().clone();
    let fetched = refresh::refresh_once(&app).await;
    if fetched.is_err() {
        autoinstall::run_cached(&app).await;
    }
    refresh::emit_changed(&app, refresh::CHANGED_DECIDED);
    Ok(retry_outcome(fetched.as_ref().map(|_| ())))
}

/// Which pilots landed here and whether the Map's device-local prefs wait to be handed over.
#[tauri::command]
pub async fn mods_handover_state(webview: Webview) -> Result<HandoverState> {
    blocking(&webview, |app| {
        let state = app.state::<AppState>();
        let conn = state.conn().map_err(|_| ModsError::no_store())?;
        autoinstall::handover_state(&conn).map_err(ModsError::internal)
    })
    .await
}

/// Hand this device's Map view prefs (arrangement, cohesion, names) to the mod's storage, filling
/// only what it doesn't hold yet. `true` when it did (or there was nothing left to hand over, which
/// is the same to the page); the page keeps its keys either way, since another vault on the device
/// is owed them too.
#[tauri::command]
pub async fn adopt_map_prefs(
    webview: Webview,
    layout_mode: String,
    cohesion: f64,
    labels: bool,
) -> Result<bool> {
    blocking(&webview, move |app| {
        let state = app.state::<AppState>();
        // Only the writer: a store that isn't the writer's is closed anyway, and this says so too.
        if !crate::lock_session::this_instance_writes(state.inner()) {
            return Err(ModsError::no_store());
        }
        let mut conn = state.conn().map_err(|_| ModsError::no_store())?;
        autoinstall::adopt_map_prefs(&mut conn, &layout_mode, cohesion, labels).map_err(|e| match e
        {
            AdoptRefusal::BadValue => ModsError::bad_request("That isn't a setting the Map has."),
            AdoptRefusal::NotInstalled => {
                ModsError::new(Code::NotInstalled, "The Map isn't installed here yet.")
            }
            AdoptRefusal::Db(e) => ModsError::internal(e),
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::fetch::FetchError;

    #[test]
    fn the_retry_says_offline_in_words_and_a_taken_index_says_nothing() {
        assert_eq!(
            retry_outcome(Ok(())),
            RetryResult {
                fetched: true,
                problem: None
            }
        );
        let offline = RefreshError::Fetch(FetchError::Offline("down".into()));
        let r = retry_outcome(Err(&offline));
        assert!(!r.fetched);
        assert!(r.problem.unwrap().contains("couldn't reach the internet"));
        let refused = RefreshError::Refused("it is old".into());
        let r = retry_outcome(Err(&refused));
        assert!(!r.fetched && r.problem.unwrap().contains("it is old"));
    }

    #[test]
    fn the_commands_are_main_window_only_writer_gated_and_registered() {
        let source = include_str!("pilots.rs");
        let production = source.split("#[cfg(test)]\nmod tests").next().unwrap();
        for name in [
            "retry_auto_install",
            "mods_handover_state",
            "adopt_map_prefs",
        ] {
            let at = production
                .find(&format!("pub async fn {name}("))
                .unwrap_or_else(|| panic!("{name}"));
            let body = &production[at..];
            let body = body.split("#[tauri::command]").next().unwrap();
            assert!(
                body.contains("blocking(&webview"),
                "{name}: the window gate"
            );
            assert!(!name.starts_with("set_"));
            let lib = include_str!("../../lib.rs");
            assert!(
                lib.contains(&format!("mods::tab::pilots::{name},")),
                "{name}"
            );
        }
        let adopt = production
            .split("pub async fn adopt_map_prefs(")
            .nth(1)
            .unwrap();
        assert!(
            adopt.find("this_instance_writes").unwrap() < adopt.find("state.conn()").unwrap(),
            "the writer is asked before the store is touched"
        );
    }
}
