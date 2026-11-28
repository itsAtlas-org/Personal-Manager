// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Secrets the app holds and the posture of the machine it runs on: the OpenRouter keys,
//! the app lock, and the OS/packaging readouts.
//!
//! `unlock_app` is the APP lock — OS user presence, then the app-unlocked flag — and is
//! deliberately not beside `vault_lock_status` / `continue_here` / `force_take_vault` in
//! `vaults`, which are `lock_session`: which *device* currently holds the vault. The pair
//! reads as duplicated and is not; keep them apart.

use tauri::State;

use crate::error::{Error, Result};
use crate::{applock, openrouter, secrets, AppState};

// --- secrets ---

#[tauri::command]
pub fn has_openrouter_key() -> Result<bool> {
    Ok(secrets::get_openrouter_key()?.is_some())
}

/// A key for another provider pasted into an OpenRouter box would go to openrouter.ai as a Bearer
/// token: a key handed to another company. Refused before any write, for two kinds of key. One PM
/// already holds for a direct provider (`stored_elsewhere`, a comparison, so no prefix guess), and
/// one whose prefix is plainly another company's (the shared list in `direct_providers`: Anthropic's
/// `sk-ant-`, Google's `AIza`, OpenAI's project, service-account and admin shapes), which catches a
/// key not yet saved. An OpenRouter-style key (`sk-or-`), and the same OpenRouter key saved again,
/// pass. PURE over its arguments.
fn refuse_another_providers_key(
    key: &str,
    stored_elsewhere: impl FnOnce(&str) -> bool,
) -> Result<()> {
    if crate::direct_providers::looks_like_another_providers_key(
        crate::llm_gateway::CloudProvider::OpenRouter,
        key,
    ) || stored_elsewhere(key)
    {
        return Err(Error::Other(
            "That looks like a key from another provider, not an OpenRouter key. Paste it into Your own keys instead."
                .into(),
        ));
    }
    Ok(())
}

fn refuse_a_key_held_for_a_direct_provider(key: &str) -> Result<()> {
    refuse_another_providers_key(key, |k| {
        crate::llm_gateway::is_a_stored_key_of_another_provider(
            crate::llm_gateway::CloudProvider::OpenRouter,
            k,
        )
    })
}

#[tauri::command]
pub fn set_openrouter_key(key: String) -> Result<()> {
    let key = key.trim();
    if key.is_empty() {
        return Err(Error::Other("API key is empty".into()));
    }
    refuse_a_key_held_for_a_direct_provider(key)?;
    secrets::set_openrouter_key(key)?;
    forget_openrouter_trouble();
    Ok(())
}

/// A new OpenRouter key is a new chance: what the entry-health memory holds against OpenRouter (a
/// bad key, no credit) was about the OLD key, and keeping it would send replies to the next
/// company, a paid one, for up to ten minutes while the person believes OpenRouter is back.
fn forget_openrouter_trouble() {
    forget_openrouter_trouble_in(crate::entry_health::global());
}

fn forget_openrouter_trouble_in(health: &crate::entry_health::Health) {
    health.clear_provider(crate::llm_gateway::CloudProvider::OpenRouter);
}

#[tauri::command]
pub fn has_openrouter_background_key() -> Result<bool> {
    Ok(secrets::get_openrouter_background_key()?.is_some())
}

#[tauri::command]
pub fn set_openrouter_background_key(key: String) -> Result<()> {
    let key = key.trim();
    if key.is_empty() {
        return Err(Error::Other("API key is empty".into()));
    }
    refuse_a_key_held_for_a_direct_provider(key)?;
    secrets::set_openrouter_background_key(key)?;
    forget_openrouter_trouble();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new OpenRouter key clears what the entry-health memory holds against OpenRouter (a bad
    /// key is remembered for ten minutes and would send replies to the next company meanwhile).
    /// Both key boxes call it, and it touches no other provider.
    #[test]
    fn a_new_openrouter_key_forgets_the_old_ones_trouble() {
        use crate::error::FailureClass;
        use crate::llm_gateway::CloudProvider::{Anthropic, OpenRouter};
        // A private memory: any list edit in a parallel test clears the process-wide one.
        let health = crate::entry_health::private_for_test();
        health.mark(OpenRouter, "keys-test/model", FailureClass::KeyBad, None);
        health.mark(Anthropic, "keys-test-claude", FailureClass::KeyBad, None);
        assert!(health.is_down(OpenRouter, "keys-test/model").is_some());
        forget_openrouter_trouble_in(&health);
        assert!(health.is_down(OpenRouter, "keys-test/model").is_none());
        assert!(
            health.is_down(Anthropic, "keys-test-claude").is_some(),
            "another company's memory is not OpenRouter's to clear"
        );
        let src = include_str!("keys.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for sig in [
            "pub fn set_openrouter_key(",
            "pub fn set_openrouter_background_key(",
        ] {
            let body = src
                .split(sig)
                .nth(1)
                .unwrap()
                .split("\n}\n")
                .next()
                .unwrap();
            assert!(body.contains("forget_openrouter_trouble()"), "{sig}");
        }
    }

    /// The refusal is decided before any keychain write, whichever box the key was pasted into.
    #[test]
    fn a_key_for_another_provider_is_refused_in_an_openrouter_box() {
        let held = |k: &str| k == "held-openai-key-0123456789";
        // One PM already holds for a direct provider.
        assert!(refuse_another_providers_key("held-openai-key-0123456789", held).is_err());
        // An Anthropic key that is not saved yet, by its documented prefix.
        let e =
            refuse_another_providers_key("sk-ant-api03-abcdefghijklmnop", |_| false).unwrap_err();
        assert!(e.to_string().contains("another provider"), "{e}");
        // The other companies' unsaved keys, by the shared prefix list: OpenAI's three prefixed
        // shapes and Google's.
        for key in [
            "sk-proj-abcdefghijklmnop",
            "sk-svcacct-abcdefghijklmnop",
            "sk-admin-abcdefghijklmnop",
            "AIzaSyAbcdefghijklmnopqrstuvwxyz0123456",
        ] {
            let e = refuse_another_providers_key(key, |_| false).unwrap_err();
            assert!(e.to_string().contains("another provider"), "{key}: {e}");
        }
        // An OpenRouter-style key, and the same OpenRouter key saved again, pass.
        assert!(refuse_another_providers_key("sk-or-v1-abcdefghijklmnop", |_| false).is_ok());
        assert!(refuse_another_providers_key("or-key-primary-0123456789", held).is_ok());
    }
}

/// Run the OS verification (Windows Hello / Touch ID) to lift the launch lock. Returns
/// `true` on success, `false` when the user cancels/fails. The HWND is read on the UI
/// thread (it's `!Send`) and the blocking WinRT wait runs on a worker thread so the UI
/// stays responsive while the system prompt is up.
#[tauri::command]
pub async fn unlock_app(state: State<'_, AppState>, window: tauri::WebviewWindow) -> Result<bool> {
    let raw_handle = {
        #[cfg(target_os = "windows")]
        {
            window
                .hwnd()
                .map_err(|e| Error::Other(format!("no window handle for verification: {e}")))?
                .0 as isize
        }
        #[cfg(not(target_os = "windows"))]
        {
            // Unused off Windows (the stubs ignore it), but keep the binding so the
            // worker closure is identical across platforms.
            let _ = &window;
            0isize
        }
    };
    // Deliberately NOT `blocking::spawn_blocking_result`: this one says "verification task FAILED",
    // not "panicked", and that string reaches the user on the app-unlock path. Converting would be a
    // wording change dressed as a cleanup.
    let verified =
        tauri::async_runtime::spawn_blocking(move || applock::verify(raw_handle, "Unlock PM"))
            .await
            .map_err(|e| Error::Other(format!("verification task failed: {e}")))??;
    if verified {
        state
            .app_unlocked
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(verified)
}

/// Current Windows Smart App Control state, so the updater UI can warn before offering a
/// restart that SAC would silently block (an unsigned installer under SAC-enforced closes
/// PM and reopens on the old version with no error — see `crate::smart_app_control`).
/// Off-Windows, or when SAC is absent, this reports `Unknown` and the UI proceeds normally.
#[tauri::command]
pub fn smart_app_control_state() -> crate::smart_app_control::SmartAppControlState {
    crate::smart_app_control::state()
}

/// Whether the running app is a Linux **package** install (rpm/deb) rather than an AppImage.
/// Tauri's in-app updater can only replace an AppImage in place, so on a package install the
/// updater UI skips the (doomed) background auto-download and points the user at reinstalling
/// the new package instead. False on Windows, macOS, and the Linux AppImage.
#[tauri::command]
pub fn package_managed_linux() -> bool {
    crate::update_delivery::package_managed_linux()
}

/// The running macOS version (e.g. "14.5"), so the updater UI can hold back an update that
/// states a newer minimum macOS than this Mac has: macOS wouldn't open it, and the updater
/// replaces PM in place. `None` off macOS, or when the version can't be read.
#[tauri::command]
pub fn macos_version() -> Option<String> {
    crate::update_delivery::macos_version()
}

/// The OpenRouter model catalogue (public endpoint, no key needed) so the user can
/// browse, search, and pick a model with pricing in Settings (spec §6 — any model,
/// swappable).
#[tauri::command]
pub async fn list_models() -> Result<Vec<openrouter::ModelInfo>> {
    openrouter::list_models().await
}
