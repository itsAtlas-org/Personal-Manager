// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The connected apps: minting (the token shown once, its checks, its cap), verifying (the four things
//! that must agree), revoking, listing, and the clock of `last_used_at`.

use std::time::{Duration, Instant};

use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

use super::clients::*;
use super::device::{self, PairedClient};
use super::test_support::open_vault;
use super::{Fault, SCOPES};
use crate::mods::api::test_support::TestHost;
use crate::mods::devmode::Here;

fn rig() -> (TestHost, Here) {
    let host = TestHost::new();
    let here = open_vault(&host);
    (host, here)
}

fn scopes(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| (*s).to_owned()).collect()
}

fn sha(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// Mint an app and switch the server on.
fn app(host: &TestHost, here: &Here, held: &[&str]) -> (i64, String) {
    let made = host.sql(|c| mint(c, here, "Desk", &scopes(held))).unwrap();
    device::set_on(here, true).unwrap();
    made
}

fn verified(host: &TestHost, here: &Here, token: &str) -> Option<VerifiedClient> {
    host.sql(|c| verify(c, here, token))
}

fn rechecked(host: &TestHost, here: &Here, id: i64) -> Option<VerifiedClient> {
    host.sql(|c| recheck(c, here, id))
}

fn rows(host: &TestHost) -> i64 {
    host.sql(|c| {
        c.query_row("SELECT count(*) FROM mcp_clients", [], |r| r.get(0))
            .unwrap()
    })
}

/// A row some OTHER device made in the shared store: a token, its hash and its scopes in the row, and
/// no pairing in this device's file. Returns the app's id and token.
fn foreign_row(host: &TestHost, held: &[&str]) -> (i64, String) {
    host.sql(|c| {
        c.execute(
            "INSERT INTO mcp_clients(name, token_hash, scopes) VALUES ('Elsewhere', 'pending', ?1)",
            [serde_json::to_string(&scopes(held)).unwrap()],
        )
        .unwrap();
        let id = c.last_insert_rowid();
        let token = format!("pmcp_{id}_{}", "ab".repeat(32));
        c.execute(
            "UPDATE mcp_clients SET token_hash = ?2 WHERE id = ?1",
            params![id, sha(&token)],
        )
        .unwrap();
        (id, token)
    })
}

// --- mint ---------------------------------------------------------------------------------------

#[test]
fn a_token_is_shown_once_and_only_its_hash_is_kept() {
    let (host, here) = rig();
    let (id, token) = host
        .sql(|c| mint(c, &here, "Desk", &scopes(&["tasks.read", "calendar.read"])))
        .unwrap();
    let body = token
        .strip_prefix(&format!("pmcp_{id}_"))
        .expect("pmcp_<id>_");
    assert_eq!(body.len(), 64);
    assert!(body
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    // The row holds the hash of the whole token and nothing of the token.
    let (name, hash, held): (String, String, String) = host.sql(|c| {
        c.query_row(
            "SELECT name, token_hash, scopes FROM mcp_clients WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    });
    assert_eq!(name, "Desk");
    assert_eq!(hash, sha(&token));
    assert_eq!(held, r#"["calendar.read","tasks.read"]"#);
    assert!(!hash.contains(body) && !held.contains(body));
    // Nor does this device's file (it holds hashes, never a token).
    let file = std::fs::read_to_string(here.dir().join(device::DEVICE_FILE)).unwrap();
    assert!(!file.contains(body) && file.contains(&hash));
    // Two apps never share a token.
    let (_, other) = host
        .sql(|c| mint(c, &here, "Laptop", &scopes(&["tasks.read"])))
        .unwrap();
    assert_ne!(token, other);
}

#[test]
fn mint_takes_exactly_the_scopes_it_is_given() {
    let (host, here) = rig();
    // `documents.text` is the app's to be given, never the core's to add.
    let (id, _) = host
        .sql(|c| mint(c, &here, "Desk", &scopes(&["tasks.read", "tasks.read"])))
        .unwrap();
    let listed = host.sql(|c| list(c, &here));
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (listed[0].id, listed[0].scopes.clone()),
        (id, scopes(&["tasks.read"]))
    );
}

#[test]
fn a_name_and_the_scopes_are_checked_and_a_refusal_leaves_nothing() {
    let (host, here) = rig();
    let refused = |name: &str, held: &[&str]| {
        let r = host.sql(|c| mint(c, &here, name, &scopes(held)));
        matches!(r, Err(Fault::Refused(_)))
    };
    assert!(refused("", &["tasks.read"]));
    assert!(refused("   ", &["tasks.read"]));
    assert!(refused("\n\t", &["tasks.read"]));
    assert!(refused(&"x".repeat(61), &["tasks.read"]));
    assert!(refused("Desk", &[]));
    assert!(refused("Desk", &["calendar.details"]));
    assert!(refused("Desk", &["tasks.read", "tasks.write"]));
    assert!(refused("Desk", &["TASKS.READ"]));
    assert_eq!(rows(&host), 0, "a refusal left a row");
    assert!(!here.dir().join(device::DEVICE_FILE).exists());
    // The longest name goes in; a name is tidied like any line of the person's words.
    let (id, _) = host
        .sql(|c| mint(c, &here, &"x".repeat(60), &scopes(&["tasks.read"])))
        .unwrap();
    assert_eq!(host.sql(|c| list(c, &here))[0].name.len(), 60);
    host.sql(|c| revoke(c, &here, id)).unwrap();
    host.sql(|c| mint(c, &here, "  Desk\n", &scopes(&["tasks.read"])))
        .unwrap();
    assert_eq!(host.sql(|c| list(c, &here))[0].name, "Desk");
    // Every scope that is offered can be given, and nothing else is offered.
    assert_eq!(SCOPES.len(), 4);
    assert!(!SCOPES.contains(&"calendar.details"));
    host.sql(|c| mint(c, &here, "All", &scopes(&SCOPES)))
        .unwrap();
}

#[test]
fn at_most_sixteen_apps() {
    let (host, here) = rig();
    for i in 0..16 {
        host.sql(|c| mint(c, &here, &format!("App {i}"), &scopes(&["tasks.read"])))
            .unwrap();
    }
    let seventeenth = host.sql(|c| mint(c, &here, "One too many", &scopes(&["tasks.read"])));
    assert!(matches!(seventeenth, Err(Fault::Refused(_))));
    assert_eq!(rows(&host), 16);
}

#[test]
fn pairings_whose_rows_have_gone_never_count_against_the_cap() {
    let (host, here) = rig();
    // Another device in a shared vault revokes sixteen apps this device paired: the rows go, the
    // pairings stay in this device's file, and the list has nothing left to revoke them by.
    for round in 0..16 {
        host.sql(|c| mint(c, &here, &format!("App {round}"), &scopes(&["tasks.read"])))
            .unwrap();
    }
    host.sql(|c| c.execute("DELETE FROM mcp_clients", []).unwrap());
    assert_eq!(device::device_state(&here).clients.len(), 16);
    assert_eq!(rows(&host), 0);
    // The next mint forgets them, and goes through.
    let (id, token) = host
        .sql(|c| mint(c, &here, "Fresh", &scopes(&["tasks.read"])))
        .unwrap();
    assert_eq!(device::device_state(&here).clients.len(), 1);
    device::set_on(&here, true).unwrap();
    assert_eq!(verified(&host, &here, &token).map(|c| c.id), Some(id));
}

#[test]
fn a_pairing_with_its_row_is_kept_when_another_app_is_minted() {
    let (host, here) = rig();
    let (first, token) = app(&host, &here, &["tasks.read"]);
    app(&host, &here, &["tasks.read"]);
    assert_eq!(device::device_state(&here).clients.len(), 2);
    assert_eq!(verified(&host, &here, &token).map(|c| c.id), Some(first));
}

#[test]
fn what_a_fault_says_never_carries_a_database_or_disk_text() {
    let secret = "SELECT secret FROM /home/bobby/vault.db";
    let db = Fault::Db(rusqlite::Error::InvalidParameterName(secret.into()));
    let disk = Fault::Disk(std::io::Error::other(secret));
    for fault in [db, disk] {
        let said = fault.to_string();
        assert!(
            !said.contains("secret") && !said.contains("/home"),
            "{said}"
        );
        assert!(said.ends_with("Try again."), "{said}");
    }
    // The detail is still there for whoever debugs it.
    assert!(format!("{:?}", Fault::Disk(std::io::Error::other(secret))).contains("secret"));
    assert_eq!(Fault::Refused("No.").to_string(), "No.");
}

#[test]
fn a_mint_whose_pairing_cannot_be_written_leaves_no_row() {
    let (host, here) = rig();
    // A directory where the device file goes: the pairing cannot be written.
    std::fs::create_dir(here.dir().join(device::DEVICE_FILE)).unwrap();
    let r = host.sql(|c| mint(c, &here, "Desk", &scopes(&["tasks.read"])));
    assert!(r.is_err());
    assert_eq!(rows(&host), 0);
}

// --- verify -------------------------------------------------------------------------------------

#[test]
fn the_right_token_verifies_with_its_scopes() {
    let (host, here) = rig();
    let (id, token) = app(&host, &here, &["tasks.read", "projects.read"]);
    let got = verified(&host, &here, &token).expect("verifies");
    assert_eq!(got.id, id);
    assert_eq!(got.scopes, scopes(&["projects.read", "tasks.read"]));
    assert!(got.holds("tasks.read") && !got.holds("documents.text"));
    assert_eq!(rechecked(&host, &here, id), Some(got));
}

#[test]
fn a_wrong_token_or_another_apps_body_does_not() {
    let (host, here) = rig();
    let (a, token_a) = app(&host, &here, &["tasks.read"]);
    let (b, token_b) = app(&host, &here, &["tasks.read"]);
    let body = |t: &str, id: i64| t.strip_prefix(&format!("pmcp_{id}_")).unwrap().to_owned();
    // One digit off.
    let mut off = token_a.clone();
    off.pop();
    off.push(if token_a.ends_with('0') { '1' } else { '0' });
    assert_eq!(verified(&host, &here, &off), None);
    // A's body under B's id, and B's under A's.
    assert_eq!(
        verified(&host, &here, &format!("pmcp_{b}_{}", body(&token_a, a))),
        None
    );
    assert_eq!(
        verified(&host, &here, &format!("pmcp_{a}_{}", body(&token_b, b))),
        None
    );
    // The two real ones still do.
    assert!(verified(&host, &here, &token_a).is_some());
    assert!(verified(&host, &here, &token_b).is_some());
}

#[test]
fn a_token_that_is_not_shaped_like_one_is_nobody() {
    let (host, here) = rig();
    let (id, token) = app(&host, &here, &["tasks.read"]);
    let body = token.strip_prefix(&format!("pmcp_{id}_")).unwrap();
    for bad in [
        String::new(),
        "pmcp_".into(),
        format!("pmcp_{id}"),
        format!("pmcp_{id}_{}", &body[..63]),
        format!("pmcp_{id}_{body}0"),
        format!("pmcp_{id}_{}", body.to_uppercase()),
        format!("pmcp_0{id}_{body}"),
        format!("pmcp_0_{body}"),
        format!("pmcp_-{id}_{body}"),
        format!("PMCP_{id}_{body}"),
        format!(" {token}"),
        format!("{token}\n"),
        format!("pmcp_99999999999999999999_{body}"),
    ] {
        assert_eq!(verified(&host, &here, &bad), None, "{bad:?}");
    }
    assert!(verified(&host, &here, &token).is_some());
}

#[test]
fn a_row_with_no_pair_from_another_device_never_verifies() {
    let (host, here) = rig();
    let (mine, my_token) = app(&host, &here, &["tasks.read"]);
    // Another computer's app in the shared store: right token, right hash, right scopes, and no pair
    // in THIS device's file.
    let (id, token) = foreign_row(&host, &["tasks.read"]);
    assert_eq!(verified(&host, &here, &token), None);
    assert_eq!(rechecked(&host, &here, id), None);
    assert!(
        !host
            .sql(|c| list(c, &here))
            .iter()
            .find(|v| v.id == id)
            .unwrap()
            .paired
    );
    // The device's own app is untouched by it.
    assert!(verified(&host, &here, &my_token).is_some());
    assert!(
        host.sql(|c| list(c, &here))
            .iter()
            .find(|v| v.id == mine)
            .unwrap()
            .paired
    );
    // Teeth: the same row WITH a pair made for it verifies, so the refusal above was the missing pair.
    let hash = sha(&token);
    device::pair(
        &here,
        PairedClient {
            id,
            scopes_hash: scopes_hash(id, &hash, &scopes(&["tasks.read"])),
            hash,
        },
    )
    .unwrap();
    assert!(verified(&host, &here, &token).is_some());
}

#[test]
fn a_row_whose_scopes_were_changed_in_the_store_never_verifies() {
    let (host, here) = rig();
    let (id, token) = app(&host, &here, &["tasks.read"]);
    for changed in [
        r#"["tasks.read","documents.text"]"#,
        r#"["documents.text"]"#,
        "[]",
        r#"["tasks.read","tasks.read"]"#,
        r#"["tasks.read","calendar.details"]"#,
        "not json",
        r#"{"tasks.read":true}"#,
    ] {
        host.sql(|c| {
            c.execute(
                "UPDATE mcp_clients SET scopes = ?2 WHERE id = ?1",
                params![id, changed],
            )
            .unwrap()
        });
        assert_eq!(verified(&host, &here, &token), None, "{changed}");
        assert_eq!(rechecked(&host, &here, id), None, "{changed}");
    }
    // Put back, it verifies again: the pair was never the thing that changed.
    host.sql(|c| {
        c.execute(
            "UPDATE mcp_clients SET scopes = '[\"tasks.read\"]' WHERE id = ?1",
            [id],
        )
        .unwrap()
    });
    assert!(verified(&host, &here, &token).is_some());
}

#[test]
fn a_row_swapped_in_the_store_never_verifies() {
    let (host, here) = rig();
    let (id, token) = app(&host, &here, &["tasks.read"]);
    host.sql(|c| {
        c.execute(
            "UPDATE mcp_clients SET token_hash = ?2 WHERE id = ?1",
            params![id, sha("other")],
        )
        .unwrap()
    });
    assert_eq!(verified(&host, &here, &token), None);
    assert_eq!(rechecked(&host, &here, id), None);
}

#[test]
fn a_lost_or_switched_off_device_file_verifies_nobody() {
    let (host, here) = rig();
    let (id, token) = app(&host, &here, &["tasks.read"]);
    assert!(verified(&host, &here, &token).is_some());
    // A copy of the store (rows and all) opened from another folder is off: the file ties its
    // entry to the folder the vault sat in.
    let copy = here.moved_to("elsewhere");
    assert_eq!(host.sql(|c| verify(c, &copy, &token)), None);
    assert_eq!(host.sql(|c| recheck(c, &copy, id)), None);
    // Off.
    device::set_on(&here, false).unwrap();
    assert_eq!(verified(&host, &here, &token), None);
    assert_eq!(rechecked(&host, &here, id), None);
    // On again: the pairing was kept, so the same token works.
    device::set_on(&here, true).unwrap();
    assert!(verified(&host, &here, &token).is_some());
    // The file lost.
    std::fs::remove_file(here.dir().join(device::DEVICE_FILE)).unwrap();
    assert_eq!(verified(&host, &here, &token), None);
    assert_eq!(rechecked(&host, &here, id), None);
    // The file corrupt.
    std::fs::write(here.dir().join(device::DEVICE_FILE), "{").unwrap();
    assert_eq!(verified(&host, &here, &token), None);
    // Switching on again over a corrupt file does not bring the old pairing back.
    device::set_on(&here, true).unwrap();
    assert_eq!(verified(&host, &here, &token), None);
}

#[test]
fn a_token_whose_row_is_gone_does_not_verify() {
    let (host, here) = rig();
    let (id, token) = app(&host, &here, &["tasks.read"]);
    host.sql(|c| {
        c.execute("DELETE FROM mcp_clients WHERE id = ?1", [id])
            .unwrap()
    });
    assert_eq!(verified(&host, &here, &token), None);
    assert_eq!(rechecked(&host, &here, id), None);
}

/// The hash comparison is the constant-time one, and the store is never asked to compare a hash: the
/// row comes by id, and the comparison happens in Rust. (A source test, since a timing difference
/// cannot be seen from a unit test.)
#[test]
fn hashes_are_compared_with_ct_eq_and_never_in_sql() {
    let source = include_str!("clients.rs");
    // The code, not this file's own words about it.
    let code: String = source
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        code.matches("ct_eq(").count() >= 3,
        "the presented hash, the row's hash and the scopes hash are each compared with ct_eq"
    );
    assert!(code.contains("oauth_loopback::ct_eq"));
    for banned in [
        "WHERE token_hash",
        "AND token_hash",
        "token_hash IN",
        "token_hash LIKE",
        "hash ==",
        "hash !=",
        "== pair.",
        "!= pair.",
        "== row_hash",
        "== &pair",
    ] {
        assert!(!code.contains(banned), "{banned}");
    }
}

// --- revoke and list ----------------------------------------------------------------------------

#[test]
fn revoking_deletes_the_row_and_the_pair_and_twice_is_fine() {
    let (host, here) = rig();
    let (id, token) = app(&host, &here, &["tasks.read"]);
    let (keep, keep_token) = app(&host, &here, &["tasks.read"]);
    host.sql(|c| revoke(c, &here, id)).unwrap();
    assert_eq!(rows(&host), 1);
    assert_eq!(verified(&host, &here, &token), None);
    assert!(device::device_state(&here)
        .clients
        .iter()
        .all(|c| c.id != id));
    // The other app is untouched.
    assert!(verified(&host, &here, &keep_token).is_some());
    assert!(device::device_state(&here)
        .clients
        .iter()
        .any(|c| c.id == keep));
    // Again, and one that never existed.
    host.sql(|c| revoke(c, &here, id)).unwrap();
    host.sql(|c| revoke(c, &here, 9999)).unwrap();
    assert_eq!(rows(&host), 1);
    // A row this device never paired (another computer's) is revoked from the store too.
    let (foreign, _) = foreign_row(&host, &["tasks.read"]);
    host.sql(|c| revoke(c, &here, foreign)).unwrap();
    assert_eq!(rows(&host), 1);
}

#[test]
fn the_list_says_which_apps_this_device_holds() {
    let (host, here) = rig();
    let (mine, _) = app(&host, &here, &["tasks.read", "projects.read"]);
    let (theirs, _) = foreign_row(&host, &["tasks.read"]);
    // A hostile row: scopes that aren't a set of offered ids.
    let hostile = host.sql(|c| {
        c.execute(
            "INSERT INTO mcp_clients(name, token_hash, scopes) VALUES ('Odd', 'x', '[\"root\"]')",
            [],
        )
        .unwrap();
        c.last_insert_rowid()
    });
    let listed = host.sql(|c| list(c, &here));
    let of = |id: i64| listed.iter().find(|v| v.id == id).unwrap();
    assert_eq!(listed.len(), 3);
    assert!(of(mine).paired);
    assert_eq!(of(mine).scopes, scopes(&["projects.read", "tasks.read"]));
    assert!(of(mine).last_used_at.is_none());
    assert!(!of(mine).created_at.is_empty());
    assert!(!of(theirs).paired);
    assert!(!of(hostile).paired);
    assert!(of(hostile).scopes.is_empty());
    // Off or lost, the list still names the apps (the person can revoke them), paired or not.
    device::set_on(&here, false).unwrap();
    assert!(host
        .sql(|c| list(c, &here))
        .iter()
        .any(|v| v.id == mine && v.paired));
}

#[test]
fn the_list_cuts_a_hostile_rows_text_to_a_screens_worth() {
    let (host, here) = rig();
    // A shared store's writer does not go through `mint`: huge name, scopes and dates.
    let big = "é".repeat(1_000_000);
    let id = host.sql(|c| {
        c.execute(
            "INSERT INTO mcp_clients(name, token_hash, scopes, created_at, last_used_at) \
             VALUES (?1, 'x', ?1, ?1, ?1)",
            [&big],
        )
        .unwrap();
        c.last_insert_rowid()
    });
    // A name of exactly the longest allowed is shown whole; one over is cut with an ellipsis.
    let exact = "n".repeat(NAME_MAX_CHARS);
    let over = "m".repeat(NAME_MAX_CHARS + 1);
    host.sql(|c| {
        for name in [&exact, &over] {
            c.execute(
                "INSERT INTO mcp_clients(name, token_hash, scopes) VALUES (?1, ?1, '[]')",
                [name],
            )
            .unwrap();
        }
    });
    let listed = host.sql(|c| list(c, &here));
    let of = |id: i64| listed.iter().find(|v| v.id == id).unwrap();
    let hostile = of(id);
    assert_eq!(hostile.name.chars().count(), NAME_MAX_CHARS + 1);
    assert!(hostile.name.ends_with('…'));
    assert!(hostile.scopes.is_empty());
    assert_eq!(hostile.created_at.chars().count(), 40);
    assert_eq!(
        hostile.last_used_at.as_deref().map(|s| s.chars().count()),
        Some(40)
    );
    assert_eq!(listed[1].name, exact);
    assert_eq!(listed[2].name, format!("{}…", &over[..NAME_MAX_CHARS]));
    // The whole list is small enough to be sent over and over.
    assert!(serde_json::to_string(&listed).unwrap().len() < 4_000);
}

// --- last_used_at -------------------------------------------------------------------------------

fn last_used(host: &TestHost, id: i64) -> Option<String> {
    host.sql(|c| {
        c.query_row(
            "SELECT last_used_at FROM mcp_clients WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
    })
}

#[test]
fn last_used_is_written_once_a_minute() {
    let (host, here) = rig();
    let (id, _) = app(&host, &here, &["tasks.read"]);
    let clock = LastUsed::default();
    let t0 = Instant::now();
    assert_eq!(last_used(&host, id), None);
    host.sql(|c| clock.touch(c, id, t0));
    assert!(last_used(&host, id).is_some(), "the first use is written");
    // Within the minute: not written again (cleared by hand to see it).
    host.sql(|c| {
        c.execute(
            "UPDATE mcp_clients SET last_used_at = NULL WHERE id = ?1",
            [id],
        )
        .unwrap()
    });
    host.sql(|c| clock.touch(c, id, t0 + Duration::from_secs(30)));
    host.sql(|c| clock.touch(c, id, t0 + Duration::from_secs(59)));
    assert_eq!(last_used(&host, id), None);
    // A minute on: written, and the minute starts again from there.
    host.sql(|c| clock.touch(c, id, t0 + Duration::from_secs(61)));
    assert!(last_used(&host, id).is_some());
    host.sql(|c| {
        c.execute(
            "UPDATE mcp_clients SET last_used_at = NULL WHERE id = ?1",
            [id],
        )
        .unwrap()
    });
    host.sql(|c| clock.touch(c, id, t0 + Duration::from_secs(100)));
    assert_eq!(last_used(&host, id), None);
    // Another app has its own minute.
    let (other, _) = app(&host, &here, &["tasks.read"]);
    host.sql(|c| clock.touch(c, other, t0 + Duration::from_secs(100)));
    assert!(last_used(&host, other).is_some());
}

#[test]
fn a_failed_last_used_write_is_ignored() {
    let conn = Connection::open_in_memory().unwrap();
    // No table at all: the write fails, and nothing panics or is reported to the app.
    LastUsed::default().touch(&conn, 1, Instant::now());
}
