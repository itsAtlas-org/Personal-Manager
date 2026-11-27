// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Google accounts for Contacts (PR B, spec 7.1 and 7.2): link an account, ask for the sending
//! consent, pause sending, switch to Copy once, Stop syncing and Forget this account's links. Thin
//! commands over [`crate::contacts_accounts`], which owns the rules; nothing here reads or writes a
//! contact. The sync itself comes in later slices.
//!
//! Every command belongs to the main window (the Undo strips and confirmations are there), and
//! every one that signs in, signs out or forgets is the vault owner's, like every connector change.
//! The DB guard is never held across an `.await`, and a token is only ever saved under its key's
//! refresh lock. No command starts with `set_`. Errors are fixed words: no token, address or contact
//! value, and no body Google sent, ever reaches one (the one address named is the user's own
//! account, as Calendar's "you chose X but signed in as Y" names it).

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::calendar_editing::{Choice, ConsentVerdict};
use crate::contacts_accounts::{self, AccountsRead, ContactsAccount, SyncMode};
use crate::error::{Error, Result};
use crate::people::{self, sources::ForgottenLinks};
use crate::{google, oauth_loopback, secrets, AppState};

use super::shared::{
    forget_write_grants, google_grant_token_keys, google_grant_users, own_client,
    pin_existing_grants, release_plan, require_people_main_window, wrong_account, GoogleDisconnect,
    GoogleUse, GrantRelease,
};
use super::vaults::require_vault_owner;

/// The accounts Contacts knows, each with how it is used and whether PM may send for it: those with
/// a sign-in, those restored without one ("Reconnect") and those that are only kept links ("Not
/// syncing"). Reads the keychain for what each token holds and no network.
#[tauri::command]
pub async fn contacts_google_status(
    window: tauri::Window,
    state: State<'_, AppState>,
) -> Result<Vec<ContactsAccount>> {
    require_people_main_window(&window)?;
    // The store under a short guard, the keychain after it is released: a keychain can wait on an
    // unlock prompt, and the guard would hold the whole app behind it.
    let read = {
        let conn = state.conn()?;
        AccountsRead::read(&conn)?
    };
    Ok(read.all(contacts_accounts::stored_token_scope))
}

/// What a link did.
#[derive(Debug, Serialize)]
pub struct LinkedContacts {
    pub account: ContactsAccount,
    /// The account had sending on or paused before this link, which asked for reading only, so
    /// sending is off now and the page says so ("Connecting again gave PM reading only…").
    pub sending_turned_off: bool,
}

/// What Forget this account's links did: the sign-in it let go of, and the links it deleted.
#[derive(Debug, Serialize)]
pub struct ForgotLinks {
    pub disconnect: GoogleDisconnect,
    pub forgotten: ForgottenLinks,
}

/// One account as the status list shows it, from the store half read under the guard (the caller
/// drops the guard first: this reads the keychain).
fn account_from(read: &AccountsRead, email: &str) -> Result<ContactsAccount> {
    read.one(email, contacts_accounts::stored_token_scope)
        .ok_or_else(|| Error::Other("PM couldn't read the account back after saving it.".into()))
}

/// The store changed (another vault opened) while Google's page was open: nothing may be saved for
/// the store the user started in, so the sign-in is dropped in fixed words.
fn require_same_store(state: &AppState, epoch: u64) -> Result<()> {
    if state.store_epoch() == epoch {
        Ok(())
    } else {
        Err(Error::Other(
            "The vault changed while Google was asking, so PM didn't keep the sign-in.".into(),
        ))
    }
}

/// Link a Google account for Contacts, in the mode the user chose (Copy once or Keep in sync).
///
/// Asks Google for reading only (`contacts.readonly` and the address scope), through Google's own
/// account chooser, and classifies the consent before anything is saved: the read box unticked saves
/// nothing, and another account than `email` (when the user is reconnecting a known one) is refused.
/// The account's address is read with `people/me` and no `sources`, which is also how a first link,
/// which expects nobody, learns who signed in. `client_id` and `client_secret` sign in through the
/// account's own Cloud project (an Advanced-Protection account), as Calendar's connect does.
///
/// A read-only link turns sending off even when the account had it: only the sending consent
/// ([`ask_contacts_sending_consent`]) turns it on. The people in PM and their links to Google are
/// never touched, so linking an account again matches by Google's id and creates nobody twice.
#[tauri::command]
pub async fn link_google_contacts(
    window: tauri::Window,
    app: AppHandle,
    email: Option<String>,
    mode: SyncMode,
    client_id: Option<String>,
    client_secret: Option<String>,
) -> Result<LinkedContacts> {
    require_people_main_window(&window)?;
    require_vault_owner(&app)?;
    let own = own_client(client_id, client_secret)?;
    let expected = email
        .map(|e| contacts_accounts::key_of(&e))
        .filter(|e| !e.is_empty());
    let state = app.state::<AppState>();
    // The store the user linked from: Google's page can stay open for minutes.
    let epoch = state.store_epoch();
    let scopes = google::contacts_link_scopes();
    let token = match (&own, &expected) {
        (Some((id, secret)), _) => {
            google::run_consent_with_client(
                &app,
                &scopes,
                "Google Contacts",
                id.clone(),
                secret.clone(),
            )
            .await?
        }
        // Reconnecting a known account: through the client that minted its token, starting Google's
        // chooser on it.
        (None, Some(known)) => {
            google::run_consent_for_key(
                &app,
                &contacts_accounts::token_key(known),
                known,
                &scopes,
                "Google Contacts",
            )
            .await?
        }
        (None, None) => google::run_consent(&app, &scopes, "Google Contacts").await?,
    };
    let granted = token.scope.clone().unwrap_or_default();
    // Which account signed in is read through `people/me`, so only with the read scope.
    let signed_in_as = if google::scope_set_has(&granted, google::CONTACTS_READONLY_SCOPE) {
        Some(contacts_accounts::fetch_signed_in_address(&token).await?)
    } else {
        None
    };
    match contacts_accounts::classify_consent(
        expected.as_deref(),
        &granted,
        signed_in_as.as_deref(),
    ) {
        ConsentVerdict::ReadonlyMissing => return Err(read_unticked()),
        ConsentVerdict::WrongAccount { signed_in_as } => {
            return Err(wrong_account(
                expected.as_deref().unwrap_or_default(),
                &signed_in_as,
            ))
        }
        // A link never asks for the write scope, so both mean the right account read-granted.
        ConsentVerdict::Enabled | ConsentVerdict::WriteDeclined => {}
    }
    let Some(email) = signed_in_as else {
        return Err(read_unticked());
    };
    // Before any keychain write: no token or client is kept for a store that is gone.
    require_same_store(&state, epoch)?;
    if let Some((id, secret)) = &own {
        let keys = google_grant_token_keys(&*state.conn()?, &email)?;
        pin_existing_grants(&keys).await?;
        secrets::set_google_client_for_account(&email, id, secret)?;
    }
    google::save_consented_token(&contacts_accounts::token_key(&email), &token).await?;
    let (read, sending_turned_off) = {
        let conn = state.conn()?;
        // Checked again under the guard: the save above awaited.
        require_same_store(&state, epoch)?;
        contacts_accounts::upsert_source(&conn, &email)?;
        contacts_accounts::set_sync_mode(&conn, &email, Some(mode))?;
        // A read-only consent can't carry sending along: the choice goes, the grant on record stays.
        let had_sending = contacts_accounts::writing_choice(&conn, &email)?.is_some();
        contacts_accounts::set_writing_choice(&conn, &email, None)?;
        (AccountsRead::read(&conn)?, had_sending)
    };
    let account = account_from(&read, &email)?;
    start_first_sync(&app, &email, mode);
    Ok(LinkedContacts {
        account,
        sending_turned_off,
    })
}

/// The read box was unticked on Google's screen.
fn read_unticked() -> Error {
    Error::Other(
        "Google didn't give PM permission to see your contacts, so nothing changed. Try again and \
         leave the box ticked: PM needs to see your contacts to copy them into People."
            .into(),
    )
}

/// **The seam for the read path.** A freshly linked account is owed its first full sync: for Keep in
/// sync it runs and the sign-in stays; for Copy once it runs and then the sign-in is let go
/// ([`release_sign_in`] with `Some(SyncMode::Once)`), so the links stay and the access doesn't. The
/// read path starts that here; this slice builds the account and its settings only, so a link
/// copies nobody yet.
fn start_first_sync(_app: &AppHandle, _email: &str, _mode: SyncMode) {}

/// Ask for the sending consent: a second sign-in for `contacts.readonly contacts`, stated in full,
/// through the client that minted the account's token, starting Google's chooser on that account.
/// Only Keep in sync accounts can send. Classified before anything is saved: another account, or the
/// read box unticked, changes nothing; the write box unticked keeps the fresh read-only token and
/// leaves sending off. Returns the account as it stands.
#[tauri::command]
pub async fn ask_contacts_sending_consent(
    window: tauri::Window,
    app: AppHandle,
    email: String,
) -> Result<ContactsAccount> {
    require_people_main_window(&window)?;
    require_vault_owner(&app)?;
    let email = contacts_accounts::key_of(&email);
    let state = app.state::<AppState>();
    let epoch = state.store_epoch();
    let live = |conn: &rusqlite::Connection| -> Result<bool> {
        Ok(contacts_accounts::is_linked(conn, &email)?
            && contacts_accounts::sync_mode(conn, &email)? == Some(SyncMode::Live))
    };
    if !live(&*state.conn()?)? {
        return Err(Error::Other(
            "Sending your changes to Google works with Keep in sync. Link this Google account \
             with Keep in sync first."
                .into(),
        ));
    }
    let token_key = contacts_accounts::token_key(&email);
    let token = google::run_consent_for_key(
        &app,
        &token_key,
        &email,
        &google::contacts_sending_scopes(),
        "Google Contacts sending",
    )
    .await?;
    let granted = token.scope.clone().unwrap_or_default();
    let signed_in_as = if google::scope_set_has(&granted, google::CONTACTS_READONLY_SCOPE) {
        Some(contacts_accounts::fetch_signed_in_address(&token).await?)
    } else {
        None
    };
    let verdict =
        contacts_accounts::classify_consent(Some(&email), &granted, signed_in_as.as_deref());
    match &verdict {
        ConsentVerdict::ReadonlyMissing => return Err(read_unticked()),
        ConsentVerdict::WrongAccount { signed_in_as } => {
            return Err(wrong_account(&email, signed_in_as))
        }
        ConsentVerdict::Enabled | ConsentVerdict::WriteDeclined => {}
    }
    // Check, save and record as one step: the key's refresh lock (so a refresh in flight can't save
    // the old token over this one), then the DB guard, which Stop syncing holds across its own
    // delete. Lock order matches `release_sign_in`: refresh lock, then DB.
    let _refresh = oauth_loopback::refresh_lock(&token_key).await;
    let read = {
        let conn = state.conn()?;
        // Another vault opened while Google's page was open: this store never linked the token.
        require_same_store(&state, epoch)?;
        // Stopped, or switched to Copy once, while Google's page was open: saving now would leave a
        // token (and a write grant) nobody asked to keep.
        if !live(&conn)? {
            return Err(Error::Other(
                "This account was stopped while Google was asking, so PM didn't keep the sign-in."
                    .into(),
            ));
        }
        google::save_token(&token_key, &token)?;
        if verdict == ConsentVerdict::Enabled {
            contacts_accounts::set_writing_choice(&conn, &email, Some(Choice::On))?;
            contacts_accounts::mark_write_granted(&conn, &email)?;
        } else {
            contacts_accounts::set_writing_choice(&conn, &email, None)?;
        }
        AccountsRead::read(&conn)?
    };
    // The read-back asks the keychain, so it waits for the guard to go.
    account_from(&read, &email)
}

/// Switch sending off (`paused`) or back on for an account that has it turned on, without asking
/// Google: the permission stays granted, PM just stops using it. Returns the account as it stands.
#[tauri::command]
pub async fn pause_contacts_sending(
    window: tauri::Window,
    app: AppHandle,
    state: State<'_, AppState>,
    email: String,
    paused: bool,
) -> Result<ContactsAccount> {
    require_people_main_window(&window)?;
    require_vault_owner(&app)?;
    let choice = if paused { Choice::Paused } else { Choice::On };
    let read = {
        let conn = state.conn()?;
        if contacts_accounts::writing_choice(&conn, &email)?.is_none() {
            return Err(Error::Other(
                "Sending isn't turned on for this account.".into(),
            ));
        }
        contacts_accounts::set_writing_choice(&conn, &email, Some(choice))?;
        AccountsRead::read(&conn)?
    };
    account_from(&read, &email)
}

/// Let go of an account's Contacts sign-in: revoke it at Google only when Contacts was the grant's
/// last user ([`release_plan`]), delete the token, forget the account's own Cloud project when
/// nothing else uses it, drop the Contacts row, and settle the settings
/// ([`contacts_accounts::settle_after_sign_out`]: the mode becomes `leave_mode`, sending reads Off).
/// The people and their links are never touched. Shared by Stop syncing, switching to Copy once,
/// Forget links and, in the read path, a Copy once that has finished copying.
pub(crate) async fn release_sign_in(
    app: &AppHandle,
    email: &str,
    leave_mode: Option<SyncMode>,
) -> Result<GoogleDisconnect> {
    let state = app.state::<AppState>();
    let email = contacts_accounts::key_of(email);
    let token_key = contacts_accounts::token_key(&email);
    // Read the account's users under a short lock, released before the revoke's await.
    let plan = {
        let conn = state.conn()?;
        release_plan(&google_grant_users(&conn, &email)?, GoogleUse::Contacts)
    };
    // Sever the grant at Google's end BEFORE forgetting the local token (best-effort, like the other
    // disconnects), and only as the account's last user: a revoke ends the grant for every feature.
    if plan.revoke {
        if let Ok(Some(blob)) = secrets::get_google_token_for(&token_key) {
            let _ = google::revoke(blob.expose()).await;
        }
    }
    // The key's refresh lock, taken before the DB guard and held through the delete, so a refresh
    // already in flight can't save its token back after it.
    let _refresh = oauth_loopback::refresh_lock(&token_key).await;
    let conn = state.conn()?;
    // The token first, and a real keychain failure stops here with the row still listed: dropping the
    // row over a token that wouldn't clear would orphan a live sign-in with nothing left to clear it.
    secrets::clear_google_token_for(&token_key)?;
    if plan.forget_account_client {
        secrets::clear_google_client_for_account(&email)?;
    }
    settle_store(&conn, &email, &plan, leave_mode)?;
    Ok(GoogleDisconnect {
        kept_for: plan.kept_for,
        calendar_write: false,
    })
}

/// The store half of [`release_sign_in`]: the row goes, the settings settle, and a revoke ends the
/// write grants with the rest. Pure over the store, so it is tested without a keychain.
fn settle_store(
    conn: &rusqlite::Connection,
    email: &str,
    plan: &GrantRelease,
    leave_mode: Option<SyncMode>,
) -> Result<()> {
    contacts_accounts::remove_source(conn, email)?;
    contacts_accounts::settle_after_sign_out(conn, email, leave_mode)?;
    if plan.revoke {
        forget_write_grants(conn, email)?;
    }
    Ok(())
}

/// Switch an account from Keep in sync to Copy once: the sign-in is let go (revoked only if Contacts
/// was the grant's last user), sending reads Off, and every link stays. Linking again, with Keep in
/// sync, signs in and matches everyone by Google's id.
#[tauri::command]
pub async fn switch_contacts_to_copy_once(
    window: tauri::Window,
    app: AppHandle,
    email: String,
) -> Result<GoogleDisconnect> {
    require_people_main_window(&window)?;
    require_vault_owner(&app)?;
    let state = app.state::<AppState>();
    let live = contacts_accounts::sync_mode(&*state.conn()?, &email)? == Some(SyncMode::Live);
    if !live {
        return Err(Error::Other(
            "Only an account that is kept in sync can be switched to Copy once.".into(),
        ));
    }
    release_sign_in(&app, &email, Some(SyncMode::Once)).await
}

/// Stop syncing an account: the sign-in is let go (revoked only if Contacts was the grant's last
/// user), sending reads Off, and every link is **kept**, shown "Not syncing", so linking again
/// matches by Google's id, tombstones included, and nobody comes back twice.
#[tauri::command]
pub async fn stop_contacts_syncing(
    window: tauri::Window,
    app: AppHandle,
    email: String,
) -> Result<GoogleDisconnect> {
    require_people_main_window(&window)?;
    require_vault_owner(&app)?;
    release_sign_in(&app, &email, None).await
}

/// Forget this account's links. It stops syncing first (so the next pass can't list every contact
/// again and bring each one back as a new person), then deletes the account's `person_sources` and
/// `label_sources`, tombstones and waiting Google deletes included. People, labels and everything
/// else of theirs are never touched, and a waiting delete is dropped, not sent.
#[tauri::command]
pub async fn forget_contacts_links(
    window: tauri::Window,
    app: AppHandle,
    email: String,
) -> Result<ForgotLinks> {
    require_people_main_window(&window)?;
    require_vault_owner(&app)?;
    let email = contacts_accounts::key_of(&email);
    let disconnect = release_sign_in(&app, &email, None).await?;
    let forgotten = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        people::sources::forget_account_links(&conn, &email)?
    };
    people::announce_changed(&app);
    Ok(ForgotLinks {
        disconnect,
        forgotten,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar_editing;

    /// Every Contacts account command and its tier. Each is the main window's: the Undo strips and
    /// confirmations are there, and the briefing window and the pop-out Calendar have no reason to
    /// sign an account in or out. `true` means the vault owner's too.
    const TIERS: &[(&str, bool)] = &[
        ("contacts_google_status", false),
        ("link_google_contacts", true),
        ("ask_contacts_sending_consent", true),
        ("pause_contacts_sending", true),
        ("switch_contacts_to_copy_once", true),
        ("stop_contacts_syncing", true),
        ("forget_contacts_links", true),
    ];

    /// The shipped part of this file, and each command's body in it (name, body).
    fn commands() -> Vec<(String, String)> {
        let source = include_str!("contacts_google.rs");
        let shipped = source
            .split("#[cfg(test)]\nmod tests {")
            .next()
            .unwrap_or_default();
        shipped
            .split("#[tauri::command]\n")
            .skip(1)
            .map(|chunk| {
                let name = chunk
                    .strip_prefix("pub async fn ")
                    .or_else(|| chunk.strip_prefix("pub fn "))
                    .and_then(|rest| rest.split('(').next())
                    .expect("a command is `pub fn name(` or `pub async fn name(`")
                    .to_owned();
                // Comment lines dropped, so a guard named only in a comment proves nothing.
                let body = chunk
                    .split("\n}\n")
                    .next()
                    .unwrap_or_default()
                    .lines()
                    .filter(|line| !line.trim_start().starts_with("//"))
                    .collect::<Vec<_>>()
                    .join("\n");
                (name, body)
            })
            .collect()
    }

    /// Each command opens with the main-window guard, before it touches the store, a token or the
    /// network; every one that signs in or out then checks the vault owner. A command with the wrong
    /// guard, or none, is how the pop-out Calendar or the briefing window ends up linking an account.
    #[test]
    fn every_command_checks_the_main_window_first() {
        let found = commands();
        let mut names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
        let mut table: Vec<&str> = TIERS.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        table.sort_unstable();
        assert_eq!(names, table, "the tier table and the commands disagree");
        for (name, body) in &found {
            let owner = TIERS.iter().find(|(n, _)| n == name).expect("listed").1;
            let guard = body
                .find("require_people_main_window(&window)?;")
                .unwrap_or(usize::MAX);
            for later in [
                "state.conn()?",
                "app.state::<AppState>()",
                "require_vault_owner",
                "google::run_consent",
                "release_sign_in(",
            ] {
                if let Some(at) = body.find(later) {
                    assert!(guard < at, "{name}: its window guard comes before {later}");
                }
            }
            assert!(guard != usize::MAX, "{name}: no window guard");
            assert_eq!(
                body.contains("require_vault_owner(&app)?;"),
                owner,
                "{name}: the owner check"
            );
            for wrong in [
                "require_people_window",
                "require_calendar_window",
                "caller_may_write",
            ] {
                assert!(!body.contains(wrong), "{name}: it names {wrong}");
            }
            assert!(
                !name.starts_with("set_"),
                "{name}: no command starts with `set_`"
            );
            // Nothing here reads or writes a contact.
            assert!(!body.contains("people::create_person") && !body.contains("update_person"));
        }
    }

    /// The commands that wait on Google or its browser never hold the DB guard across the wait: a
    /// `conn` bound before an `.await` must be inside its own block. Checked as text: no command binds
    /// a `let conn` before a consent call in the same body.
    #[test]
    fn no_command_holds_the_store_across_google() {
        for (name, body) in commands() {
            let Some(waits) = body.find("google::run_consent") else {
                continue;
            };
            let held = body[..waits].contains("let conn = state.conn()?;");
            assert!(
                !held,
                "{name}: the store is held while Google's page is open"
            );
        }
    }

    /// Stop syncing and switching to Copy once let go of the sign-in and settle the settings; every
    /// link stays, and a revoke (the last user leaving) also ends the write grants.
    #[test]
    fn letting_go_of_a_sign_in_keeps_the_links_and_settles_the_settings() {
        use crate::calendar_editing::Choice;
        let (_dir, conn) = super::super::shared::temp_db();
        let person = people::create_person(
            &conn,
            &people::PersonDraft {
                given_name: "Sam".into(),
                family_name: "Example".into(),
                ..Default::default()
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO person_sources(person_id, provider, account, external_id) \
             VALUES (?1, 'google', 'me@example.com', 'people/c1')",
            rusqlite::params![person],
        )
        .unwrap();
        let links = |conn: &rusqlite::Connection| -> i64 {
            conn.query_row("SELECT COUNT(*) FROM person_sources", [], |r| r.get(0))
                .unwrap()
        };
        let keep = GrantRelease {
            revoke: false,
            forget_account_client: false,
            keep_own_token: false,
            kept_for: vec![GoogleUse::Calendar],
        };
        let revoke = GrantRelease {
            revoke: true,
            forget_account_client: true,
            keep_own_token: false,
            kept_for: vec![],
        };
        for (plan, leave, grant_after) in [
            (&keep, None, true),
            (&keep, Some(SyncMode::Once), true),
            (&revoke, None, false),
        ] {
            contacts_accounts::upsert_source(&conn, "me@example.com").unwrap();
            contacts_accounts::set_sync_mode(&conn, "me@example.com", Some(SyncMode::Live))
                .unwrap();
            contacts_accounts::set_writing_choice(&conn, "me@example.com", Some(Choice::On))
                .unwrap();
            contacts_accounts::mark_write_granted(&conn, "me@example.com").unwrap();
            calendar_editing::mark_write_granted(&conn, "me@example.com").unwrap();

            settle_store(&conn, "me@example.com", plan, leave).unwrap();

            assert!(!contacts_accounts::is_linked(&conn, "me@example.com").unwrap());
            assert_eq!(
                contacts_accounts::sync_mode(&conn, "me@example.com").unwrap(),
                leave
            );
            assert_eq!(
                contacts_accounts::writing_choice(&conn, "me@example.com").unwrap(),
                None
            );
            // The grant stays on record unless a revoke ended it, Calendar's with it.
            assert_eq!(
                contacts_accounts::write_granted(&conn, "me@example.com").unwrap(),
                grant_after
            );
            assert_eq!(
                calendar_editing::write_granted(&conn, "me@example.com").unwrap(),
                grant_after
            );
            // Nobody and nothing of theirs is touched.
            assert_eq!(links(&conn), 1);
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM people", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
    }

    /// The words the page reads.
    #[test]
    fn a_link_and_a_forget_serialize_to_the_shapes_the_page_names() {
        let account = ContactsAccount {
            email: "me@example.com".into(),
            mode: Some(SyncMode::Once),
            signed_in: true,
            needs_reconnect: false,
            sending: contacts_accounts::SendingStatus::Off,
            links: 3,
            label_links: 1,
        };
        let json = serde_json::to_value(LinkedContacts {
            account,
            sending_turned_off: true,
        })
        .unwrap();
        assert_eq!(json["sending_turned_off"], true);
        assert_eq!(json["account"]["email"], "me@example.com");
        assert_eq!(json["account"]["mode"], "once");
        assert_eq!(json["account"]["sending"], "off");
        assert_eq!(json["account"]["links"], 3);
        let forgot = serde_json::to_value(ForgotLinks {
            disconnect: GoogleDisconnect {
                kept_for: vec![GoogleUse::Calendar, GoogleUse::Contacts],
                calendar_write: false,
            },
            forgotten: ForgottenLinks {
                people: 4,
                labels: 1,
            },
        })
        .unwrap();
        assert_eq!(forgot["disconnect"]["kept_for"][1], "contacts");
        assert_eq!(forgot["forgotten"]["people"], 4);
        assert_eq!(forgot["forgotten"]["labels"], 1);
        // The mode arrives as a bare word, and only the two words.
        assert_eq!(
            serde_json::from_str::<SyncMode>("\"live\"").unwrap(),
            SyncMode::Live
        );
        assert!(serde_json::from_str::<SyncMode>("\"sometimes\"").is_err());
    }
}
