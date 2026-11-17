// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Loading a developer's test folder from the Developer tab (Mods spec §1.15, plan decision 8,
//! INVARIANTS.md I-35; slice 6, part 4): the stark screen of an unsigned file, for a folder.
//!
//! - [`load_test_mod`] reads the folder (`mods::folder::scan`: only the package layout, no link, the
//!   verifier's caps and manifest rules), runs every check the load would (`Ledger::preview_folder`:
//!   developer mode for this device read by Rust, the safety list by id and version, the API range,
//!   the id not held by another source) and judges D3 across the unsigned mods BEFORE the screen is
//!   offered, exactly as the unsigned file's does, and returns the screen *without loading
//!   anything*. The screen says the folder, that the mod comes from nobody (the manifest's author and
//!   repository are the file's own claim), and that a test mod and what it stores last until PM
//!   restarts. Its tier reads third party whatever the manifest says.
//! - the second step is `install_mod_consented` with the screen's token, the user's answers and the
//!   id they typed. It reads the folder again and loads it only if its `manifest.json` is still the
//!   one the screen was made from (the staged "bytes" are that manifest, so the token's integrity
//!   check covers it): what the user answered is what the manifest asks. Everything else in the
//!   folder is the developer's to change while they work, and a reload picks it up.
//! - [`reload_test_mod`] is the Reload button (and what the file watcher calls); [`unload_test_mod`]
//!   ends the session at once; [`list_test_mods`] says what is loaded, whether a reload failed and
//!   why, and whether a save reloads it.
//!
//! The commands are main-window only and refuse while the store is closed, like every Mods command.

use std::path::PathBuf;

use chrono::Utc;
use tauri::{AppHandle, Webview};

use super::commands::{blocking, current_epoch, parse_id, with_ledger, with_token, View};
use super::consent::{self, Answers};
use super::dev_commands::{require_dev_mode, this_device};
use super::error::{ModsError, Result};
use super::install::{ConsentScreen, Finished, InstallResult};
use super::stage::{self, Staged, Trust};
use super::unsigned::{self, UnsignedEnv};
use crate::mods::api::Host;
use crate::mods::folder::{self, Scan};
use crate::mods::folder_watch;
use crate::mods::ledger::{
    self, FolderLoad, Ledger, LedgerError, LedgerStore, Mint, Reloaded, Source, TestModStatus,
    ALLOW_ATLAS_FOLDERS,
};
use crate::mods::policy::InstallKind;
use crate::mods::refresh;
use crate::pathguard;

/// The stark screen for a scanned folder (pure over the manifest and what the checks found).
/// `held_others`: every grant the other unsigned mods hold, which D3 judges this one against.
pub fn screen(
    scan: &Scan,
    reload: bool,
    installed_version: Option<String>,
    held_others: &[(
        crate::mods::api::permissions::PolicyTier,
        crate::mods::api::permissions::Class,
    )],
    manifest_sha: &str,
    engine_sealed: bool,
) -> ConsentScreen {
    let mut s = unsigned::screen(
        &scan.pkg.manifest,
        reload,
        installed_version,
        held_others,
        manifest_sha,
        engine_sealed,
    );
    // A test folder is third party and unsigned whatever its manifest claims (an `atlas.` one in a
    // development build says first party): no band, badge or screen may read it as anything else.
    s.tier = crate::mods::ids::Tier::Third.as_str().to_owned();
    s.from = "folder";
    if let Some(u) = s.unsigned.as_mut() {
        u.test_folder = Some(scan.root.to_string_lossy().into_owned());
    }
    s
}

/// `manifest.json` as read and its sha256: what a folder's screen and token are about.
fn manifest_of(scan: &Scan) -> Result<(String, Vec<u8>)> {
    let bytes = scan
        .pkg
        .files
        .get(crate::mods::package::MANIFEST)
        .map(|f| f.bytes.clone())
        .ok_or_else(|| ModsError::internal("a scanned folder without a manifest"))?;
    Ok((stage::sha256_of(&bytes), bytes))
}

/// The folder as it is now still has the manifest the screen was made from (pure): the user's
/// answers are about that manifest.
pub fn check_same_manifest(staged: &Staged, now_sha256: &str) -> Result<()> {
    if staged.sha256 == now_sha256 {
        Ok(())
    } else {
        // Expired, not a bug: the developer edited the manifest while the screen was open.
        Err(ModsError::consent_expired())
    }
}

/// Run every check a load makes, judge D3 across the other unsigned mods, and build the stark screen
/// and what is staged behind it, loading nothing. Blocking.
pub fn prepare<S: LedgerStore>(
    ledger: &Ledger<'_, S>,
    scan: Scan,
    env: &UnsignedEnv<'_>,
) -> Result<(ConsentScreen, Staged)> {
    let preview = ledger.preview_folder(&scan, &env.options())?;
    let id = scan.pkg.manifest.id.clone();
    let held = unsigned::held_by_others(ledger, &id)?;
    let (sha256, bytes) = manifest_of(&scan)?;
    let screen = screen(
        &scan,
        preview.reload,
        preview.installed_version,
        &held,
        &sha256,
        env.engine_sealed,
    );
    let staged = Staged {
        id,
        version: scan.pkg.manifest.version.to_string(),
        sha256,
        source: Source::File,
        trust: Trust::Folder,
        bytes,
        folder: Some(scan.root),
    };
    Ok((screen, staged))
}

/// Load the staged folder after the user typed the id, and write the grants `answers` make. The
/// order is the unsigned file's: the typed id first, then the folder read again and its manifest
/// compared with the screen's, then the plan (D3 against what the other unsigned mods hold now),
/// then the load, then the grants in a transaction of their own. Blocking.
pub fn finish<S: LedgerStore>(
    ledger: &Ledger<'_, S>,
    staged: Staged,
    answers: &Answers,
    confirm_id: Option<&str>,
    env: &UnsignedEnv<'_>,
) -> Result<Finished> {
    // A token made for a package never finishes here.
    if staged.trust != Trust::Folder {
        return Err(ModsError::consent_expired());
    }
    unsigned::check_confirm(&staged.id, confirm_id)?;
    let root = staged
        .folder
        .clone()
        .ok_or_else(ModsError::consent_expired)?;
    let scan = folder::scan(&root, ALLOW_ATLAS_FOLDERS)?;
    let m = &scan.pkg.manifest;
    check_same_manifest(&staged, &manifest_of(&scan)?.0)?;
    if m.id != staged.id || m.version.to_string() != staged.version {
        // The manifest is the one hashed at the screen, so this is a bug, not an attack.
        return Err(ModsError::consent_expired());
    }
    let held = unsigned::held_by_others(ledger, &m.id)?;
    let plan = consent::plan_unsigned(&unsigned::declared_of(m), answers, &held, env.engine_sealed)
        .map_err(unsigned::plan_error)?;
    let FolderLoad { installed, reload } = ledger.load_folder(&scan, &env.options(), env.now)?;
    let id = m.id.clone();
    let wrote = ledger.with_conn(|c| {
        consent::apply_atomically(c, &id, &plan, env.engine_sealed).map_err(LedgerError::Grants)?;
        // A reload reset this mod's grants, which may free the other side for every other unsigned
        // mod: they are all told.
        Ok(if reload {
            consent::announce_unsigned_set(c, &id)?
        } else {
            consent::announce(c, &id, plan.touched())?
        })
    });
    let (grants_saved, grants_problem, told) = match wrote {
        Ok(told) => (true, None, told),
        Err(e) => (false, Some(ModsError::from(e)), Vec::new()),
    };
    Ok(Finished {
        result: InstallResult {
            id: id.as_str().to_owned(),
            version: installed.version,
            state: installed.state,
            kind: match installed.kind {
                InstallKind::Fresh => "fresh",
                InstallKind::Update => "update",
                InstallKind::Repair => "repair",
            },
            pruned: installed.pruned,
            grants_saved,
            grants_problem,
        },
        told,
    })
}

/// `install_mod_consented`'s second step for a folder's token (on a blocking thread).
pub(super) fn finish_folder(
    app: &AppHandle,
    view: &View,
    staged: Staged,
    decisions: &Answers,
    confirm_id: Option<&str>,
) -> Result<Finished> {
    // Again, as at the screen: the index may have gone since (a reset of the record).
    view.for_unsigned()?;
    let device = this_device(app)?;
    let env = UnsignedEnv {
        now: Utc::now(),
        index: view.index(),
        engine_sealed: Host::engine_sealed(app),
        device: &device,
    };
    let finished = with_ledger(app, Mint::Always, |ledger| {
        finish(ledger, staged, decisions, confirm_id, &env)
    })?;
    // The folder is loaded: watch it for changes, and tell the page (`install_mod_consented` also
    // announces the install and the grants).
    folder_watch::resync(app);
    refresh::emit_changed(app, refresh::CHANGED_TEST_MOD);
    Ok(finished)
}

// ---- The commands ---------------------------------------------------------------------------------

/// Read the folder at `path` (an absolute path, resolved, never a link out of it), run every check a
/// load makes, judge D3 across the unsigned mods, and return the stark screen, *without loading*.
/// Hand the screen's `token`, the user's answers and the id they typed to `install_mod_consented`.
/// Refused at once while developer mode is off for this device, before the folder is read.
#[tauri::command]
pub async fn load_test_mod(webview: Webview, path: String) -> Result<ConsentScreen> {
    blocking(&webview, move |app| {
        let epoch = current_epoch(app);
        let device = this_device(app)?;
        // Before the folder is read: a cheap refusal, and a hostile folder isn't scanned while it
        // is off. The ledger reads it again, as the authority.
        require_dev_mode(app, &device)?;
        let root: PathBuf =
            pathguard::sanitize_source(&path).map_err(|e| ModsError::bad_request(e.to_string()))?;
        let scan = folder::scan(&root, ALLOW_ATLAS_FOLDERS)?;
        let view = View::read(app)?;
        view.for_unsigned()?;
        let env = UnsignedEnv {
            now: Utc::now(),
            index: view.index(),
            engine_sealed: Host::engine_sealed(app),
            device: &device,
        };
        let (screen, staged) =
            with_ledger(app, Mint::Always, |ledger| prepare(ledger, scan, &env))?;
        with_token(app, epoch, screen, staged)
    })
    .await
}

/// Read a loaded folder again and replace what it serves: the Reload button. A reload that can't
/// take (a half-written manifest, a manifest that now asks for more than the user approved) leaves
/// the old one running and says why; it is also on the folder's entry in [`list_test_mods`].
#[tauri::command]
pub async fn reload_test_mod(webview: Webview, id: String) -> Result<Reloaded> {
    let id = parse_id(&id)?;
    blocking(&webview, move |app| {
        let done = ledger::reload_test_mod(app, &id);
        // A failed reload is a change too: the problem is on the session.
        refresh::emit_changed(app, refresh::CHANGED_TEST_MOD);
        folder_watch::resync(app);
        Ok(done?)
    })
    .await
}

/// End a loaded folder's session now: its row and what it stored go, its files are never touched.
#[tauri::command]
pub async fn unload_test_mod(webview: Webview, id: String) -> Result<()> {
    let id = parse_id(&id)?;
    blocking(&webview, move |app| {
        ledger::unload_test_mod(app, &id)?;
        folder_watch::resync(app);
        refresh::emit_changed(app, refresh::CHANGED_TEST_MOD);
        // The page hears the same through `mods://changed`; nothing else to say.
        let _ = app;
        Ok(())
    })
    .await
}

/// The folders loaded now.
#[tauri::command]
pub async fn list_test_mods(webview: Webview) -> Result<Vec<TestModStatus>> {
    blocking(&webview, |app| Ok(ledger::list_test_mods(app))).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::folder::tests::tool_folder;
    use crate::mods::ids::ModId;
    use serde_json::json;

    fn scanned(
        change: impl FnOnce(&mut serde_json::Value, &mut std::collections::BTreeMap<String, Vec<u8>>),
    ) -> (tempfile::TempDir, Scan) {
        let dir = tempfile::tempdir().unwrap();
        tool_folder(dir.path(), change);
        let scan = folder::scan(dir.path(), false).unwrap();
        (dir, scan)
    }

    #[test]
    fn the_screen_is_the_stark_one_for_a_folder_third_party_and_says_where_it_is() {
        let (dir, scan) = scanned(|_, _| {});
        let sha = "a".repeat(64);
        let s = screen(&scan, false, None, &[], &sha, true);
        assert_eq!(
            (s.tier.as_str(), s.kind, s.from),
            ("third", "fresh", "folder")
        );
        assert_eq!(s.id, "acme.tool");
        let u = s.unsigned.as_ref().expect("the stark screen");
        assert_eq!(u.confirm_id, "acme.tool");
        assert_eq!(
            u.test_folder.as_deref(),
            Some(std::fs::canonicalize(dir.path()).unwrap().to_str().unwrap())
        );
        // Every answer starts at no, as the unsigned file's does.
        assert!(s.permissions.iter().all(|p| !p.default_answer && !p.is_new));
        // A file's screen carries no folder.
        let file = unsigned::screen(&scan.pkg.manifest, false, None, &[], &sha, true);
        assert!(file.unsigned.unwrap().test_folder.is_none());
    }

    #[test]
    fn a_reload_of_an_earlier_folder_says_so_and_names_the_version_it_replaces() {
        let (_dir, scan) = scanned(|_, _| {});
        let s = screen(
            &scan,
            true,
            Some("0.9.0".into()),
            &[],
            &"a".repeat(64),
            true,
        );
        assert!(s.unsigned.unwrap().reinstall);
        assert_eq!(s.installed_version.as_deref(), Some("0.9.0"));
    }

    #[test]
    fn an_atlas_folder_in_a_development_build_reads_third_party_on_the_screen() {
        let (bytes, key) = crate::mods::package::tests::signed_package(|_, _| {});
        let pkg = crate::mods::package::verify(&bytes, &[key]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        crate::mods::folder::tests::write_files(dir.path(), &pkg.files);
        let scan = folder::scan(dir.path(), true).unwrap();
        assert_eq!(scan.pkg.manifest.publisher.tier.as_str(), "first");
        let s = screen(&scan, false, None, &[], &"a".repeat(64), true);
        assert_eq!(s.tier, "third");
        assert_eq!(s.from, "folder");
    }

    #[test]
    fn a_manifest_edited_since_the_screen_is_not_loaded_on_the_old_answers() {
        let (_dir, scan) = scanned(|_, _| {});
        let (sha, bytes) = manifest_of(&scan).unwrap();
        let staged = Staged {
            id: ModId::parse("acme.tool").unwrap(),
            version: "1.0.0".into(),
            sha256: sha.clone(),
            source: Source::File,
            trust: Trust::Folder,
            bytes,
            folder: Some(scan.root.clone()),
        };
        assert!(check_same_manifest(&staged, &sha).is_ok());
        let (_dir2, other) = scanned(|m, _| m["description"] = json!("changed"));
        let e = check_same_manifest(&staged, &manifest_of(&other).unwrap().0).unwrap_err();
        assert_eq!(e.code, super::super::error::Code::ConsentExpired);
    }

    /// The commands' source, split at each `#[tauri::command]`: the function's name and its body.
    fn command_bodies() -> Vec<(String, String)> {
        let source = include_str!("folder.rs");
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
    fn every_command_is_main_window_gated_unprefixed_and_registered() {
        let commands = command_bodies();
        assert_eq!(commands.len(), 4);
        let lib = include_str!("../../lib.rs");
        for (name, body) in &commands {
            assert!(
                !name.starts_with("set_"),
                "{name}: the Settings tick keys on `set_`"
            );
            assert!(
                body.contains("webview: Webview"),
                "{name} must see its window"
            );
            assert!(
                body.contains("blocking(&webview"),
                "{name} doesn't go through the window and store gate"
            );
            assert!(
                lib.contains(&format!("mods::tab::folder::{name},")),
                "{name} isn't in generate_handler!"
            );
        }
        assert_eq!(
            lib.matches("mods::tab::folder::").count(),
            commands.len(),
            "a folder command is registered that this file doesn't hold"
        );
    }

    #[test]
    fn loading_checks_developer_mode_before_it_reads_the_folder() {
        let (_, body) = command_bodies()
            .into_iter()
            .find(|(n, _)| n == "load_test_mod")
            .unwrap();
        let dev = body.find("require_dev_mode").unwrap();
        let scan = body.find("folder::scan").unwrap();
        assert!(dev < scan);
        // The path is resolved before it is read.
        assert!(body.find("sanitize_source").unwrap() < scan);
    }

    #[test]
    fn a_folder_token_finishes_only_as_a_folder_and_a_package_token_never_does() {
        // `finish_folder` is the only caller, from the one `Trust::Folder` arm, and `finish` refuses
        // anything else before it reads a thing.
        let source = include_str!("commands.rs");
        assert!(source.contains("Trust::Folder => {"));
        let body = include_str!("folder.rs");
        let finish = body.split("pub fn finish<S: LedgerStore>").nth(1).unwrap();
        let trust = finish.find("staged.trust != Trust::Folder").unwrap();
        let confirm = finish.find("check_confirm").unwrap();
        let scan = finish.find("folder::scan").unwrap();
        assert!(trust < confirm && confirm < scan);
        // And the other two finishers refuse a folder's token.
        assert!(include_str!("install.rs").contains("staged.trust != Trust::Signed"));
        assert!(include_str!("unsigned.rs").contains("staged.trust != Trust::Unsigned"));
    }
}
