// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The commands the pilots' auto-install gives the page (`mods::autoinstall`, PR B plan B8; the last
//! four are the Pinboard's, PR C K8, dormant until a migration writes its markers):
//!
//! - `retry_auto_install`, the "Try now" under an "Installing" tab: fetch the safety list now and let
//!   its pass run the install (offline, the index already held may still do it);
//! - `mods_handover_state`, one read: which pilots landed in this vault, and whether the Map's
//!   device-local view prefs still wait to be handed over (both false in a store with no pilot: one with
//!   no documents, or a fresh one, since v61 writes the Map's marker only where a document exists);
//! - `adopt_map_prefs`, which carries those device-local prefs into the mod's storage, once, writer
//!   only, and only while the handover's flag stands;
//! - `adopt_pinboard_prefs`, the same for the Pinboard's three nags;
//! - `pinboard_move_state`, which strips the page draws about the Pinboard's move (a conflict, a
//!   repaired copy, a board that cannot move), `pinboard_use_old_board` and `pinboard_move_dismiss`,
//!   the two answers to them (`mods::pinboard_move`).
//!
//! Each is main-window only through [`blocking`], and none is named `set_*`.

use serde::Serialize;
use tauri::{Manager, Webview};

use super::commands::blocking;
use super::error::{Code, ModsError, Result};
use crate::mods::autoinstall::{self, AdoptRefusal, HandoverState};
use crate::mods::pinboard_move::{self, MoveState, UseOldRefusal};
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

/// Hand this device's three Pinboard nags to the mod's storage, filling only what it doesn't hold
/// yet. `None` is a nag this device never set. `true` when it did; the page keeps its keys.
#[tauri::command]
pub async fn adopt_pinboard_prefs(
    webview: Webview,
    confirm_delete: Option<bool>,
    show_past: Option<bool>,
    show_completed: Option<bool>,
) -> Result<bool> {
    blocking(&webview, move |app| {
        let state = app.state::<AppState>();
        if !crate::lock_session::this_instance_writes(state.inner()) {
            return Err(ModsError::no_store());
        }
        let mut conn = state.conn().map_err(|_| ModsError::no_store())?;
        pinboard_move::adopt_pinboard_prefs(&mut conn, confirm_delete, show_past, show_completed)
            .map_err(|e| match e {
                AdoptRefusal::BadValue => ModsError::bad_request("That isn't a Pinboard setting."),
                AdoptRefusal::NotInstalled => {
                    ModsError::new(Code::NotInstalled, "The Pinboard isn't installed here yet.")
                }
                AdoptRefusal::Db(e) => ModsError::internal(e),
            })
    })
    .await
}

/// What the strips about the Pinboard's move say: booleans and a word, never the board's text.
#[tauri::command]
pub async fn pinboard_move_state(webview: Webview) -> Result<MoveState> {
    blocking(&webview, |app| {
        let state = app.state::<AppState>();
        let conn = state.conn().map_err(|_| ModsError::no_store())?;
        pinboard_move::state(&conn).map_err(ModsError::internal)
    })
    .await
}

/// "Use my old board": see [`pinboard_move::use_old_board`]. Nothing is deleted, and a refusal
/// changes nothing.
#[tauri::command]
pub async fn pinboard_use_old_board(webview: Webview) -> Result<()> {
    let result = blocking(&webview, |app| {
        let state = app.state::<AppState>();
        if !crate::lock_session::this_instance_writes(state.inner()) {
            return Err(ModsError::no_store());
        }
        let mut conn = state.conn().map_err(|_| ModsError::no_store())?;
        pinboard_move::use_old_board(&mut conn).map_err(use_old_error)
    })
    .await;
    if result.is_ok() {
        refresh::emit_changed(webview.app_handle(), refresh::CHANGED_DECIDED);
    }
    result
}

/// What each way "Use my old board" can be refused says. Fixed words.
fn use_old_error(e: UseOldRefusal) -> ModsError {
    match e {
        UseOldRefusal::NoConflict => ModsError::new(
            Code::Changed,
            "There is nothing to choose between any more. Nothing was changed.",
        ),
        UseOldRefusal::NotInstalled => {
            ModsError::new(Code::NotInstalled, "The Pinboard isn't installed here.")
        }
        UseOldRefusal::Changed => ModsError::new(
            Code::Changed,
            "The old board in PM's store isn't the one this question was about, so nothing was changed.",
        ),
        UseOldRefusal::Unreadable => ModsError::new(
            Code::Refused,
            "PM can't read the old board well enough to move it, so nothing was changed. Both boards are still safe.",
        ),
        UseOldRefusal::TooBig => ModsError::new(
            Code::Refused,
            "The old board is bigger than a mod can save, so nothing was changed. Both boards are still safe.",
        ),
        UseOldRefusal::CopyInTheWay => ModsError::new(
            Code::Refused,
            "The mod already holds a saved copy called \"board before move\", and PM won't replace it, so nothing was changed. Both boards are still safe.",
        ),
        UseOldRefusal::Failed => ModsError::new(
            Code::Internal,
            "PM couldn't switch the boards, so nothing was changed. Both boards are still safe.",
        ),
        UseOldRefusal::Db(e) => ModsError::internal(e),
    }
}

/// Clear a Pinboard strip's note: `conflict` ("Keep this one") or `repaired`. Nothing else is touched.
#[tauri::command]
pub async fn pinboard_move_dismiss(webview: Webview, note: String) -> Result<()> {
    blocking(&webview, move |app| {
        let state = app.state::<AppState>();
        if !crate::lock_session::this_instance_writes(state.inner()) {
            return Err(ModsError::no_store());
        }
        let conn = state.conn().map_err(|_| ModsError::no_store())?;
        match pinboard_move::dismiss(&conn, &note) {
            Ok(true) => Ok(()),
            Ok(false) => Err(ModsError::bad_request("That isn't a note PM keeps.")),
            Err(e) => Err(ModsError::internal(e)),
        }
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
            "adopt_pinboard_prefs",
            "pinboard_move_state",
            "pinboard_use_old_board",
            "pinboard_move_dismiss",
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
