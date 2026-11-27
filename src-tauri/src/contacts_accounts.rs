// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Google accounts for Contacts (spec 7.1 and 7.2): which accounts PM is linked to, how each is
//! used, the consent that lets PM send changes, and the call that learns which account signed in.
//!
//! Each linked account is a separate service, not a widened Calendar token: a `connector_sources`
//! row `gcontacts:<email>` (provider `google`, service `contacts`) and a keychain token
//! `google_oauth_token_contacts::<email>`. Three `settings` keys hold the user's choices, copying
//! [`crate::calendar_editing`]'s shape (one JSON object per key, lowercased address as the key):
//! - [`SYNC_KEY`], `{ email: "once" | "live" }`: Copy once, or Keep in sync;
//! - [`WRITING_KEY`], `{ email: "on" | "paused" }`: whether PM sends the user's changes;
//! - [`WRITE_GRANTED_KEY`], `[email]`: the accounts that ever granted the write scope, which outlives
//!   the choice.
//!
//! Sending is **on** only while three things hold, as calendar editing's rule: the user turned it on
//! (the choice), the account's token carries Google's `contacts` scope, and the user hasn't paused it.
//! The choice is the gate, not the scope: a scope alone never turns sending on, and switching to Copy
//! once or stopping removes the choice.
//!
//! Nothing here reads or writes a contact. The one call it makes is `people/me`, which names the
//! account, and it answers in words of its own: a Google reply's body never reaches an error (a body
//! can hold a person's address), with one exception, the link a disabled API's 403 names for turning
//! it on, which holds no contact data.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::calendar_editing::{self, Choice, ConsentVerdict, EditingStatus};
use crate::error::{Error, Result};
use crate::google;
use crate::secrets;

/// `connector_sources.provider` of a Contacts account, and the `provider` of its `person_sources` and
/// `label_sources` rows: the one vocabulary [`crate::people::PROVIDERS`] owns.
pub const PROVIDER: &str = crate::people::PROVIDER_GOOGLE;
/// `connector_sources.service` of a Contacts account (no CHECK on the column, so no migration).
pub const SERVICE: &str = "contacts";
/// The id prefix of a Contacts connector row: `gcontacts:<email>`. A one-way door, like the
/// keychain prefix in `secrets` and the `clientData` key below: a stored id or token that no longer
/// matches is an account PM can't find (a table test pins all of them).
pub const SOURCE_ID_PREFIX: &str = "gcontacts:";
/// The `clientData` key PM puts on a Google contact it creates, holding the person's random id, so a
/// create that was lost on the way can be found again. A one-way door: contacts already in a
/// Google account carry it.
// Named here so the one-way-door table pins it with the others; the request plans (the pure core
// and the write path) put it on a created contact.
#[allow(dead_code)]
pub const PM_UID_CLIENT_DATA_KEY: &str = "pm.uid";

/// `settings` key: `{ "<lowercased email>": "once" | "live" }`. An account that isn't listed is not
/// synced ("Not syncing"), whatever links it keeps.
pub const SYNC_KEY: &str = "contacts_sync";
/// `settings` key: `{ "<lowercased email>": "on" | "paused" }`. An account that isn't listed has
/// sending off.
pub const WRITING_KEY: &str = "contacts_writing";
/// `settings` key: the (lowercased) accounts that granted PM the Contacts write scope, as a JSON
/// list. Separate from the choice, because the grant outlives it: Stop syncing, Copy once and a
/// read-only reconnect leave Google's grant as it was, and only a revoke ends it.
pub const WRITE_GRANTED_KEY: &str = "contacts_write_granted";

/// How an account is used, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    /// Copy once: sign in, copy, keep each person's link, then forget the sign-in.
    Once,
    /// Keep in sync: the sign-in stays and PM checks Google on a schedule.
    Live,
}

/// Whether PM may send changes to an account's Google Contacts. The same four answers as calendar
/// editing's status, so the one rule serves both.
pub type SendingStatus = EditingStatus;

/// The address every account is stored under: trimmed and lowercased, as Calendar's are.
pub fn key_of(email: &str) -> String {
    calendar_editing::account_key(email)
}

// --- the connector row and the keychain token ---

/// The `connector_sources.id` of one account's Contacts row.
pub fn source_id(email: &str) -> String {
    format!("{SOURCE_ID_PREFIX}{}", key_of(email))
}

/// The keychain key of one account's Contacts token.
pub fn token_key(email: &str) -> String {
    secrets::token_key_for(PROVIDER, SERVICE, &key_of(email))
        .expect("google/contacts is a token-bearing pair")
}

/// Register (or refresh) the Contacts row of an account; resets its state to `ok`.
pub fn upsert_source(conn: &Connection, email: &str) -> Result<()> {
    let email = key_of(email);
    conn.execute(
        "INSERT INTO connector_sources(id, provider, service, label, account_email) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(id) DO UPDATE SET label = excluded.label, \
             account_email = excluded.account_email, state = 'ok'",
        params![source_id(&email), PROVIDER, SERVICE, email, email],
    )?;
    Ok(())
}

/// The accounts with a Contacts row (signed in, or restored from a backup without their sign-in),
/// in the order they were linked.
pub fn linked_emails(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT account_email FROM connector_sources \
          WHERE provider = ?1 AND service = ?2 AND account_email IS NOT NULL ORDER BY created_at",
    )?;
    let rows = stmt
        .query_map(params![PROVIDER, SERVICE], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows)
}

/// Remove an account's Contacts row. Its people and their links stay.
pub fn remove_source(conn: &Connection, email: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM connector_sources WHERE id = ?1 AND service = ?2",
        params![source_id(email), SERVICE],
    )?;
    Ok(())
}

/// Whether an account has a Contacts row.
pub fn is_linked(conn: &Connection, email: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM connector_sources WHERE id = ?1 AND service = ?2",
        params![source_id(email), SERVICE],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// What the keychain holds for an account's Contacts token, with no network: `None` when there is no
/// token (or PM can't read the one there), else the scopes it records. The outer `None` is what a
/// restored store shows as "Reconnect".
pub fn stored_token_scope(email: &str) -> Option<Option<String>> {
    let raw = secrets::get_google_token_for(&token_key(email))
        .ok()
        .flatten()?;
    let token: google::Token = serde_json::from_str(raw.expose()).ok()?;
    Some(token.scope)
}

// --- the user's choices ---

/// Every stored mode. A value PM can't read reads as no mode for every account.
pub fn sync_modes(conn: &Connection) -> Result<BTreeMap<String, SyncMode>> {
    calendar_editing::read_account_map(conn, SYNC_KEY)
}

/// One account's mode.
pub fn sync_mode(conn: &Connection, email: &str) -> Result<Option<SyncMode>> {
    Ok(sync_modes(conn)?.get(&key_of(email)).copied())
}

/// Store (or with `None`, clear) one account's mode.
pub fn set_sync_mode(conn: &Connection, email: &str, mode: Option<SyncMode>) -> Result<()> {
    calendar_editing::write_account_entry(conn, SYNC_KEY, email, mode)
}

/// Every stored sending choice.
pub fn writing_choices(conn: &Connection) -> Result<BTreeMap<String, Choice>> {
    calendar_editing::read_account_map(conn, WRITING_KEY)
}

/// One account's sending choice.
pub fn writing_choice(conn: &Connection, email: &str) -> Result<Option<Choice>> {
    Ok(writing_choices(conn)?.get(&key_of(email)).copied())
}

/// Store (or with `None`, clear) one account's sending choice.
pub fn set_writing_choice(conn: &Connection, email: &str, choice: Option<Choice>) -> Result<()> {
    calendar_editing::write_account_entry(conn, WRITING_KEY, email, choice)
}

/// Whether `email`'s account granted PM the Contacts write scope and PM hasn't revoked it since.
// Read by the account notes in the Connectors page (the UI slice), as Calendar's is by its note.
#[allow(dead_code)]
pub fn write_granted(conn: &Connection, email: &str) -> Result<bool> {
    Ok(calendar_editing::read_account_set(conn, WRITE_GRANTED_KEY)?.contains(&key_of(email)))
}

/// Record that `email`'s account granted the Contacts write scope.
pub fn mark_write_granted(conn: &Connection, email: &str) -> Result<()> {
    let mut all = calendar_editing::read_account_set(conn, WRITE_GRANTED_KEY)?;
    all.insert(key_of(email));
    calendar_editing::write_account_set(conn, WRITE_GRANTED_KEY, &all)
}

/// PM revoked `email`'s grant at Google, which ends the write scope with everything else.
pub fn forget_write_grant(conn: &Connection, email: &str) -> Result<()> {
    let mut all = calendar_editing::read_account_set(conn, WRITE_GRANTED_KEY)?;
    all.remove(&key_of(email));
    calendar_editing::write_account_set(conn, WRITE_GRANTED_KEY, &all)
}

/// The settings that follow an account's sign-in being forgotten: its mode becomes `leave_mode` (Copy
/// once keeps `Once`, Stop syncing passes `None`) and its sending choice is removed, so sending reads
/// Off. The grant stays recorded in [`WRITE_GRANTED_KEY`]: Google's side is unchanged until a revoke.
/// People changed meanwhile keep their `dirty_at`, so turning sending on later asks before sending.
pub fn settle_after_sign_out(
    conn: &Connection,
    email: &str,
    leave_mode: Option<SyncMode>,
) -> Result<()> {
    set_sync_mode(conn, email, leave_mode)?;
    set_writing_choice(conn, email, None)
}

/// Forget every account's choices (the Google client was cleared, so every sign-in went), the grant
/// record with them.
pub fn clear_all(conn: &Connection) -> Result<()> {
    crate::db::delete_setting(conn, SYNC_KEY)?;
    crate::db::delete_setting(conn, WRITING_KEY)?;
    crate::db::delete_setting(conn, WRITE_GRANTED_KEY)
}

/// Whether PM may send an account's changes, from its mode, its sending choice and the scopes its
/// token holds. Sending belongs to Keep in sync only, so any other mode reads Off.
pub fn sending_status(
    mode: Option<SyncMode>,
    choice: Option<Choice>,
    token_scope: Option<&str>,
) -> SendingStatus {
    if mode != Some(SyncMode::Live) {
        return SendingStatus::Off;
    }
    calendar_editing::status_with(choice, token_scope, google::CONTACTS_SCOPE)
}

/// [`sending_status`] for one account as stored now: the answer every write checks.
// Every send checks it, which the write path adds; nothing sends yet.
#[allow(dead_code)]
pub fn sending(conn: &Connection, email: &str) -> Result<SendingStatus> {
    let scope = stored_token_scope(email).flatten();
    Ok(sending_status(
        sync_mode(conn, email)?,
        writing_choice(conn, email)?,
        scope.as_deref(),
    ))
}

/// Classify a finished consent from the scopes Google `granted` and the address it signed in as
/// (`None` when it couldn't be read, which only happens without the read scope): the calendar's rule
/// ([`calendar_editing::classify_consent`]) over Contacts' scopes. `expected` is the account the user
/// meant, or `None` on a first link, where whoever signs in is the account.
///
/// - the read box unticked: nothing is saved (`ReadonlyMissing`);
/// - the write box unticked on a sending consent: the read token is saved and sending stays off
///   (`WriteDeclined`);
/// - another account picked: refused (`WrongAccount`).
///
/// A link consent never asks for the write scope, so for it `Enabled` and `WriteDeclined` both mean
/// "the right account read-granted".
pub fn classify_consent(
    expected: Option<&str>,
    granted: &str,
    signed_in_as: Option<&str>,
) -> ConsentVerdict {
    calendar_editing::classify(
        expected,
        granted,
        signed_in_as,
        google::CONTACTS_READONLY_SCOPE,
        google::CONTACTS_SCOPE,
    )
}

// --- which account signed in ---

/// The People API's own address for the signed-in account, which is the only person `people/me`
/// answers for.
const ME_URL: &str = "https://people.googleapis.com/v1/people/me";

/// The request that names the account: `GET people/me?personFields=emailAddresses`, with **no**
/// `sources` parameter. It is the one People call that doesn't use the pinned `SOURCES`, because the
/// account's address is profile data and a contact-only mask would not return it. Both Contacts
/// scopes are accepted by `people.get`. Pure, so a test pins the query.
pub fn me_url() -> reqwest::Url {
    reqwest::Url::parse_with_params(ME_URL, [("personFields", "emailAddresses")])
        .expect("a constant URL with one constant parameter parses")
}

/// The address of the account that signed in, from `people/me`'s reply, lowercased. A refusal is
/// answered in fixed words, never with the body: the disabled-API 403 gets the actionable error, a
/// 401 or 403 says the sign-in was refused, a 429 says Google is busy.
pub fn parse_address_reply(status: u16, body: &str) -> Result<String> {
    match status {
        200 => {}
        403 if is_api_disabled(body) => return Err(api_disabled_error(body)),
        401 | 403 => {
            return Err(Error::Other(
                "Google didn't let PM read which account signed in, so nothing was saved. Try \
                 again and leave every box ticked."
                    .into(),
            ))
        }
        429 => {
            return Err(Error::Other(
                "Google is busy right now. Wait a minute and try again.".into(),
            ))
        }
        other => {
            return Err(Error::Other(format!(
                "Google couldn't say which account signed in (HTTP {other}). Try again in a minute."
            )))
        }
    }
    let parsed: Value = serde_json::from_str(body).map_err(|_| unreadable_reply())?;
    let addresses = parsed
        .get("emailAddresses")
        .and_then(Value::as_array)
        .ok_or_else(unreadable_reply)?;
    // Only the address Google marks as coming from the ACCOUNT is the account's own. A primary or
    // first entry can be a public profile address, and keying the account (its row, its keychain
    // token, every setting) under that would hide it from Calendar's disconnect, which would then
    // revoke the shared grant. The address scope is asked for to make this entry present; without it
    // nothing is saved.
    let chosen = addresses
        .iter()
        .find(|a| a.pointer("/metadata/source/type").and_then(Value::as_str) == Some("ACCOUNT"))
        .and_then(|a| a.get("value"))
        .and_then(Value::as_str)
        .map(key_of)
        .filter(|a| a.contains('@') && !a.contains(char::is_whitespace))
        .ok_or_else(unreadable_reply)?;
    Ok(chosen)
}

fn unreadable_reply() -> Error {
    Error::Other("Google's answer about the account couldn't be read, so nothing was saved.".into())
}

/// Ask Google which account a fresh token belongs to, before the token is saved anywhere. The token
/// is in hand, seconds old, so no refresh; the call is a GET with the one parameter above.
pub async fn fetch_signed_in_address(token: &google::Token) -> Result<String> {
    let url = me_url();
    let (status, body) = google::get_raw_with_token(token, url.as_str()).await?;
    parse_address_reply(status, &body)
}

// --- the People API switched off ---

/// Where to turn the People API on when Google's own link isn't to hand.
pub const PEOPLE_API_ENABLE_URL: &str =
    "https://console.cloud.google.com/apis/library/people.googleapis.com";
/// The words every disabled-API error carries, which the page matches to show its link.
pub const API_DISABLED_MARKER: &str = "People API isn't turned on";

/// Whether a 403 body is Google saying the API isn't enabled on the Cloud project (rather than the
/// grant being refused): the two markers its body carries for this case, and the older
/// `accessNotConfigured` reason. Reconnecting can't fix it, so it needs its own answer.
pub fn is_api_disabled(body: &str) -> bool {
    body.contains("SERVICE_DISABLED")
        || body.contains("has not been used in project")
        || body.contains("accessNotConfigured")
}

/// The error for a disabled People API, naming where to turn it on: Google's own link for the
/// project when its body carries one on a Google console host (it is the user's project number, and
/// no contact data), else [`PEOPLE_API_ENABLE_URL`]. The only thing taken from the body is that link.
pub fn api_disabled_error(body: &str) -> Error {
    let link = enable_link(body).unwrap_or_else(|| PEOPLE_API_ENABLE_URL.to_string());
    Error::Other(format!(
        "Google's {API_DISABLED_MARKER} for your Google Cloud project, so Contacts can't reach \
         Google yet. Turn it on at {link}, wait a minute, then try again."
    ))
}

/// The first console link in a Google error body, if it is on a Google console host and made of
/// URL characters only.
fn enable_link(body: &str) -> Option<String> {
    const HOSTS: [&str; 2] = [
        "https://console.developers.google.com/",
        "https://console.cloud.google.com/",
    ];
    let (start, host) = HOSTS
        .iter()
        .filter_map(|h| body.find(h).map(|at| (at, *h)))
        .min_by_key(|(at, _)| *at)?;
    let link: String = body[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || "/._-?=&%:~".contains(*c))
        .collect();
    let link = link.trim_end_matches(['.', ',', ':']).to_string();
    // Longer than the host it matched: a bare host isn't where to turn the API on.
    (link.len() > host.len()).then_some(link)
}

// --- what Connectors shows ---

/// One linked account as Connectors shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContactsAccount {
    pub email: String,
    /// `None` for an account that is only a set of kept links ("Not syncing").
    pub mode: Option<SyncMode>,
    /// There is a Contacts row and a token PM can read.
    pub signed_in: bool,
    /// There is a Contacts row but no token (a restore, or a keychain that lost it): "Reconnect".
    pub needs_reconnect: bool,
    /// Whether PM may send changes now.
    pub sending: SendingStatus,
    /// Rows of `person_sources` for the account: linked, copied, waiting and tombstoned alike.
    pub links: i64,
    /// Rows of `label_sources` for the account.
    pub label_links: i64,
}

/// The store's half of the accounts Contacts shows: those with a row, a mode, or links kept. Read
/// under the DB guard; the keychain's half (what each token holds, [`stored_token_scope`]) is read
/// after the guard is released, as Calendar's overview does, because a keychain can wait on an unlock
/// prompt and the guard would hold the whole app behind it.
pub struct AccountsRead {
    linked: BTreeSet<String>,
    modes: BTreeMap<String, SyncMode>,
    choices: BTreeMap<String, Choice>,
    person_links: BTreeMap<String, i64>,
    label_links: BTreeMap<String, i64>,
}

impl AccountsRead {
    /// Everything the store knows about the accounts; no keychain.
    pub fn read(conn: &Connection) -> Result<Self> {
        Ok(Self {
            linked: linked_emails(conn)?.iter().map(|e| key_of(e)).collect(),
            modes: sync_modes(conn)?,
            choices: writing_choices(conn)?,
            person_links: link_counts(conn, PERSON_LINKS_SQL)?,
            label_links: link_counts(conn, LABEL_LINKS_SQL)?,
        })
    }

    /// Every account, in address order. `stored_scope` says what the keychain holds for an account's
    /// token ([`stored_token_scope`]); passed in so a test builds the answer without a keychain.
    pub fn all(
        &self,
        stored_scope: impl Fn(&str) -> Option<Option<String>>,
    ) -> Vec<ContactsAccount> {
        let mut every: BTreeSet<&String> = self.linked.iter().collect();
        every.extend(self.modes.keys());
        every.extend(self.person_links.keys());
        every.extend(self.label_links.keys());
        every
            .into_iter()
            .map(|email| self.build(email, &stored_scope))
            .collect()
    }

    /// One account, the keychain asked about that account only; `None` when PM knows nothing of it.
    pub fn one(
        &self,
        email: &str,
        stored_scope: impl Fn(&str) -> Option<Option<String>>,
    ) -> Option<ContactsAccount> {
        let email = key_of(email);
        let known = self.linked.contains(&email)
            || self.modes.contains_key(&email)
            || self.person_links.contains_key(&email)
            || self.label_links.contains_key(&email);
        known.then(|| self.build(&email, &stored_scope))
    }

    fn build(
        &self,
        email: &str,
        stored_scope: &impl Fn(&str) -> Option<Option<String>>,
    ) -> ContactsAccount {
        let token = self.linked.contains(email).then(|| stored_scope(email));
        let mode = self.modes.get(email).copied();
        let scope = token.clone().flatten().flatten();
        ContactsAccount {
            sending: sending_status(mode, self.choices.get(email).copied(), scope.as_deref()),
            signed_in: matches!(token, Some(Some(_))),
            needs_reconnect: matches!(token, Some(None)),
            mode,
            links: self.person_links.get(email).copied().unwrap_or(0),
            label_links: self.label_links.get(email).copied().unwrap_or(0),
            email: email.to_string(),
        }
    }
}

/// The Google links per account, from one of the two counting queries below.
fn link_counts(conn: &Connection, sql: &str) -> Result<BTreeMap<String, i64>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params![PROVIDER], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<BTreeMap<String, i64>, _>>()?;
    Ok(rows)
}

const PERSON_LINKS_SQL: &str = "SELECT account, COUNT(*) FROM person_sources \
     WHERE provider = ?1 AND account <> '' GROUP BY account";
const LABEL_LINKS_SQL: &str = "SELECT account, COUNT(*) FROM label_sources \
     WHERE provider = ?1 AND account <> '' GROUP BY account";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::google::{CONTACTS_READONLY_SCOPE as READ, CONTACTS_SCOPE as WRITE};

    const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn store() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        (dir, conn)
    }

    /// The strings that cannot change without orphaning something a user already holds: the keychain
    /// prefix, the connector id prefix, the `clientData` key a created contact carries, and the three
    /// vCard names a file PM exported carries. Pinned together so a rename fails here.
    #[test]
    fn the_one_way_door_strings_are_pinned() {
        assert_eq!(
            secrets::GOOGLE_TOKEN_CONTACTS_PREFIX,
            "google_oauth_token_contacts::"
        );
        assert_eq!(
            token_key("Me@Example.com"),
            "google_oauth_token_contacts::me@example.com"
        );
        assert_eq!(SOURCE_ID_PREFIX, "gcontacts:");
        assert_eq!(source_id(" Me@Example.com "), "gcontacts:me@example.com");
        assert_eq!(PM_UID_CLIENT_DATA_KEY, "pm.uid");
        assert_eq!((PROVIDER, SERVICE), ("google", "contacts"));
        for key in [SYNC_KEY, WRITING_KEY, WRITE_GRANTED_KEY] {
            assert!(key.starts_with("contacts_"), "{key}");
        }

        // The vCard names, read and written: a file PM wrote years ago must still read back.
        use crate::people::card::{Card, CardDetail};
        let mut card = Card {
            display_name: "Sam Example".into(),
            met_where: "The Lisbon meetup".into(),
            met_on: Some("2026-03-04".into()),
            ..Default::default()
        };
        card.details.push(CardDetail {
            kind: "location".into(),
            value: "Lisbon".into(),
            label: String::new(),
            parts: None,
            from_date: None,
            to_date: None,
            is_current: false,
        });
        let mut text = String::new();
        crate::people::export::write_card(&mut text, &card);
        for name in ["X-PM-MET-WHERE:", "X-PM-MET-ON:", "X-PM-LOCATION:"] {
            assert!(
                text.lines().any(|l| l.starts_with(name)),
                "the export writes {name}"
            );
        }
        let back = crate::people::vcard::parse_vcf(text.as_bytes());
        assert_eq!(back.cards.len(), 1);
        assert!(back.cards[0].met_where == card.met_where);
        assert!(back.cards[0].met_on == card.met_on);
        assert!(back.cards[0]
            .details
            .iter()
            .any(|d| d.kind == "location" && d.value == "Lisbon"));
    }

    /// The Contacts row is its own service: `service` has no CHECK, so it lands with no migration,
    /// and it is told apart from Calendar's and Drive's rows for the same address.
    #[test]
    fn a_contacts_row_is_its_own_service() {
        let (_dir, conn) = store();
        upsert_source(&conn, "Me@Example.com").unwrap();
        crate::calendar::upsert_source(
            &conn,
            &crate::calendar::google_account_id("me@example.com"),
            "google",
            Some("me@example.com"),
            "me@example.com",
        )
        .unwrap();
        crate::drive::upsert_account(&conn, "me@example.com", "Me").unwrap();
        assert_eq!(linked_emails(&conn).unwrap(), vec!["me@example.com"]);
        assert!(is_linked(&conn, " ME@example.com").unwrap());
        assert!(!is_linked(&conn, "other@example.com").unwrap());
        // Linking again is the same row.
        upsert_source(&conn, "me@example.com").unwrap();
        assert_eq!(linked_emails(&conn).unwrap().len(), 1);
        // Removing it leaves Calendar's and Drive's.
        remove_source(&conn, "me@example.com").unwrap();
        assert!(linked_emails(&conn).unwrap().is_empty());
        assert_eq!(
            crate::calendar::list_sources(&conn, Some("google"))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(crate::drive::list_accounts(&conn).unwrap().len(), 1);
    }

    #[test]
    fn sending_is_on_only_when_kept_in_sync_chosen_and_granted() {
        use EditingStatus::*;
        let both = format!("{READ} {WRITE}");
        let live = Some(SyncMode::Live);
        type Case<'a> = (
            Option<SyncMode>,
            Option<Choice>,
            Option<&'a str>,
            SendingStatus,
        );
        let cases: [Case; 11] = [
            (live, Some(Choice::On), Some(&both), On),
            // The scope alone never turns sending on.
            (live, None, Some(&both), Off),
            (live, None, None, Off),
            // Chosen, but the token lost the write scope (or is gone): ask Google again.
            (live, Some(Choice::On), Some(READ), NeedsConsent),
            (live, Some(Choice::On), None, NeedsConsent),
            // The user's word, whatever the token holds.
            (live, Some(Choice::Paused), Some(&both), Paused),
            (live, Some(Choice::Paused), None, Paused),
            // Sending belongs to Keep in sync: any other mode reads Off, a stale choice or not.
            (Some(SyncMode::Once), Some(Choice::On), Some(&both), Off),
            (Some(SyncMode::Once), Some(Choice::Paused), Some(&both), Off),
            (None, Some(Choice::On), Some(&both), Off),
            (None, None, None, Off),
        ];
        for (mode, choice, scope, want) in cases {
            assert_eq!(
                sending_status(mode, choice, scope),
                want,
                "{mode:?} {choice:?} {scope:?}"
            );
        }
    }

    #[test]
    fn a_consent_is_classified_before_anything_is_saved() {
        use ConsentVerdict::*;
        let both = format!("{READ} {WRITE} {}", google::USERINFO_EMAIL_SCOPE);
        let read = format!("{READ} {}", google::USERINFO_EMAIL_SCOPE);
        let me = Some("me@example.com");
        let cases: [(Option<&str>, &str, Option<&str>, ConsentVerdict); 9] = [
            (me, both.as_str(), me, Enabled),
            // Google's casing of the address doesn't matter.
            (me, both.as_str(), Some("Me@Example.com"), Enabled),
            (me, read.as_str(), me, WriteDeclined),
            (
                me,
                both.as_str(),
                Some("other@example.com"),
                WrongAccount {
                    signed_in_as: "other@example.com".into(),
                },
            ),
            // Read scope unticked: the account couldn't even be identified.
            (me, WRITE, None, ReadonlyMissing),
            (me, WRITE, me, ReadonlyMissing),
            (me, "", None, ReadonlyMissing),
            // A first link expects nobody: whoever signs in is the account.
            (None, read.as_str(), Some("new@example.com"), WriteDeclined),
            (None, WRITE, Some("new@example.com"), ReadonlyMissing),
        ];
        for (expected, granted, signed_in_as, want) in cases {
            assert_eq!(
                classify_consent(expected, granted, signed_in_as),
                want,
                "{expected:?} {granted:?} {signed_in_as:?}"
            );
        }
    }

    /// The account's address is asked for with `personFields` and nothing else: no `sources`, which
    /// would narrow the answer to contacts, and nothing from the account in the request at all.
    #[test]
    fn the_account_address_call_sends_no_sources() {
        let url = me_url();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("people.googleapis.com"));
        assert_eq!(url.path(), "/v1/people/me");
        let query: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(
            query,
            vec![("personFields".to_string(), "emailAddresses".to_string())]
        );
        assert!(!url.as_str().contains("sources"));
        assert_eq!(url.fragment(), None);
    }

    #[test]
    fn the_signed_in_address_is_read_from_the_account_entry() {
        let both = r#"{"resourceName":"people/1","emailAddresses":[
            {"metadata":{"primary":true,"source":{"type":"PROFILE","id":"1"}},"value":"Public@Example.com"},
            {"metadata":{"source":{"type":"ACCOUNT","id":"1"}},"value":" Me@Example.com "}]}"#;
        assert_eq!(parse_address_reply(200, both).unwrap(), "me@example.com");
        // Nothing usable: no ACCOUNT entry (a primary or first address may be a public profile one,
        // so it is never taken for the account), no list, an empty one, a value that isn't an
        // address, a body that isn't JSON.
        for body in [
            r#"{"emailAddresses":[{"value":"a@example.com"},
                {"metadata":{"primary":true},"value":"B@example.com"}]}"#,
            r#"{"emailAddresses":[{"metadata":{"primary":true,"source":{"type":"PROFILE"}},"value":"a@example.com"}]}"#,
            r#"{"emailAddresses":[{"value":"a@example.com"}]}"#,
            r#"{"emailAddresses":[{"metadata":{"source":{"type":"ACCOUNT"}},"value":"not an address"}]}"#,
            "{}",
            r#"{"emailAddresses":[]}"#,
            r#"{"emailAddresses":[{"value":"not an address"}]}"#,
            r#"{"emailAddresses":[{"value":""}]}"#,
            "<html>",
            "",
        ] {
            assert!(parse_address_reply(200, body).is_err(), "{body}");
        }
    }

    /// A refusal is answered in fixed words: the body, which can hold an address, never reaches the
    /// error, whatever it says.
    #[test]
    fn a_refused_address_call_never_echoes_the_reply() {
        let body =
            r#"{"error":{"code":403,"message":"Nobody at ghost@example.com may read this"}}"#;
        for status in [400, 401, 403, 404, 429, 500, 503] {
            let message = parse_address_reply(status, body).unwrap_err().to_string();
            assert!(!message.contains("ghost"), "{status}: {message}");
            assert!(!message.contains("example.com"), "{status}: {message}");
        }
        // A 200 that can't be read doesn't echo it either.
        let message = parse_address_reply(200, "ghost@example.com not json")
            .unwrap_err()
            .to_string();
        assert!(!message.contains("ghost"));
    }

    /// Google's documented body for a Cloud project with the API switched off.
    const DISABLED: &str = r#"{"error":{"code":403,"message":"People API has not been used in project 123456789 before or it is disabled. Enable it by visiting https://console.developers.google.com/apis/api/people.googleapis.com/overview?project=123456789 then retry.","status":"PERMISSION_DENIED","details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"SERVICE_DISABLED","domain":"googleapis.com"}]}}"#;

    #[test]
    fn a_disabled_people_api_gets_the_actionable_error() {
        assert!(is_api_disabled(DISABLED));
        assert!(is_api_disabled("has not been used in project 1"));
        assert!(is_api_disabled(
            r#"{"error":{"errors":[{"reason":"accessNotConfigured"}]}}"#
        ));
        assert!(!is_api_disabled(
            r#"{"error":{"status":"PERMISSION_DENIED"}}"#
        ));

        let message = parse_address_reply(403, DISABLED).unwrap_err().to_string();
        assert!(message.contains(API_DISABLED_MARKER), "{message}");
        // Google's own link for the project, and nothing else of its body.
        assert!(
            message.contains(
                "at https://console.developers.google.com/apis/api/people.googleapis.com/overview?project=123456789, wait"
            ),
            "{message}"
        );
        assert!(!message.contains("ErrorInfo") && !message.contains("PERMISSION_DENIED"));

        // No link in the body, a link to anywhere but a Google console, or one that is nothing but a
        // host: the fallback is the library page.
        for body in [
            r#"{"error":{"details":[{"reason":"SERVICE_DISABLED"}]}}"#,
            r#"SERVICE_DISABLED see https://evil.example.com/apis/api/people.googleapis.com"#,
            "SERVICE_DISABLED https://console.cloud.google.com/",
            "SERVICE_DISABLED https://console.developers.google.com/",
        ] {
            let message = parse_address_reply(403, body).unwrap_err().to_string();
            assert!(message.contains(PEOPLE_API_ENABLE_URL), "{body}: {message}");
            assert!(!message.contains("evil"), "{message}");
        }
        assert_eq!(
            PEOPLE_API_ENABLE_URL,
            "https://console.cloud.google.com/apis/library/people.googleapis.com"
        );
        // A 403 for any other reason is the sign-in refused, not a disabled API.
        let other = parse_address_reply(403, r#"{"error":{"status":"PERMISSION_DENIED"}}"#)
            .unwrap_err()
            .to_string();
        assert!(!other.contains(API_DISABLED_MARKER));
    }

    #[test]
    fn choices_are_stored_per_account_and_cleared_cleanly() {
        let (_dir, conn) = store();
        assert_eq!(sync_mode(&conn, "a@example.com").unwrap(), None);
        set_sync_mode(&conn, "A@Example.com", Some(SyncMode::Live)).unwrap();
        set_sync_mode(&conn, "b@example.com", Some(SyncMode::Once)).unwrap();
        set_writing_choice(&conn, "a@example.com", Some(Choice::Paused)).unwrap();
        assert_eq!(
            sync_mode(&conn, " a@EXAMPLE.com").unwrap(),
            Some(SyncMode::Live)
        );
        assert_eq!(
            crate::db::get_setting(&conn, SYNC_KEY).unwrap().as_deref(),
            Some(r#"{"a@example.com":"live","b@example.com":"once"}"#)
        );
        assert_eq!(
            crate::db::get_setting(&conn, WRITING_KEY)
                .unwrap()
                .as_deref(),
            Some(r#"{"a@example.com":"paused"}"#)
        );
        set_writing_choice(&conn, "a@example.com", None).unwrap();
        // The last one out removes the key rather than leaving `{}` behind.
        assert_eq!(crate::db::get_setting(&conn, WRITING_KEY).unwrap(), None);

        // An unreadable value reads as no choice for anybody.
        crate::db::set_setting(&conn, SYNC_KEY, r#"{"a@example.com":"sometimes"}"#).unwrap();
        assert_eq!(sync_mode(&conn, "a@example.com").unwrap(), None);
        crate::db::set_setting(&conn, WRITING_KEY, "not json").unwrap();
        assert!(writing_choices(&conn).unwrap().is_empty());
    }

    /// Stop syncing and switching to Copy once remove the account's sending entry, and keep the
    /// grant on record: Google's side hasn't changed.
    #[test]
    fn stopping_or_copying_once_removes_the_sending_choice_and_keeps_the_grant() {
        let (_dir, conn) = store();
        for (leave, want_mode) in [(None, None), (Some(SyncMode::Once), Some(SyncMode::Once))] {
            set_sync_mode(&conn, "a@example.com", Some(SyncMode::Live)).unwrap();
            set_writing_choice(&conn, "a@example.com", Some(Choice::On)).unwrap();
            set_writing_choice(&conn, "b@example.com", Some(Choice::On)).unwrap();
            mark_write_granted(&conn, "A@example.com").unwrap();

            settle_after_sign_out(&conn, "a@example.com", leave).unwrap();

            assert_eq!(sync_mode(&conn, "a@example.com").unwrap(), want_mode);
            assert_eq!(writing_choice(&conn, "a@example.com").unwrap(), None);
            // Another account is untouched.
            assert_eq!(
                writing_choice(&conn, "b@example.com").unwrap(),
                Some(Choice::On)
            );
            assert!(write_granted(&conn, "a@example.com").unwrap());
            assert_eq!(
                sending_status(
                    want_mode,
                    writing_choice(&conn, "a@example.com").unwrap(),
                    Some(WRITE)
                ),
                SendingStatus::Off
            );
        }
        forget_write_grant(&conn, " A@example.com ").unwrap();
        assert!(!write_granted(&conn, "a@example.com").unwrap());
        // Clear credentials takes every key.
        set_sync_mode(&conn, "a@example.com", Some(SyncMode::Live)).unwrap();
        mark_write_granted(&conn, "a@example.com").unwrap();
        clear_all(&conn).unwrap();
        for key in [SYNC_KEY, WRITING_KEY, WRITE_GRANTED_KEY] {
            assert_eq!(crate::db::get_setting(&conn, key).unwrap(), None, "{key}");
        }
    }

    /// Calendar's own choices and Contacts' are different keys: touching one never touches the other.
    #[test]
    fn contacts_settings_and_calendar_editing_do_not_share_keys() {
        let (_dir, conn) = store();
        calendar_editing::set_choice(&conn, "a@example.com", Some(Choice::On)).unwrap();
        calendar_editing::mark_write_granted(&conn, "a@example.com").unwrap();
        set_writing_choice(&conn, "a@example.com", Some(Choice::Paused)).unwrap();
        mark_write_granted(&conn, "a@example.com").unwrap();
        clear_all(&conn).unwrap();
        assert_eq!(
            calendar_editing::choice(&conn, "a@example.com").unwrap(),
            Some(Choice::On)
        );
        assert!(calendar_editing::write_granted(&conn, "a@example.com").unwrap());
        calendar_editing::clear_all(&conn).unwrap();
        set_writing_choice(&conn, "a@example.com", Some(Choice::On)).unwrap();
        assert_eq!(
            writing_choice(&conn, "a@example.com").unwrap(),
            Some(Choice::On)
        );
    }

    #[test]
    fn the_accounts_shown_are_the_rows_the_modes_and_the_kept_links() {
        let (_dir, conn) = store();
        let person = crate::people::create_person(
            &conn,
            &crate::people::PersonDraft {
                given_name: "Sam".into(),
                family_name: "Example".into(),
                ..Default::default()
            },
        )
        .unwrap();
        // live: signed in and sending; once: copied, no row; stopped: links only; restored: a row
        // with no token.
        upsert_source(&conn, "live@example.com").unwrap();
        upsert_source(&conn, "restored@example.com").unwrap();
        set_sync_mode(&conn, "live@example.com", Some(SyncMode::Live)).unwrap();
        set_writing_choice(&conn, "live@example.com", Some(Choice::On)).unwrap();
        set_sync_mode(&conn, "once@example.com", Some(SyncMode::Once)).unwrap();
        set_sync_mode(&conn, "restored@example.com", Some(SyncMode::Live)).unwrap();
        set_writing_choice(&conn, "restored@example.com", Some(Choice::On)).unwrap();
        for (i, acct) in [
            "live@example.com",
            "stopped@example.com",
            "stopped@example.com",
        ]
        .iter()
        .enumerate()
        {
            conn.execute(
                "INSERT INTO person_sources(person_id, provider, account, external_id) \
                 VALUES (?1, 'google', ?2, ?3)",
                params![person, acct, format!("people/c{i}")],
            )
            .unwrap();
        }
        // A vCard link and an account-less one are not a Google account.
        conn.execute(
            "INSERT INTO person_sources(person_id, provider, account, external_id) \
             VALUES (?1, 'vcard', '', 'file-1')",
            params![person],
        )
        .unwrap();
        let held = format!("{READ} {WRITE}");
        let scope_of = |email: &str| match email {
            "live@example.com" => Some(Some(held.clone())),
            _ => None,
        };
        let read = AccountsRead::read(&conn).unwrap();
        let shown = read.all(scope_of);
        // One account is the same answer as its row in the list, and an unknown one is none.
        assert_eq!(
            read.one(" LIVE@example.com", scope_of).as_ref(),
            shown.iter().find(|a| a.email == "live@example.com")
        );
        assert!(read.one("nobody@example.com", scope_of).is_none());
        let by = |email: &str| shown.iter().find(|a| a.email == email).unwrap().clone();
        assert_eq!(
            shown.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(),
            vec![
                "live@example.com",
                "once@example.com",
                "restored@example.com",
                "stopped@example.com"
            ]
        );
        let live = by("live@example.com");
        assert_eq!(
            (
                live.mode,
                live.signed_in,
                live.needs_reconnect,
                live.sending,
                live.links
            ),
            (Some(SyncMode::Live), true, false, SendingStatus::On, 1)
        );
        let once = by("once@example.com");
        assert_eq!(
            (
                once.mode,
                once.signed_in,
                once.needs_reconnect,
                once.sending
            ),
            (Some(SyncMode::Once), false, false, SendingStatus::Off)
        );
        // A restore brings the row and the choices but not the sign-in: Reconnect, and sending
        // asks for Google's permission again.
        let restored = by("restored@example.com");
        assert_eq!(
            (
                restored.signed_in,
                restored.needs_reconnect,
                restored.sending
            ),
            (false, true, SendingStatus::NeedsConsent)
        );
        let stopped = by("stopped@example.com");
        assert_eq!(
            (
                stopped.mode,
                stopped.signed_in,
                stopped.needs_reconnect,
                stopped.links
            ),
            (None, false, false, 2)
        );
        // The wire words the page reads.
        let json = serde_json::to_value(&live).unwrap();
        assert_eq!(json["mode"], "live");
        assert_eq!(json["sending"], "on");
        assert_eq!(serde_json::to_value(&stopped).unwrap()["mode"], Value::Null);
    }
}
