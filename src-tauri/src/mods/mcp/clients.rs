// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The connected apps (Mods spec 1.16): minting a token, listing the apps, revoking one, and the
//! single question every request comes down to, "is this app still allowed to ask?" ([`verify`] for a
//! token, [`recheck`] for an app a connection already authenticated as).
//!
//! **A token** is `pmcp_<id>_<64 hex>`: the app's row id, so the row is found without scanning, and 32
//! random bytes. It is shown once, at [`mint`], and kept nowhere: the store's row holds its hash
//! (`token_hash`, the hex SHA-256 of the whole token text), this device's file holds the same hash in
//! a pair ([`device`]). It is never logged, never in an error, a tool result or a child's environment.
//!
//! **An app verifies only when four things agree**, none of them the store alone:
//! 1. this device's file says the server is ON for this vault, at this folder ([`device::device_state`]);
//! 2. the file holds a pair for this app id, whose `hash` equals the hash of the presented token (when
//!    there is a token to compare; the recheck of an authenticated connection has none);
//! 3. the store's row for the id exists and holds that same hash (a row someone swapped in the store
//!    is not the one this device paired);
//! 4. the row's `scopes` hash to the pair's `scopes_hash`, so a store writer cannot widen what an app
//!    may read.
//!
//! A row another device made (a shared vault) has no pair here and never verifies: that is how a
//! device accepts only its own apps, with nothing written to the store to say so. Every comparison of a
//! hash is [`ct_eq`], and the store is never asked to compare one (no `WHERE` on the hash): the row
//! comes by id, and the comparison happens here.
//!
//! The scopes an app may hold are permission ids, [`super::SCOPES`]. `calendar.details` is never
//! offered.

use std::collections::HashMap;
use std::io;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::device::{self, PairedClient, MAX_CLIENTS_PER_VAULT};
use super::{Fault, SCOPES};
use crate::mods::devmode::Here;
use crate::mods::package::tidy_user_line;
use crate::oauth_loopback::ct_eq;

/// The longest an app's name is, in characters.
pub const NAME_MAX_CHARS: usize = 60;

const TOKEN_PREFIX: &str = "pmcp_";

/// Rows listed at most: a shared store's writer can add rows, and the list is a screen.
const LIST_MAX: usize = 64;

/// How often an app's `last_used_at` is written at most.
const LAST_USED_EVERY: Duration = Duration::from_secs(60);

/// An app that passed every check: its row id and the scopes it holds now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedClient {
    pub id: i64,
    pub scopes: Vec<String>,
}

impl VerifiedClient {
    pub fn holds(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

/// An app as the list shows it. `paired` is whether this device holds its pairing (a row made on
/// another computer sharing the vault, or whose pairing was lost, needs pairing again here).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientView {
    pub id: i64,
    pub name: String,
    pub scopes: Vec<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub paired: bool,
}

fn sha256_hex(parts: &[&[u8]]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    hex::encode(h.finalize())
}

/// What the pair's `scopes_hash` is: `id|hash|scopes` with the scopes sorted and joined by commas.
pub(crate) fn scopes_hash(id: i64, hash: &str, scopes: &[String]) -> String {
    let mut sorted: Vec<&str> = scopes.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    let joined = sorted.join(",");
    sha256_hex(&[
        id.to_string().as_bytes(),
        b"|",
        hash.as_bytes(),
        b"|",
        joined.as_bytes(),
    ])
}

/// The scopes a stored `scopes` column holds, sorted: a JSON array of distinct, offered ids. Anything
/// else (a hand edit, a hostile row) is `None`, which no pair can match.
fn parse_scopes(stored: &str) -> Option<Vec<String>> {
    let mut scopes: Vec<String> = serde_json::from_str(stored).ok()?;
    scopes.sort_unstable();
    let distinct = scopes.windows(2).all(|w| w[0] != w[1]);
    (distinct && scopes.iter().all(|s| SCOPES.contains(&s.as_str()))).then_some(scopes)
}

/// `(id, token hash)` of a presented token, or `None` when it isn't shaped like one PM mints.
fn parse_token(token: &str) -> Option<(i64, String)> {
    let rest = token.strip_prefix(TOKEN_PREFIX)?;
    let (id, body) = rest.split_once('_')?;
    let digits = !id.is_empty()
        && id.len() <= 18
        && id.bytes().all(|b| b.is_ascii_digit())
        && !(id.len() > 1 && id.starts_with('0'));
    let hex = body.len() == 64
        && body
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !digits || !hex {
        return None;
    }
    let id: i64 = id.parse().ok().filter(|id| *id > 0)?;
    Some((id, sha256_hex(&[token.as_bytes()])))
}

/// Make an app: a row, a token and this device's pairing. Returns the app's id and its token, which
/// this call alone ever holds. `scopes` must be a non-empty set of offered ids; `name` is tidied like
/// any line of the person's words and must be 1 to [`NAME_MAX_CHARS`] characters. At most
/// [`MAX_CLIENTS_PER_VAULT`] apps (this device's pairings whose rows have gone are forgotten first, so
/// they never count). Nothing is left behind by a refusal or a failure: the row and the pairing are
/// made together or not at all.
pub fn mint(
    conn: &Connection,
    here: &Here,
    name: &str,
    scopes: &[String],
) -> Result<(i64, String), Fault> {
    let name = tidy_user_line(name);
    if name.is_empty() || name.chars().count() > NAME_MAX_CHARS {
        return Err(Fault::Refused("An app's name is 1 to 60 characters."));
    }
    let mut scopes: Vec<String> = scopes.to_vec();
    scopes.sort_unstable();
    scopes.dedup();
    if scopes.is_empty() || !scopes.iter().all(|s| SCOPES.contains(&s.as_str())) {
        return Err(Fault::Refused("Choose what the app may read."));
    }
    // A pairing whose row is gone (the app was revoked from another computer sharing the vault, or a
    // failed mint's cleanup failed) is nobody's to revoke, and would count against this device's cap
    // for good while the list shows nothing to remove.
    forget_orphan_pairs(conn, here)?;
    let existing: i64 = conn.query_row("SELECT count(*) FROM mcp_clients", [], |r| r.get(0))?;
    if existing >= MAX_CLIENTS_PER_VAULT as i64 {
        return Err(Fault::Refused("Too many apps are connected."));
    }
    let scopes_json = serde_json::to_string(&scopes).map_err(|e| Fault::Disk(e.into()))?;

    let tx = conn.unchecked_transaction()?;
    // The row first, with a placeholder hash (the column is unique and not null), because the token
    // carries the row's id.
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|e| Fault::Disk(std::io::Error::other(e.to_string())))?;
    let body = hex::encode(random);
    let placeholder = format!("pending-{}", &body[..16]);
    tx.execute(
        "INSERT INTO mcp_clients(name, token_hash, scopes) VALUES (?1, ?2, ?3)",
        params![name, placeholder, scopes_json],
    )?;
    let id = tx.last_insert_rowid();
    let token = format!("{TOKEN_PREFIX}{id}_{body}");
    let hash = sha256_hex(&[token.as_bytes()]);
    tx.execute(
        "UPDATE mcp_clients SET token_hash = ?2 WHERE id = ?1",
        params![id, hash],
    )?;
    let pair = PairedClient {
        id,
        scopes_hash: scopes_hash(id, &hash, &scopes),
        hash,
    };
    // A refusal of the file's own (a cap, a folder it can't key) is PM's sentence, not an io error's.
    device::pair(here, pair).map_err(|e| match e.kind() {
        io::ErrorKind::InvalidInput => {
            Fault::Refused("This computer can't connect another app to this vault.")
        }
        _ => e.into(),
    })?;
    if let Err(e) = tx.commit() {
        // The row did not land: the pairing has nothing to pair with.
        let _ = device::unpair(here, id);
        return Err(e.into());
    }
    Ok((id, token))
}

/// Drop this device's pairings that have no row in the store. Asked row by row (a device holds at
/// most [`MAX_CLIENTS_PER_VAULT`] pairs), never by listing the table, which a shared store's writer
/// can fill. Nothing orphaned writes nothing.
fn forget_orphan_pairs(conn: &Connection, here: &Here) -> Result<(), Fault> {
    for pair in device::device_state(here).clients {
        let has_row = conn
            .query_row("SELECT 1 FROM mcp_clients WHERE id = ?1", [pair.id], |_| {
                Ok(())
            })
            .optional()?
            .is_some();
        if !has_row {
            device::unpair(here, pair.id)?;
        }
    }
    Ok(())
}

/// The apps of the store, newest last, as the list shows them.
pub fn list(conn: &Connection, here: &Here) -> Vec<ClientView> {
    let state = device::device_state(here);
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, name, scopes, created_at, last_used_at FROM mcp_clients ORDER BY id LIMIT ?1",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map([LIST_MAX as i64], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<String>>(4)?,
        ))
    });
    let Ok(rows) = rows else {
        return Vec::new();
    };
    rows.flatten()
        .map(|(id, name, scopes, created_at, last_used_at)| ClientView {
            id,
            name,
            paired: pairing_matches(conn, &state.clients, id, None).is_some(),
            scopes: parse_scopes(&scopes).unwrap_or_default(),
            created_at,
            last_used_at,
        })
        .collect()
}

/// Take an app away: its row and this device's pairing. Revoking one that is already gone is not an
/// error. Both steps are tried; the first failure is returned.
pub fn revoke(conn: &Connection, here: &Here, id: i64) -> Result<(), Fault> {
    let deleted = conn
        .execute("DELETE FROM mcp_clients WHERE id = ?1", [id])
        .map(|_| ());
    let unpaired = device::unpair(here, id);
    deleted?;
    unpaired?;
    Ok(())
}

/// Whether app `id`, at the pairs this device holds, still matches its row in the store: the pair
/// exists, the row exists with the pair's hash (and `presented`, the hash of a presented token, when
/// there is one, equals it too), and the row's scopes hash to the pair's. The app's scopes if so.
fn pairing_matches(
    conn: &Connection,
    pairs: &[PairedClient],
    id: i64,
    presented: Option<&str>,
) -> Option<VerifiedClient> {
    let pair = pairs.iter().find(|p| p.id == id)?;
    if let Some(hash) = presented {
        if !ct_eq(hash, &pair.hash) {
            return None;
        }
    }
    let (row_hash, stored_scopes): (String, String) = conn
        .query_row(
            "SELECT token_hash, scopes FROM mcp_clients WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .ok()
        .flatten()?;
    if !ct_eq(&row_hash, &pair.hash) {
        return None;
    }
    let scopes = parse_scopes(&stored_scopes)?;
    if !ct_eq(&scopes_hash(id, &pair.hash, &scopes), &pair.scopes_hash) {
        return None;
    }
    Some(VerifiedClient { id, scopes })
}

/// The app a presented `token` belongs to, if it still may ask (module doc). The one function the
/// server's first frame goes through.
pub fn verify(conn: &Connection, here: &Here, token: &str) -> Option<VerifiedClient> {
    let (id, hash) = parse_token(token)?;
    let state = device::device_state(here);
    if !state.on {
        return None;
    }
    pairing_matches(conn, &state.clients, id, Some(&hash))
}

/// [`verify`] for an app a connection already authenticated as: the same checks, with nothing
/// presented. Every request asks it again, so a revoked app, a switch turned off or a widened row
/// stops at the next request whatever hooks did or didn't run.
pub fn recheck(conn: &Connection, here: &Here, id: i64) -> Option<VerifiedClient> {
    let state = device::device_state(here);
    if !state.on {
        return None;
    }
    pairing_matches(conn, &state.clients, id, None)
}

/// When each app's `last_used_at` was last written, so a chatty app costs one write a minute.
#[derive(Debug, Default)]
pub struct LastUsed(Mutex<HashMap<i64, Instant>>);

impl LastUsed {
    /// Stamp app `id` as used now, unless it was stamped less than a minute before `now`. The write is
    /// best effort: a failure is ignored (an app is never refused for a clock it could not write),
    /// and the in-memory map's lock is not held while it runs.
    pub fn touch(&self, conn: &Connection, id: i64, now: Instant) {
        {
            let mut seen = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            if seen
                .get(&id)
                .is_some_and(|at| now.saturating_duration_since(*at) < LAST_USED_EVERY)
            {
                return;
            }
            // A store's rows are few, but never let the map grow without bound.
            if seen.len() >= 256 {
                seen.clear();
            }
            seen.insert(id, now);
        }
        let _ = conn.execute(
            "UPDATE mcp_clients SET last_used_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') \
             WHERE id = ?1",
            [id],
        );
    }
}
