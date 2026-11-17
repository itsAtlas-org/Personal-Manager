// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Mods tab's commands (Mods spec §1.13): the thin edge over the pure pieces in this folder.
//! Every command
//! - is main-window only (the webview's own label, like `mod_call`) and refuses with a typed
//!   [`ModsError`] otherwise;
//! - refuses while the store is closed (the vault is locked, or another PM window is the writer: a
//!   non-writer closes its store) and while PM's app lock holds, in that order, before it reads or
//!   writes anything ([`preconditions`]);
//! - runs its file, network-free work and its DB steps in `spawn_blocking`, never under the DB lock
//!   across file I/O, and never re-taking the lock it holds (the ledger and each helper take it for
//!   one step at a time);
//! - announces what changed once it has committed: `mods://changed` after any change to the rows or
//!   the registry, `grants.changed` to the frames whose answers may now differ.
//!
//! The commands are named for what they do, none with a `set_` prefix (the Settings footer's
//! "Saved" tick keys on that prefix), and each has a wrapper in `ipc.ts`.
//!
//! **Signed installs are two commands** (`install_mod_file` or `install_from_catalogue` or
//! `approve_update`, then `install_mod_consented`): the first returns the consent screen without
//! installing, the second installs the bytes it staged ([`super::stage`]). **Signed installs and
//! repairs refuse while this machine's record of the safety list is unusable**: with the revoked
//! keys unknown, a key the list revoked could still sign a package ([`View::for_install`]).

use std::collections::BTreeMap;
use std::time::Instant;

use chrono::Utc;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Webview};

use super::catalogue::{self, Catalogue};
use super::consent::{self, Answers};
use super::device;
use super::disclaimer;
use super::error::{Code, ModsError, Result};
use super::icon::{self, Icon};
use super::install::{self, ConsentScreen, Env, InstallResult};
use super::listing::{self, ModDto};
use super::stage::{Stage, Staged, Trust};
use super::status::{self, SafetyStatus};
use super::uninstall::{self, UninstallPreview, Uninstalled};
use crate::mods::ai as money;
use crate::mods::api::events as mod_events;
use crate::mods::api::grants;
use crate::mods::api::{self, Host, PromptInfo};
use crate::mods::fetch::{Endpoints, ReqwestTransport};
use crate::mods::folder_watch;
use crate::mods::ids::ModId;
use crate::mods::index::Index;
use crate::mods::index_state::{self, Loaded};
use crate::mods::keys::TrustedKeys;
use crate::mods::ledger::{self, Ledger, Mint, Source, Standing};
use crate::mods::refresh::{self, RefreshError};
use crate::mods::safety::{self, Mode, SafetyCheck};
use crate::mods::secrets;
use crate::mods::store;
use crate::mods::MAIN_LABEL;
use crate::AppState;

// ---- The gate every command passes ---------------------------------------------------------------

/// Mods live in the main window only: exactly its label, nothing like it (pure).
pub fn check_window(label: &str) -> Result<()> {
    if label == MAIN_LABEL {
        Ok(())
    } else {
        Err(ModsError::new(
            Code::NotMainWindow,
            "Mods run only in PM's main window.",
        ))
    }
}

/// The window, the store, then the lock (pure): the main window's own label and nothing like it; a
/// store that is open; PM not locked. `locked` is `None` when the lock couldn't be read, which is
/// the store being closed.
pub fn preconditions(label: &str, store_open: bool, locked: Option<bool>) -> Result<()> {
    check_window(label)?;
    if !store_open || locked.is_none() {
        return Err(ModsError::no_store());
    }
    if locked == Some(true) {
        return Err(ModsError::new(
            Code::Locked,
            "PM is locked. Unlock it to manage your mods.",
        ));
    }
    Ok(())
}

/// The gate for the store and the lock, once the window is known to be the main one. Blocking: it
/// reads the lock setting.
fn ready(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let locked = Host::app_locked(app).ok();
    preconditions(MAIN_LABEL, state.active_vault_id().is_some(), locked)
}

/// Run `f` on a blocking thread, after the window check and the store gate. Everything a command
/// does that touches a file or the store goes through here.
pub(super) async fn blocking<T: Send + 'static>(
    webview: &Webview,
    f: impl FnOnce(&AppHandle) -> Result<T> + Send + 'static,
) -> Result<T> {
    // The window first, on the caller's thread: a refusal costs nothing.
    check_window(webview.label())?;
    let app = webview.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        ready(&app)?;
        f(&app)
    })
    .await
    .map_err(ModsError::internal)?
}

pub(super) fn parse_id(id: &str) -> Result<ModId> {
    ModId::parse(id).map_err(|e| ModsError::bad_request(e.to_string()))
}

/// What a command knows of the safety list: the record as read, the keys it still trusts and the
/// index in force.
pub(super) struct View {
    loaded: Loaded,
    keys: TrustedKeys,
}

impl View {
    pub(super) fn read(app: &AppHandle) -> Result<Self> {
        let data_dir = crate::paths::data_dir(app).map_err(ModsError::internal)?;
        let loaded = index_state::load(&data_dir, safety::ring());
        let keys = safety::trusted_keys(&loaded.state);
        Ok(Self { loaded, keys })
    }

    pub(super) fn index(&self) -> Option<&Index> {
        self.loaded.index.as_ref().map(|v| &v.index)
    }

    /// A signed install or repair needs a record it can trust: without it the revoked keys are
    /// unknown. An unsigned install needs it too, for the blacklist by sha256 and the catalogue's
    /// ids.
    pub(super) fn for_install(&self) -> Result<()> {
        install_gate(&self.loaded)
    }

    /// An unsigned file or a test folder needs the signed index itself, not only a record that isn't
    /// broken: the catalogue's ids are the one thing that keeps such a file from squatting on a
    /// second-party mod's id, and with no cached index nothing says which ids those are (and nothing
    /// later re-judges the row once one arrives).
    pub(super) fn for_unsigned(&self) -> Result<()> {
        unsigned_gate(&self.loaded)
    }

    fn env<'a>(&'a self, app: &AppHandle) -> Env<'a> {
        Env {
            now: Utc::now(),
            keys: &self.keys,
            index: self.index(),
            engine_sealed: Host::engine_sealed(app),
        }
    }
}

/// Whether a signed install or repair may go ahead on `loaded`'s record (pure): not while the record
/// is unusable, since its revoked keys are then unknown (INVARIANTS.md I-33).
pub fn install_gate(loaded: &Loaded) -> Result<()> {
    match refresh::fault_of(loaded) {
        None => Ok(()),
        Some((why, _)) => Err(ModsError::new(
            Code::SafetyList,
            format!("PM can't install anything until its safety list record is mended: {why}."),
        )),
    }
}

/// Whether an unsigned file or a test folder may be prepared or finished on `loaded`'s record (pure):
/// the signed install's gate, and a cached index to check the id and the blacklist against.
pub fn unsigned_gate(loaded: &Loaded) -> Result<()> {
    install_gate(loaded)?;
    if loaded.index.is_none() {
        return Err(ModsError::new(
            Code::SafetyList,
            "PM hasn't loaded its safety list on this computer yet, and it checks an unsigned mod's \
             id against it. Try again once PM has fetched it (it does so on its own when you are \
             online).",
        ));
    }
    Ok(())
}

/// A ledger over the open store for a change to mods: the store folder is minted when `mint` says.
pub(super) fn with_ledger<T>(
    app: &AppHandle,
    mint: Mint,
    f: impl FnOnce(&Ledger<'_, AppState>) -> Result<T>,
) -> Result<T> {
    ledger::with_active_ledger(app, mint, f).ok_or_else(ModsError::no_store)?
}

/// A ledger for a change to an existing mod: with no mods folder there is no such mod.
fn with_existing<T>(
    app: &AppHandle,
    f: impl FnOnce(&Ledger<'_, AppState>) -> Result<T>,
) -> Result<T> {
    ledger::with_active_ledger(app, Mint::IfRows, f).ok_or_else(ModsError::not_installed)?
}

// ---- Reading --------------------------------------------------------------------------------------

/// Every installed mod, as the Mods tab lists it.
#[tauri::command]
pub async fn list_mods(webview: Webview) -> Result<Vec<ModDto>> {
    blocking(&webview, |app| {
        // The index is read before the DB lock is taken: a file read never holds the store.
        let view = View::read(app)?;
        let ctx = listing::Ctx {
            index: view.index(),
            engine_sealed: Host::engine_sealed(app),
        };
        let state = app.state::<AppState>();
        let conn = state.conn().map_err(|_| ModsError::no_store())?;
        listing::list(&conn, &ctx).map_err(ModsError::internal)
    })
    .await
}

/// A mod's icon as bytes and a media type, for a `blob:` URL (the page's CSP has no `pmmod:` in
/// `img-src`, and isn't widened). Only a PNG or WebP of the installed, verified files.
#[tauri::command]
pub async fn mod_icon(webview: Webview, mod_id: String) -> Result<Icon> {
    let id = parse_id(&mod_id)?;
    blocking(&webview, move |app| {
        with_existing(app, |ledger| icon::read(ledger, &id))
    })
    .await
}

/// The signed catalogue, from the cached index.
#[tauri::command]
pub async fn list_catalogue(webview: Webview) -> Result<Catalogue> {
    blocking(&webview, |app| {
        let view = View::read(app)?;
        let installed = installed_versions(app)?;
        Ok(catalogue::list(view.index(), &installed, Utc::now()))
    })
    .await
}

fn installed_versions(app: &AppHandle) -> Result<BTreeMap<String, String>> {
    let state = app.state::<AppState>();
    let conn = state.conn().map_err(|_| ModsError::no_store())?;
    let mut stmt = conn
        .prepare("SELECT id, version FROM mods")
        .map_err(ModsError::internal)?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(ModsError::internal)?;
    rows.collect::<rusqlite::Result<_>>()
        .map_err(ModsError::internal)
}

// ---- Installing: the consent screen, then the install ---------------------------------------------

/// Which store is open now. A command that builds a consent screen reads this FIRST, before it
/// reads anything the screen is judged against, and hands it to [`with_token`]: read any later, a
/// vault switch or lock during the build would stamp store A's screen with store B's epoch, and
/// `Stage::take` would accept it. Read first, it can only be older than the live epoch, which
/// `take` refuses (the epoch is never ahead of the store: see `AppState::store_epoch`).
pub(super) fn current_epoch(app: &AppHandle) -> u64 {
    app.state::<AppState>().store_epoch()
}

/// Stage `staged` and hand the screen its token, bound to `epoch`: the store that was open when the
/// command began (see [`current_epoch`]).
pub(super) fn with_token(
    app: &AppHandle,
    epoch: u64,
    mut screen: ConsentScreen,
    staged: Staged,
) -> Result<ConsentScreen> {
    let stage = app.state::<Stage>();
    screen.token = stage
        .put(staged, Instant::now(), epoch)
        .ok_or_else(|| ModsError::internal("no randomness for a consent token"))?;
    Ok(screen)
}

/// Read and verify the signed `.pmmod` at `path` (size-capped), run every check an install would,
/// and return what the consent screen needs, *without installing*. Hand the screen's `token` and the
/// user's answers to `install_mod_consented`. An unsigned file is refused with the `unsigned` code:
/// that install is the developer path's.
#[tauri::command]
pub async fn install_mod_file(webview: Webview, path: String) -> Result<ConsentScreen> {
    blocking(&webview, move |app| {
        let epoch = current_epoch(app);
        let bytes = install::read_file(std::path::Path::new(&path))?;
        let view = View::read(app)?;
        view.for_install()?;
        let env = view.env(app);
        let (screen, staged) = with_ledger(app, Mint::Always, |ledger| {
            install::prepare(ledger, bytes, Source::File, &env)
        })?;
        with_token(app, epoch, screen, staged)
    })
    .await
}

/// Install what the consent screen `token` staged and write the grants `decisions` make (permission
/// id to yes or no, one for each permission the screen said it needs an answer for, and any it
/// likes for the rest). The token is good once. A screen made for an unsigned file
/// (`install_unsigned_file`) finishes here too, and only as one: `confirm_id` must then be the mod's
/// id, typed by the user (checked in Rust), and developer mode is read again; a signed screen
/// ignores `confirm_id`.
#[tauri::command]
pub async fn install_mod_consented(
    webview: Webview,
    token: String,
    decisions: BTreeMap<String, bool>,
    confirm_id: Option<String>,
) -> Result<InstallResult> {
    blocking(&webview, move |app| {
        let staged = app
            .state::<Stage>()
            .take(
                &token,
                Instant::now(),
                app.state::<AppState>().store_epoch(),
            )
            .ok_or_else(ModsError::consent_expired)?;
        let view = View::read(app)?;
        view.for_install()?;
        let finished = match staged.trust {
            Trust::Signed => {
                let env = view.env(app);
                with_ledger(app, Mint::Always, |ledger| {
                    install::finish(ledger, staged, &decisions as &Answers, &env)
                })?
            }
            Trust::Unsigned => super::dev_commands::finish_unsigned(
                app,
                &view,
                staged,
                &decisions,
                confirm_id.as_deref(),
            )?,
            Trust::Folder => {
                super::folder::finish_folder(app, &view, staged, &decisions, confirm_id.as_deref())?
            }
        };
        announce(app, &finished.told);
        refresh::emit_changed(app, refresh::CHANGED_INSTALLED);
        Ok(finished.result)
    })
    .await
}

/// Download `entry`'s package and stage it for consent: the shared half of the catalogue installs.
async fn catalogue_screen(
    webview: &Webview,
    mod_id: String,
    must_be_installed: bool,
) -> Result<ConsentScreen> {
    let id = parse_id(&mod_id)?;
    // 1. What may be installed, decided from the record, the index and the row.
    let plan = blocking(webview, move |app| {
        // Taken before the plan is judged: the download below can be long, and a switch during it
        // must leave the token stale.
        let epoch = current_epoch(app);
        let view = View::read(app)?;
        view.for_install()?;
        let entry = catalogue::find(view.index(), id.as_str())?.clone();
        if let Some(index) = view.index() {
            if let Some(refusal) =
                catalogue::refusal(&catalogue::availability(&entry, index, Utc::now()))
            {
                return Err(refusal);
            }
        }
        let installed = installed_versions(app)?.remove(id.as_str());
        match (&installed, must_be_installed) {
            (None, true) => return Err(ModsError::not_installed()),
            (Some(have), _) if !catalogue::is_newer(&entry, have) => {
                return Err(ModsError::new(
                    Code::Refused,
                    "It is already installed, and the catalogue has nothing newer.",
                ))
            }
            _ => {}
        }
        if installed.is_some()
            && installed_from(app, &id)?.as_deref() != Some(store::FROM_CATALOGUE)
        {
            return Err(ModsError::new(
                Code::NotFromCatalogue,
                "This mod was installed from a file, so the catalogue doesn't update it.",
            ));
        }
        Ok((entry, epoch))
    })
    .await?;
    let (plan, epoch) = plan;
    // 2. The download, off every lock, checked against the signed index's sha256.
    let endpoints = Endpoints::machine();
    let transport = ReqwestTransport::new(&endpoints);
    let bytes = install::download(&transport, &endpoints, &plan.url, &plan.sha256).await?;
    // 3. Verified and staged, with the checks run again over the same index.
    blocking(webview, move |app| {
        let view = View::read(app)?;
        view.for_install()?;
        let env = view.env(app);
        let (screen, staged) = with_ledger(app, Mint::Always, |ledger| {
            install::prepare(ledger, bytes, Source::Catalogue, &env)
        })?;
        if staged.id != plan.id || staged.version != plan.version.as_str() {
            return Err(ModsError::new(
                Code::DownloadRefused,
                "That download isn't the version the catalogue lists.",
            ));
        }
        with_token(app, epoch, screen, staged)
    })
    .await
}

fn installed_from(app: &AppHandle, id: &ModId) -> Result<Option<String>> {
    use rusqlite::OptionalExtension;
    let state = app.state::<AppState>();
    let conn = state.conn().map_err(|_| ModsError::no_store())?;
    conn.query_row(
        "SELECT installed_from FROM mods WHERE id = ?1",
        [id.as_str()],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(ModsError::internal)
}

/// Download a catalogue mod and return its consent screen, installing nothing. Then
/// `install_mod_consented`.
#[tauri::command]
pub async fn install_from_catalogue(webview: Webview, mod_id: String) -> Result<ConsentScreen> {
    catalogue_screen(&webview, mod_id, false).await
}

/// An update waiting for the user (it asks for more than was granted): download the newest version
/// and return the consent screen with what is new marked, then `install_mod_consented` writes the
/// new grants at consent.
#[tauri::command]
pub async fn approve_update(webview: Webview, mod_id: String) -> Result<ConsentScreen> {
    catalogue_screen(&webview, mod_id, true).await
}

/// What a repair did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Repaired {
    pub id: String,
    pub version: String,
    pub state: String,
}

/// Put back exactly the installed version and file of a signed catalogue mod (a parked one whose
/// files are missing or changed): the pinned release is downloaded and checked against the sha256
/// the row recorded, after the safety list is asked again. Keeps the mod's grants and its own
/// consent; gives none.
#[tauri::command]
pub async fn repair_mod(webview: Webview, mod_id: String) -> Result<Repaired> {
    let id = parse_id(&mod_id)?;
    let id2 = id.clone();
    let target = blocking(&webview, move |app| {
        View::read(app)?.for_install()?;
        match installed_from(app, &id2)?.as_deref() {
            None => return Err(ModsError::not_installed()),
            Some(store::FROM_CATALOGUE) => {}
            Some(_) => {
                return Err(ModsError::new(
                    Code::NotFromCatalogue,
                    "This mod was installed from a file. Install the file again to repair it.",
                ))
            }
        }
        with_existing(app, |ledger| Ok(ledger.repair_target(&id2)?))
    })
    .await?;
    let endpoints = Endpoints::machine();
    let transport = ReqwestTransport::new(&endpoints);
    let bytes = install::download(&transport, &endpoints, &target.url, &target.sha256).await?;
    blocking(&webview, move |app| {
        let view = View::read(app)?;
        view.for_install()?;
        let installed = with_existing(app, |ledger| {
            Ok(ledger.install_repair(&target, &bytes, &view.keys, view.index(), Utc::now())?)
        })?;
        refresh::emit_changed(app, refresh::CHANGED_INSTALLED);
        Ok(Repaired {
            id: id.as_str().to_owned(),
            version: installed.version,
            state: installed.state,
        })
    })
    .await
}

// ---- Uninstalling ---------------------------------------------------------------------------------

/// What uninstalling would take and leave.
#[tauri::command]
pub async fn uninstall_preview(webview: Webview, mod_id: String) -> Result<UninstallPreview> {
    let id = parse_id(&mod_id)?;
    blocking(&webview, move |app| {
        with_existing(app, |ledger| uninstall::preview(ledger, &id))
    })
    .await
}

/// Uninstall through the ledger and the runtime, deleting what the mod made only when
/// `also_delete_gifts` says so. Show `uninstall_preview` first: this removes the mod's private
/// storage with it. The answer says what the page prunes from `localStorage`.
#[tauri::command]
pub async fn uninstall_mod(
    webview: Webview,
    mod_id: String,
    also_delete_gifts: bool,
) -> Result<Uninstalled> {
    let id = parse_id(&mod_id)?;
    blocking(&webview, move |app| {
        let done = with_existing(app, |ledger| uninstall::run(ledger, &id, also_delete_gifts))?;
        // A loaded test folder's session went with the row; its file watches (a hard inotify
        // budget, `folder_watch`) are dropped here, since only a resync drops one.
        folder_watch::resync(app);
        // The ledger pruned the mod's overlay out of the calendar's stored blob; every window's cache
        // would write it back with its next calendar write, so each is told to re-read it, under a
        // sender id no window owns (build log, slice 3 finding 7).
        let _ = app.emit(
            "prefs://stored",
            serde_json::json!({"key": "calendar_ui", "from": "pm-backend"}),
        );
        if let Some(gone) = &done.gifts_deleted {
            if gone.tasks + gone.milestones > 0 {
                let _ = app.emit(crate::tasks::TASKS_CHANGED, ());
                crate::briefing::nudge(&app.state::<AppState>());
            }
        }
        refresh::emit_changed(app, refresh::CHANGED_UNINSTALLED);
        Ok(done)
    })
    .await
}

// ---- Pausing, resuming, turning on -----------------------------------------------------------------

/// Pause a mod for the user: its sessions end after the row commits, its data stays.
#[tauri::command]
pub async fn pause_mod(webview: Webview, mod_id: String) -> Result<()> {
    let id = parse_id(&mod_id)?;
    blocking(&webview, move |app| {
        with_existing(app, |ledger| Ok(ledger.pause_by_user(&id, Utc::now())?))?;
        refresh::emit_changed(app, refresh::CHANGED_PAUSED);
        Ok(())
    })
    .await
}

/// Turn the user's own pause off. Refused with the safety list's words while it names the mod, and
/// never lifts the safety list's or developer mode's pause.
#[tauri::command]
pub async fn resume_mod(webview: Webview, mod_id: String) -> Result<Standing> {
    let id = parse_id(&mod_id)?;
    blocking(&webview, move |app| {
        // The cached list, asked at the moment of the resume (the startup check runs only at launch).
        let view = View::read(app)?;
        let check = SafetyCheck::catalogue(view.index(), Utc::now(), Mode::Latest);
        let stood = with_existing(app, |ledger| {
            Ok(ledger.resume_by_user(&id, &check, Utc::now())?)
        })?;
        refresh::emit_changed(app, refresh::CHANGED_RESUMED);
        Ok(stood)
    })
    .await
}

/// The first-open "Turn on" of an auto-installed pilot (D17): record the user's answers as grants,
/// set `consented_at`, load the mod, and count as accepting the Mods disclaimer, all in one
/// transaction.
#[tauri::command]
pub async fn consent_mod(
    webview: Webview,
    mod_id: String,
    decisions: BTreeMap<String, bool>,
) -> Result<Standing> {
    let id = parse_id(&mod_id)?;
    blocking(&webview, move |app| {
        let view = View::read(app)?;
        let sealed = Host::engine_sealed(app);
        // The device id is a file read, done before the store is touched; when it can't be read the
        // mod still turns on and the disclaimer simply asks once more.
        let device = crate::paths::data_dir(app)
            .ok()
            .and_then(|d| device::device_id(&d).ok());
        let check = SafetyCheck::catalogue(view.index(), Utc::now(), Mode::Latest);
        let (stood, told) = with_existing(app, |ledger| {
            let facts = ledger
                .with_conn(|c| Ok(grants::read_mod(c, &id)?))?
                .ok_or_else(ModsError::not_installed)?;
            let plan = consent::plan(&facts.declared, facts.tier, &decisions)
                .map_err(|e| ModsError::bad_request(e.to_string()))?;
            let stood = ledger.consent(&id, &check, Utc::now(), |conn| {
                consent::apply(conn, &id, &plan, sealed)?;
                if let Some(device) = &device {
                    disclaimer::record(conn, device)
                        .map_err(|e| grants::GrantError::Db(e.to_string()))?;
                }
                Ok(())
            })?;
            let told = ledger.with_conn(|c| Ok(consent::announce(c, &id, plan.touched())?))?;
            Ok((stood, told))
        })?;
        announce(app, &told);
        refresh::emit_changed(app, refresh::CHANGED_CONSENTED);
        Ok(stood)
    })
    .await
}

/// One permission toggled on a mod's row (the user's yes or no, or a first-use prompt's answer).
/// A grant D3 or the unsealed engine forbids is refused and writes nothing.
///
/// `cap_usd` is the monthly AI limit the money prompt's field held, written in the same transaction
/// as a yes to `ai` (see [`consent::decide`]); it is refused with anything else. The Mods row's
/// plain Allow and Turn off send none.
#[tauri::command]
pub async fn mod_permission_decide(
    webview: Webview,
    mod_id: String,
    permission: String,
    granted: bool,
    cap_usd: Option<f64>,
) -> Result<()> {
    let id = parse_id(&mod_id)?;
    blocking(&webview, move |app| {
        let sealed = Host::engine_sealed(app);
        let told = {
            let state = app.state::<AppState>();
            let mut conn = state.conn().map_err(|_| ModsError::no_store())?;
            consent::decide(&mut conn, &id, &permission, granted, cap_usd, sealed).map_err(|e| {
                match e {
                    consent::DecideError::Grant(g) => ModsError::from(g),
                    consent::DecideError::Removed => ModsError::new(
                        Code::NotAllowed,
                        "PM's safety list removed this mod, so its permissions stay off.",
                    ),
                    consent::DecideError::Limit(l) => ModsError::from(l),
                }
            })?
        };
        announce(app, &told);
        refresh::emit_changed(app, refresh::CHANGED_DECIDED);
        Ok(())
    })
    .await
}

// ---- The AI's money (Mods spec §1.6) --------------------------------------------------------------

/// What the money prompt needs to say about the route a mod's AI call would take now: whether it
/// can cost money, which model and which kind of route, and the limit to start from. Facts about the
/// person's own settings; nothing the mod supplied. Refused for a mod that is not installed or never
/// asked for the AI. Reads only, and fetches no prices.
#[tauri::command]
pub async fn mod_ai_prompt_info(webview: Webview, mod_id: String) -> Result<PromptInfo> {
    let id = parse_id(&mod_id)?;
    blocking(&webview, move |app| {
        let state = app.state::<AppState>();
        let conn = state.conn().map_err(|_| ModsError::no_store())?;
        let facts = grants::read_mod(&conn, &id)
            .map_err(ModsError::internal)?
            .ok_or_else(ModsError::not_installed)?;
        if !facts.declared.contains("ai") {
            return Err(ModsError::bad_request("That mod didn't ask for the AI."));
        }
        Ok(())
    })
    .await?;
    // The DB guard is gone: the route's address lookup is a network wait.
    api::prompt_info(webview.app_handle())
        .await
        .map_err(|e| ModsError::internal(e.message))
}

/// Set (or, with none, take away) a mod's monthly AI limit from its row in the Mods tab. Raising it
/// is felt by the next call; lowering it below what is used refuses further paid calls; with none
/// set, the next paid call asks for one. Local models never use the limit.
#[tauri::command]
pub async fn mod_ai_cap_set(webview: Webview, mod_id: String, cap_usd: Option<f64>) -> Result<()> {
    let id = parse_id(&mod_id)?;
    blocking(&webview, move |app| {
        let state = app.state::<AppState>();
        let conn = state.conn().map_err(|_| ModsError::no_store())?;
        Ok(money::set_cap(&conn, id.as_str(), cap_usd)?)
    })
    .await?;
    refresh::emit_changed(webview.app_handle(), refresh::CHANGED_AI_CAP);
    Ok(())
}

// ---- Keys a mod's slots hold (Mods spec §1.7) --------------------------------------------------------
//
// The three commands below are the only way a key gets into `mod_secrets`, and none of them returns
// one: the value goes in, the page gets back a bool per slot. They are the PM-drawn field's
// backend (the Mods row's Details and the Developer tab's Test keys), so the typed text crosses the
// webview once, into Rust, and is never echoed, logged or kept (`mods::secrets`). A frame cannot
// call them: `mod_call` has no row that does.

/// Save the key typed for slot `name` of `mod_id` (an empty value deletes it): stored encrypted,
/// bound to the manifest slot's host and header. Answers the mod's slots, a bool each.
#[tauri::command]
pub async fn mod_secret_save(
    webview: Webview,
    mod_id: String,
    name: String,
    value: String,
) -> Result<Vec<secrets::SlotStatus>> {
    let id = parse_id(&mod_id)?;
    let slots = blocking(&webview, move |app| {
        let state = app.state::<AppState>();
        let mut conn = state.conn().map_err(|_| ModsError::no_store())?;
        secrets::save(&mut conn, id.as_str(), &name, &value)?;
        Ok(secrets::status(&conn, id.as_str())?)
    })
    .await?;
    refresh::emit_changed(webview.app_handle(), refresh::CHANGED_SECRET);
    Ok(slots)
}

/// Delete the key of slot `name`.
#[tauri::command]
pub async fn mod_secret_clear(webview: Webview, mod_id: String, name: String) -> Result<()> {
    let id = parse_id(&mod_id)?;
    blocking(&webview, move |app| {
        let state = app.state::<AppState>();
        let conn = state.conn().map_err(|_| ModsError::no_store())?;
        Ok(secrets::clear(&conn, id.as_str(), &name)?)
    })
    .await?;
    refresh::emit_changed(webview.app_handle(), refresh::CHANGED_SECRET);
    Ok(())
}

/// The mod's slots and whether each has a key now (never the key).
#[tauri::command]
pub async fn mod_secret_status(
    webview: Webview,
    mod_id: String,
) -> Result<Vec<secrets::SlotStatus>> {
    let id = parse_id(&mod_id)?;
    blocking(&webview, move |app| {
        let state = app.state::<AppState>();
        let conn = state.conn().map_err(|_| ModsError::no_store())?;
        Ok(secrets::status(&conn, id.as_str())?)
    })
    .await
}

/// Tell the frames of `mods` that their grants may have changed. After the commit, with no guard
/// held.
fn announce(app: &AppHandle, mods: &[ModId]) {
    if !mods.is_empty() {
        mod_events::grants_changed(app, mods);
    }
}

// ---- The disclaimer ---------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisclaimerState {
    /// This device has accepted it in this vault.
    pub accepted: bool,
    /// Why it reads as not accepted when the device couldn't be named (the data folder).
    pub problem: Option<String>,
}

/// Whether this device has accepted the Mods disclaimer in this vault (per device AND per vault).
#[tauri::command]
pub async fn mods_disclaimer_state(webview: Webview) -> Result<DisclaimerState> {
    blocking(&webview, |app| {
        let data_dir = crate::paths::data_dir(app).map_err(ModsError::internal)?;
        let device = match device::device_id(&data_dir) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("mods: this device couldn't be named ({e})");
                return Ok(DisclaimerState {
                    accepted: false,
                    problem: Some("PM couldn't read this device's record.".to_owned()),
                });
            }
        };
        let state = app.state::<AppState>();
        let conn = state.conn().map_err(|_| ModsError::no_store())?;
        Ok(DisclaimerState {
            accepted: disclaimer::is_accepted(&conn, &device).map_err(ModsError::internal)?,
            problem: None,
        })
    })
    .await
}

/// "I understand": record this device's acceptance in this vault.
#[tauri::command]
pub async fn accept_mods_disclaimer(webview: Webview) -> Result<()> {
    blocking(&webview, |app| {
        let data_dir = crate::paths::data_dir(app).map_err(ModsError::internal)?;
        let device = device::device_id(&data_dir).map_err(ModsError::internal)?;
        let state = app.state::<AppState>();
        let conn = state.conn().map_err(|_| ModsError::no_store())?;
        disclaimer::record(&conn, &device).map_err(ModsError::internal)
    })
    .await
}

// ---- The safety list ----------------------------------------------------------------------------------

/// When the safety list was last checked, whether this machine's record of it is damaged, and the
/// strips to draw.
#[tauri::command]
pub async fn mods_safety_status(webview: Webview) -> Result<SafetyStatus> {
    blocking(&webview, |app| {
        let view = View::read(app)?;
        let now = Utc::now();
        let state = app.state::<AppState>();
        let (rows, waiting, resumed) = {
            let conn = state.conn().map_err(|_| ModsError::no_store())?;
            listing::read_all(&conn).map_err(ModsError::internal)?
        };
        Ok(SafetyStatus {
            checked: status::checked(view.loaded.state.last_good_fetch, now),
            record: status::record_fault(refresh::fault_of(&view.loaded)),
            index: status::index_info(view.index(), now),
            notices: status::notices(&rows, view.index(), &waiting, &resumed),
        })
    })
    .await
}

/// Dismiss the "X is back on" strip for `mod_id`.
#[tauri::command]
pub async fn dismiss_mod_resumed(webview: Webview, mod_id: String) -> Result<()> {
    let id = parse_id(&mod_id)?;
    blocking(&webview, move |app| {
        {
            let state = app.state::<AppState>();
            let conn = state.conn().map_err(|_| ModsError::no_store())?;
            ledger::dismiss_resumed(&conn, id.as_str()).map_err(ModsError::internal)?;
        }
        refresh::emit_changed(app, refresh::CHANGED_DECIDED);
        Ok(())
    })
    .await
}

/// What a reset did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetResult {
    /// The damaged record was deleted.
    pub reset: bool,
    /// A fresh safety list was fetched and taken straight after.
    pub fetched: bool,
    /// When it wasn't, why, in words.
    pub problem: Option<String>,
}

/// The reset's answer from how the fetch after it went (pure).
pub fn reset_outcome(fetch: std::result::Result<(), &RefreshError>) -> ResetResult {
    let problem = match fetch {
        Ok(()) => None,
        Err(RefreshError::Fetch(_)) => Some(
            "PM deleted the damaged record but couldn't reach the safety list just now. It tries \
             again on its own."
                .to_owned(),
        ),
        Err(e) => Some(format!(
            "PM deleted the damaged record, but the new list wasn't taken: {e}."
        )),
    };
    ResetResult {
        reset: true,
        fetched: problem.is_none(),
        problem,
    }
}

/// Start the safety list over from a damaged record: delete `mods-index.json` and its backup, then
/// fetch. Refused unless the record really is unusable (a healthy record holds the seen seq and the
/// revoked keys, and is never reset). It forgets both until the next index names them, so ask first.
#[tauri::command]
pub async fn reset_safety_record(webview: Webview) -> Result<ResetResult> {
    blocking(&webview, |app| {
        let data_dir = crate::paths::data_dir(app).map_err(ModsError::internal)?;
        let did =
            index_state::reset_unusable(&data_dir, safety::ring()).map_err(ModsError::internal)?;
        if !did {
            return Err(ModsError::new(
                Code::NotAllowed,
                "PM's record of the safety list isn't damaged, so there is nothing to reset.",
            ));
        }
        Ok(())
    })
    .await?;
    let app = webview.app_handle().clone();
    let fetched = refresh::refresh_once(&app).await;
    let out = reset_outcome(fetched.as_ref().map(|_| ()));
    refresh::emit_changed(&app, refresh::CHANGED_DECIDED);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::fetch::FetchError;

    #[test]
    fn a_command_from_any_window_but_the_main_one_is_refused_first() {
        for label in [
            "calendar", "briefing", "", "Main", "main ", " main", "main\0", "mainx",
        ] {
            let e = preconditions(label, true, Some(false)).unwrap_err();
            assert_eq!(e.code, Code::NotMainWindow, "{label:?}");
            // Whatever else is wrong, the window is what is named.
            let e = preconditions(label, false, None).unwrap_err();
            assert_eq!(e.code, Code::NotMainWindow, "{label:?}");
        }
        assert!(preconditions(MAIN_LABEL, true, Some(false)).is_ok());
    }

    #[test]
    fn a_closed_store_or_an_unreadable_lock_is_no_store_and_a_lock_is_locked() {
        assert_eq!(
            preconditions(MAIN_LABEL, false, Some(false))
                .unwrap_err()
                .code,
            Code::NoStore
        );
        // The lock can't be read only while the store is closed.
        assert_eq!(
            preconditions(MAIN_LABEL, true, None).unwrap_err().code,
            Code::NoStore
        );
        assert_eq!(
            preconditions(MAIN_LABEL, true, Some(true))
                .unwrap_err()
                .code,
            Code::Locked
        );
        // A closed store is the more basic fact when both hold.
        assert_eq!(
            preconditions(MAIN_LABEL, false, Some(true))
                .unwrap_err()
                .code,
            Code::NoStore
        );
    }

    #[test]
    fn nothing_signed_installs_while_the_safety_lists_record_is_unusable() {
        // Whole, backed up, or not there yet: a first run installs.
        assert!(install_gate(&Loaded::default()).is_ok());
        let backed_up = Loaded {
            problem: Some("is damaged; its backup copy was used".into()),
            ..Loaded::default()
        };
        assert!(install_gate(&backed_up).is_ok());
        // Damaged with nothing standing in, or unreadable: the revoked keys are unknown.
        for unread in [false, true] {
            let broken = Loaded {
                fault: true,
                unread,
                problem: Some("is damaged".into()),
                ..Loaded::default()
            };
            let e = install_gate(&broken).unwrap_err();
            assert_eq!(e.code, Code::SafetyList);
            assert!(e.message.contains("record"), "{}", e.message);
        }
    }

    #[test]
    fn an_unsigned_file_or_folder_needs_the_cached_index_and_a_whole_record() {
        use crate::mods::index::tests::{good_index, verified_by, Keys};
        // A first run, or a machine that never fetched: the signed gate lets it through, this one
        // doesn't, since no catalogue id is known to refuse.
        assert!(install_gate(&Loaded::default()).is_ok());
        let e = unsigned_gate(&Loaded::default()).unwrap_err();
        assert_eq!(e.code, Code::SafetyList);
        assert!(e.message.contains("safety list"), "{}", e.message);
        // A broken record is refused as before.
        let broken = Loaded {
            fault: true,
            problem: Some("is damaged".into()),
            ..Loaded::default()
        };
        assert_eq!(unsigned_gate(&broken).unwrap_err().code, Code::SafetyList);
        // With the index, it goes ahead.
        let keys = Keys::new();
        let with_index = Loaded {
            index: Some(verified_by(&good_index(5), &keys.main, &keys)),
            ..Loaded::default()
        };
        assert!(unsigned_gate(&with_index).is_ok());
    }

    #[test]
    fn uninstalling_from_the_mods_tab_drops_a_test_folders_file_watches() {
        let (_, body) = command_bodies()
            .into_iter()
            .find(|(name, _)| name == "uninstall_mod")
            .unwrap();
        assert!(body.contains("folder_watch::resync(app)"));
        // After the ledger's uninstall, not before: the session is what the resync reads.
        assert!(body.find("uninstall::run").unwrap() < body.find("folder_watch::resync").unwrap());
    }

    #[test]
    fn the_unsigned_and_folder_paths_use_the_index_gate_not_the_signed_one() {
        for (name, src) in [
            ("dev_commands", include_str!("dev_commands.rs")),
            ("folder", include_str!("folder.rs")),
        ] {
            let production = src.split("#[cfg(test)]\nmod tests").next().unwrap();
            assert!(production.contains("view.for_unsigned()?"), "{name}");
            assert!(!production.contains("view.for_install()?"), "{name}");
        }
    }

    #[test]
    fn a_reset_says_whether_the_fresh_list_came_and_why_not() {
        let ok = reset_outcome(Ok(()));
        assert_eq!((ok.reset, ok.fetched, ok.problem), (true, true, None));
        let offline = RefreshError::Fetch(FetchError::Offline("no route".into()));
        let r = reset_outcome(Err(&offline));
        assert!(r.reset && !r.fetched);
        assert!(r.problem.unwrap().contains("tries again"));
        let refused = RefreshError::Refused("the fetched safety list was refused".into());
        let r = reset_outcome(Err(&refused));
        assert!(r.reset && !r.fetched);
        assert!(r.problem.unwrap().contains("wasn't taken"));
    }

    /// The commands' source, split at each `#[tauri::command]`: the attribute's function and its body.
    fn command_bodies() -> Vec<(String, String)> {
        let source = include_str!("commands.rs");
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
    fn every_command_passes_the_window_and_store_gate_before_it_does_anything() {
        let commands = command_bodies();
        assert!(commands.len() >= 19, "found {}", commands.len());
        for (name, body) in &commands {
            // A command reaches the gate through `blocking`, or through the shared flow that does.
            assert!(
                body.contains("blocking(&webview") || body.contains("catalogue_screen(&webview"),
                "{name} doesn't go through the gate"
            );
            assert!(
                !name.starts_with("set_"),
                "{name}: the Settings tick keys on `set_`"
            );
            assert!(
                body.contains("webview: Webview"),
                "{name} must see its window"
            );
        }
    }

    #[test]
    fn every_command_is_registered_and_none_is_left_out() {
        let lib = include_str!("../../lib.rs");
        for (name, _) in command_bodies() {
            assert!(
                lib.contains(&format!("mods::tab::commands::{name},")),
                "{name} isn't in generate_handler!"
            );
        }
        let registered = lib.matches("mods::tab::commands::").count();
        assert_eq!(registered, command_bodies().len());
    }

    /// A consent token is bound to the store the screen was judged against, so every function that
    /// stages one reads the store's epoch before its first read of anything the screen shows. Read
    /// after, a vault switch during the build stamps store A's screen with store B's epoch and
    /// `Stage::take` accepts it. (The pure half, that `take` refuses an older epoch, is in stage.rs.)
    #[test]
    fn every_consent_screen_takes_its_stores_epoch_before_it_reads_anything() {
        let mut stagers = 0;
        for source in [
            include_str!("commands.rs"),
            include_str!("dev_commands.rs"),
            include_str!("folder.rs"),
        ] {
            let production = source.split("#[cfg(test)]\nmod tests").next().unwrap();
            // Top-level items end with a brace in column 0.
            for item in production.split("\n}\n") {
                if !item.contains("with_token(app, epoch, ") || item.contains("fn with_token") {
                    continue;
                }
                stagers += 1;
                let epoch = item
                    .find("let epoch = current_epoch(app);")
                    .unwrap_or_else(|| panic!("no epoch read in:\n{item}"));
                let first_read = item.find("View::read(app)").unwrap();
                assert!(
                    epoch < first_read,
                    "the epoch is read after the view in:\n{item}"
                );
            }
        }
        // install_mod_file, catalogue_screen (both catalogue commands), install_unsigned_file and
        // load_test_mod.
        assert_eq!(stagers, 4);
    }

    #[test]
    fn the_ids_a_page_sends_are_parsed_before_anything_runs() {
        assert!(parse_id("atlas.map").is_ok());
        for bad in ["", "A", "../x", "a/b", "atlas map", "x"] {
            assert_eq!(parse_id(bad).unwrap_err().code, Code::BadRequest, "{bad:?}");
        }
    }
}
