// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The one way a copy of the store leaves this computer (Mods spec D4, §1.11, §1.14).
//!
//! A copy is a `VACUUM INTO` snapshot of the live store, and it leaves in three kinds of file: a
//! `.pmbackup` (typed, scheduled, or pushed to Proton or Google Drive: `commands::backups`), and
//! the same-machine export zip (`commands::export`). Each of them goes through
//! [`snapshot_for_leaving`] and nothing else, so what can't leave is decided in one place:
//!
//! - **Always out:** `mod_secrets` (the user's typed credentials; they re-enter them after a
//!   restore, as they do their keychain ones), `mcp_clients` (connected apps' token hashes: a
//!   device's, never a vault's), `mod_doc_data` and cache-class `mod_kv` (rebuildable), and every
//!   mod installed from a developer's test folder (`installed_from = 'folder'`: nothing a restore
//!   could install again, and unpublished work), with the rows that hang off it.
//! - **Out unless the user keeps it** (`BACKUP_MOD_DATA_KEY`, default on): content-class `mod_kv`,
//!   the mods' own saved data. Off, a restore brings those mods back empty.
//! - **Never touched:** `mod_overlay` and the gift rows (`mod_ledger` and the tasks and milestones
//!   they name). The overlay is calendar entries a mod published into a core table; it is derived
//!   and carries nothing private, so it stays. Gifts are core tasks and milestones that are the
//!   user's own whatever made them (I-16, I-29). Neither is "a mod's data" in the sense the toggle
//!   or D4 means.
//!
//! The strip runs on the SNAPSHOT's own connection, never on the live store. A plain `DELETE`
//! leaves the row's bytes in the page (and a deleted overflow page on the free list), and nothing in
//! PM sets `secure_delete`, so the strip sets it, deletes, and `VACUUM`s the copy before anything is
//! packed; the test reads every decrypted page, free ones included, to prove the values are gone.
//! If any step fails the snapshot is deleted and the caller gets an error: a backup that might hold
//! a secret is never written.

use std::path::Path;

use rusqlite::Connection;

use super::manifest::BackupMod;
use crate::db;
use crate::error::{Error, Result};
use crate::mods::store;
use crate::AppState;

/// `settings` key of "Also save mod data in my backups" (Backup settings): `"true"` or `"false"`,
/// on when absent. Per vault (it travels in the store's own `settings`), like the schedule.
pub const BACKUP_MOD_DATA_KEY: &str = "backup_mod_data";

/// The tables a mod's row owns through `mod_id REFERENCES mods(id) ON DELETE CASCADE`. A folder
/// mod's rows in each go before the mod's own row, so the strip never depends on the cascade
/// firing on a fresh connection; the guard test checks this list against the schema, so a future
/// table that references `mods` can't be missed.
pub(crate) const MOD_CHILD_TABLES: [&str; 6] = [
    "mod_grants",
    "mod_kv",
    "mod_doc_data",
    "mod_secrets",
    "mod_overlay",
    "ai_reservations",
];

/// Whether this vault keeps the mods' saved data in its copies (the toggle, default on).
pub fn keep_mod_data(state: &AppState) -> Result<bool> {
    let conn = state.conn()?;
    db::get_bool(&conn, BACKUP_MOD_DATA_KEY, true)
}

/// Snapshot the live store into `dest` (a file that doesn't exist yet) and strip the snapshot: the
/// only way a copy of the store leaves (module doc). `key_hex` is the store's key, which the caller
/// resolves FIRST so a missing key fails before any copy exists. The DB lock is held for the
/// `VACUUM INTO` only; the strip is file work on the snapshot and runs after it is released. Returns
/// the mods the snapshot holds, for the manifest. Blocking: call inside `spawn_blocking`.
pub(crate) fn snapshot_for_leaving(
    state: &AppState,
    dest: &Path,
    key_hex: &str,
    keep_content: bool,
) -> Result<Vec<BackupMod>> {
    snapshot_with(
        |dest| {
            let conn = state.conn()?;
            crate::vault::migrate::vacuum_into(&conn, dest)
        },
        dest,
        key_hex,
        keep_content,
    )
}

/// [`snapshot_for_leaving`] with the first step passed in, so a test can supply a bare connection.
fn snapshot_with(
    take: impl FnOnce(&Path) -> Result<()>,
    dest: &Path,
    key_hex: &str,
    keep_content: bool,
) -> Result<Vec<BackupMod>> {
    take(dest)?;
    strip_snapshot(dest, key_hex, keep_content).map_err(|e| {
        // Nothing half-stripped is left to be picked up by mistake.
        discard(dest);
        Error::Other(format!(
            "nothing was saved: PM couldn't take the mods' private data out of the copy of your \
             data ({e})"
        ))
    })
}

/// Remove a snapshot and any sidecar SQLite may have made beside it.
fn discard(dest: &Path) {
    let name = dest.to_string_lossy();
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(format!("{name}{suffix}"));
    }
}

/// Open the snapshot, strip it, `VACUUM` it, and read back its mod list.
fn strip_snapshot(path: &Path, key_hex: &str, keep_content: bool) -> Result<Vec<BackupMod>> {
    let conn = db::open_snapshot(path, key_hex)?;
    let mods = strip(&conn, keep_content)?;
    conn.close()
        .map_err(|(_, e)| Error::Other(format!("the copy couldn't be closed cleanly: {e}")))?;
    Ok(mods)
}

fn pragma_i64(conn: &Connection, name: &str) -> Result<i64> {
    Ok(conn.query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))?)
}

/// The strip itself, on an open connection to a snapshot. Every step is checked, because a step
/// that quietly did nothing (a pragma a build ignores, foreign keys off, a vacuum that didn't
/// shrink) is exactly how a secret would stay in the file.
fn strip(conn: &Connection, keep_content: bool) -> Result<Vec<BackupMod>> {
    // Freed bytes are overwritten with zeros, not just unlinked.
    conn.execute_batch("PRAGMA secure_delete = ON")?;
    if pragma_i64(conn, "secure_delete")? != 1 {
        return Err(Error::Other("secure_delete didn't turn on".into()));
    }
    // The cascade is the second line of defence, so make sure it can fire on this connection.
    conn.execute_batch("PRAGMA foreign_keys = ON")?;
    if pragma_i64(conn, "foreign_keys")? != 1 {
        return Err(Error::Other("foreign keys didn't turn on".into()));
    }
    // The vacuum must land in the one file that is packed: no WAL beside it.
    let mode: String = conn.query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("delete") {
        return Err(Error::Other(format!("journal mode stayed {mode}")));
    }

    let folder = format!(
        "SELECT id FROM mods WHERE installed_from = '{}'",
        store::FROM_FOLDER
    );
    let tx = conn.unchecked_transaction()?;
    for table in MOD_CHILD_TABLES {
        tx.execute(
            &format!("DELETE FROM {table} WHERE mod_id IN ({folder})"),
            [],
        )?;
    }
    tx.execute(
        &format!(
            "DELETE FROM mods WHERE installed_from = '{}'",
            store::FROM_FOLDER
        ),
        [],
    )?;
    tx.execute("DELETE FROM mod_secrets", [])?;
    tx.execute("DELETE FROM mcp_clients", [])?;
    tx.execute("DELETE FROM mod_doc_data", [])?;
    tx.execute(
        &format!("DELETE FROM mod_kv WHERE class = '{}'", store::KV_CACHE),
        [],
    )?;
    if !keep_content {
        tx.execute("DELETE FROM mod_kv", [])?;
    }
    tx.commit()?;

    // Prove the rows are gone before the file is rewritten, so a delete that matched nothing it
    // should have is an error and not a hope.
    let left: i64 = {
        let mut sql = format!(
            "SELECT (SELECT count(*) FROM mod_secrets) + (SELECT count(*) FROM mcp_clients) \
             + (SELECT count(*) FROM mod_doc_data) \
             + (SELECT count(*) FROM mods WHERE installed_from = '{}') \
             + (SELECT count(*) FROM mod_kv WHERE class = '{}')",
            store::FROM_FOLDER,
            store::KV_CACHE
        );
        if !keep_content {
            sql.push_str(" + (SELECT count(*) FROM mod_kv)");
        }
        conn.query_row(&sql, [], |r| r.get(0))?
    };
    if left != 0 {
        return Err(Error::Other(format!("{left} rows were still there")));
    }

    let mods = read_mod_list(conn)?;

    // Rewrite the whole file from what is live, dropping every free page the deletes made.
    conn.execute_batch("VACUUM")?;
    let free = pragma_i64(conn, "freelist_count")?;
    if free != 0 {
        return Err(Error::Other(format!(
            "{free} free pages survived the vacuum"
        )));
    }
    Ok(mods)
}

/// The mods the (already stripped) snapshot holds, for the manifest.
fn read_mod_list(conn: &Connection) -> Result<Vec<BackupMod>> {
    let mut stmt =
        conn.prepare("SELECT id, version, tier, installed_from, signed FROM mods ORDER BY id")?;
    let rows = stmt.query_map([], |r| {
        Ok(BackupMod {
            id: r.get(0)?,
            version: r.get(1)?,
            tier: r.get(2)?,
            source: r.get(3)?,
            signed: r.get::<_, i64>(4)? != 0,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use std::path::PathBuf;

    // ---- reading a SQLCipher file page by page -----------------------------------------------
    //
    // Grepping an encrypted file for a plaintext value proves nothing (it never matches), and
    // `sqlcipher_export` or a SELECT only show LIVE rows. To see what is really in the file (free
    // pages and the unused tail of a page included) every page is decrypted here. SQLCipher 4 with
    // a raw `x'..'` key and the pinned profile (`db::open_keyed`) is: 4096-byte pages, the last 80
    // bytes of each a reserve of a 16-byte IV then a 64-byte HMAC, AES-256-CBC over the rest of the
    // page with the key itself as the cipher key (no KDF for a raw key), and on page 1 the first 16
    // bytes the salt, in the clear, with the encrypted part starting after them. The AES below is
    // checked against FIPS-197's own vector, and the first page must come out as an SQLite header
    // (its page size field reads 4096), so a wrong layout or a wrong cipher fails loudly instead of
    // reading as "nothing found".

    const PAGE: usize = 4096;
    const RESERVE: usize = 80;

    fn gmul(mut a: u8, mut b: u8) -> u8 {
        let mut p = 0;
        while b != 0 {
            if b & 1 != 0 {
                p ^= a;
            }
            let hi = a & 0x80;
            a <<= 1;
            if hi != 0 {
                a ^= 0x1b;
            }
            b >>= 1;
        }
        p
    }

    fn sboxes() -> ([u8; 256], [u8; 256]) {
        let mut sbox = [0u8; 256];
        let (mut p, mut q) = (1u8, 1u8);
        loop {
            p = p ^ (p << 1) ^ if p & 0x80 != 0 { 0x1b } else { 0 };
            q ^= q << 1;
            q ^= q << 2;
            q ^= q << 4;
            if q & 0x80 != 0 {
                q ^= 0x09;
            }
            let x = q ^ q.rotate_left(1) ^ q.rotate_left(2) ^ q.rotate_left(3) ^ q.rotate_left(4);
            sbox[p as usize] = x ^ 0x63;
            if p == 1 {
                break;
            }
        }
        sbox[0] = 0x63;
        let mut inv = [0u8; 256];
        for (i, s) in sbox.iter().enumerate() {
            inv[*s as usize] = i as u8;
        }
        (sbox, inv)
    }

    struct Aes256 {
        round_keys: [[u8; 16]; 15],
        inv_sbox: [u8; 256],
    }

    impl Aes256 {
        fn new(key: &[u8; 32]) -> Self {
            let (sbox, inv_sbox) = sboxes();
            let mut w = [[0u8; 4]; 60];
            for i in 0..8 {
                w[i].copy_from_slice(&key[i * 4..i * 4 + 4]);
            }
            let mut rcon = 1u8;
            for i in 8..60 {
                let mut t = w[i - 1];
                if i % 8 == 0 {
                    t = [
                        sbox[t[1] as usize] ^ rcon,
                        sbox[t[2] as usize],
                        sbox[t[3] as usize],
                        sbox[t[0] as usize],
                    ];
                    rcon = gmul(rcon, 2);
                } else if i % 8 == 4 {
                    t = [
                        sbox[t[0] as usize],
                        sbox[t[1] as usize],
                        sbox[t[2] as usize],
                        sbox[t[3] as usize],
                    ];
                }
                for j in 0..4 {
                    w[i][j] = w[i - 8][j] ^ t[j];
                }
            }
            let mut round_keys = [[0u8; 16]; 15];
            for (r, rk) in round_keys.iter_mut().enumerate() {
                for c in 0..4 {
                    rk[c * 4..c * 4 + 4].copy_from_slice(&w[r * 4 + c]);
                }
            }
            Self {
                round_keys,
                inv_sbox,
            }
        }

        fn decrypt_block(&self, block: &[u8]) -> [u8; 16] {
            let mut s = [0u8; 16];
            s.copy_from_slice(block);
            let add = |s: &mut [u8; 16], r: usize| {
                for (b, k) in s.iter_mut().zip(self.round_keys[r].iter()) {
                    *b ^= k;
                }
            };
            add(&mut s, 14);
            for round in (0..14).rev() {
                // InvShiftRows: row r (bytes r, r+4, r+8, r+12) rotates right by r.
                let old = s;
                for c in 0..4 {
                    for r in 0..4 {
                        s[((c + r) % 4) * 4 + r] = old[c * 4 + r];
                    }
                }
                for b in s.iter_mut() {
                    *b = self.inv_sbox[*b as usize];
                }
                add(&mut s, round);
                if round != 0 {
                    for c in 0..4 {
                        let col = [s[c * 4], s[c * 4 + 1], s[c * 4 + 2], s[c * 4 + 3]];
                        s[c * 4] = gmul(col[0], 14)
                            ^ gmul(col[1], 11)
                            ^ gmul(col[2], 13)
                            ^ gmul(col[3], 9);
                        s[c * 4 + 1] = gmul(col[0], 9)
                            ^ gmul(col[1], 14)
                            ^ gmul(col[2], 11)
                            ^ gmul(col[3], 13);
                        s[c * 4 + 2] = gmul(col[0], 13)
                            ^ gmul(col[1], 9)
                            ^ gmul(col[2], 14)
                            ^ gmul(col[3], 11);
                        s[c * 4 + 3] = gmul(col[0], 11)
                            ^ gmul(col[1], 13)
                            ^ gmul(col[2], 9)
                            ^ gmul(col[3], 14);
                    }
                }
            }
            s
        }

        /// AES-256-CBC, no padding.
        fn cbc_decrypt(&self, iv: &[u8], data: &[u8]) -> Vec<u8> {
            assert_eq!(data.len() % 16, 0);
            let mut prev = [0u8; 16];
            prev.copy_from_slice(iv);
            let mut out = Vec::with_capacity(data.len());
            for block in data.chunks(16) {
                let plain = self.decrypt_block(block);
                out.extend(plain.iter().zip(prev.iter()).map(|(a, b)| a ^ b));
                prev.copy_from_slice(block);
            }
            out
        }
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }

    /// Every page of the SQLCipher file at `path`, decrypted and concatenated: the live pages, the
    /// free pages, and the unused bytes inside each.
    fn decrypted_file(path: &Path, key_hex: &str) -> Vec<u8> {
        let raw = std::fs::read(path).unwrap();
        assert!(
            !raw.is_empty() && raw.len().is_multiple_of(PAGE),
            "whole pages only"
        );
        let mut key = [0u8; 32];
        key.copy_from_slice(&unhex(key_hex));
        let aes = Aes256::new(&key);
        let mut out = Vec::with_capacity(raw.len());
        for (n, page) in raw.chunks(PAGE).enumerate() {
            let iv = &page[PAGE - RESERVE..PAGE - RESERVE + 16];
            let start = if n == 0 { 16 } else { 0 };
            let plain = aes.cbc_decrypt(iv, &page[start..PAGE - RESERVE]);
            if n == 0 {
                // The header: its page-size field (bytes 16..18 of the file) reads 4096.
                assert_eq!(&plain[..2], &[0x10, 0x00], "page 1 didn't decrypt");
                out.extend_from_slice(b"SQLite format 3\0");
            }
            out.extend(plain);
            out.extend_from_slice(&page[PAGE - RESERVE..]);
        }
        out
    }

    fn contains(haystack: &[u8], needle: &str) -> bool {
        haystack
            .windows(needle.len())
            .any(|w| w == needle.as_bytes())
    }

    #[test]
    fn the_page_reader_decrypts_what_sqlcipher_wrote() {
        // FIPS-197 C.3: the AES under the reader.
        let key: Vec<u8> = (0u8..32).collect();
        let aes = Aes256::new(key.as_slice().try_into().unwrap());
        let ct = unhex("8ea2b7ca516745bfeafc49904b496089");
        assert_eq!(
            aes.decrypt_block(&ct).to_vec(),
            unhex("00112233445566778899aabbccddeeff")
        );
        // And a real store: a value written through SQLCipher is found in the decrypted file, and
        // is not in the raw bytes.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pm.sqlite");
        let key_hex = fresh_key();
        let marker = fresh_marker("reader");
        {
            let conn = db::open(&path, &key_hex).unwrap();
            conn.execute(
                "INSERT INTO settings(key, value) VALUES ('probe', ?1)",
                [&marker],
            )
            .unwrap();
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .unwrap();
        }
        let raw = std::fs::read(&path).unwrap();
        assert!(!contains(&raw, &marker), "the file is encrypted");
        assert!(contains(&decrypted_file(&path, &key_hex), &marker));
    }

    // ---- the fixtures --------------------------------------------------------------------------

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// A key and distinctive values made for each run, so no secret, not even a test one, is
    /// committed.
    fn fresh_key() -> String {
        hex(&crate::vault::random_array::<32>().unwrap())
    }

    fn fresh_marker(what: &str) -> String {
        format!(
            "{what}-{}",
            hex(&crate::vault::random_array::<8>().unwrap())
        )
    }

    /// A value spread over several pages (so part of it lives on overflow pages, which a delete
    /// puts on the free list), each page carrying the marker.
    fn bulky(marker: &str) -> String {
        marker.repeat(4096 / marker.len() * 3)
    }

    struct Planted {
        secret: String,
        cache: String,
        doc: String,
        folder_content: String,
        folder_secret: String,
        mcp: String,
        content: String,
        overlay: String,
    }

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn add_mod(conn: &Connection, id: &str, from: &str, signed: bool, state: &str) {
        let sha = (from != "folder").then_some(SHA);
        conn.execute(
            "INSERT INTO mods(id, name, version, tier, author, source_repo, source_commit, signed, \
                              install_id, installed_from, package_sha256, manifest, files, state) \
             VALUES (?1, ?2, '1.2.3', ?3, 'someone', 'itsAtlas-org/PM-Mods', \
                     '0123456789abcdef0123456789abcdef01234567', ?4, '0123456789abcdef', ?5, ?6, \
                     '{}', '[]', ?7)",
            params![
                id,
                format!("{id} name"),
                if signed { "first" } else { "third" },
                signed as i64,
                from,
                sha,
                state
            ],
        )
        .unwrap();
    }

    /// A live store with three mods (a catalogue one, an unsigned file one, a folder one) holding a
    /// distinctive value in every place D4 names, plus a value that is meant to stay.
    fn plant(conn: &Connection) -> Planted {
        let p = Planted {
            secret: fresh_marker("mod-secret"),
            cache: fresh_marker("mod-cache"),
            doc: fresh_marker("mod-doc"),
            folder_content: fresh_marker("folder-content"),
            folder_secret: fresh_marker("folder-secret"),
            mcp: fresh_marker("mcp-token"),
            content: fresh_marker("mod-content"),
            overlay: fresh_marker("overlay"),
        };
        add_mod(conn, "atlas.board", "catalogue", true, "active");
        add_mod(conn, "friend.tool", "file", false, "paused");
        add_mod(conn, "dev.sandbox", "folder", false, "active");
        conn.execute(
            "INSERT INTO documents(vault_path, content_hash, title) VALUES ('d.md', 'h1', 'doc')",
            [],
        )
        .unwrap();
        let doc_id = conn.last_insert_rowid();
        let put = |sql: &str, args: &[&dyn rusqlite::ToSql]| {
            conn.execute(sql, args).unwrap();
        };
        put(
            "INSERT INTO mod_secrets(mod_id, name, host, header, value) \
             VALUES ('atlas.board', 'key', 'example.org', 'Authorization', ?1)",
            &[&bulky(&p.secret)],
        );
        put(
            "INSERT INTO mod_kv(mod_id, key, class, value) VALUES ('atlas.board', 'layout', 'cache', ?1)",
            &[&bulky(&p.cache)],
        );
        put(
            "INSERT INTO mod_kv(mod_id, key, class, value) VALUES ('atlas.board', 'board', 'content', ?1)",
            &[&bulky(&p.content)],
        );
        put(
            "INSERT INTO mod_doc_data(mod_id, document_id, key, value) \
             VALUES ('atlas.board', ?1, 'k', ?2)",
            &[&doc_id, &bulky(&p.doc)],
        );
        put(
            "INSERT INTO mod_kv(mod_id, key, class, value) VALUES ('dev.sandbox', 'board', 'content', ?1)",
            &[&bulky(&p.folder_content)],
        );
        put(
            "INSERT INTO mod_secrets(mod_id, name, host, header, value) \
             VALUES ('dev.sandbox', 'key', 'example.org', 'X-Key', ?1)",
            &[&bulky(&p.folder_secret)],
        );
        put(
            "INSERT INTO mcp_clients(name, token_hash) VALUES ('a client', ?1)",
            &[&bulky(&p.mcp)],
        );
        put(
            "INSERT INTO mod_overlay(mod_id, id, date, label) \
             VALUES ('atlas.board', 'e1', '2026-12-01', ?1)",
            &[&p.overlay],
        );
        p
    }

    /// A live store with the planted rows, as an `AppState` the way a backup reaches it.
    fn live_state(dir: &Path, key_hex: &str) -> (AppState, Planted) {
        let conn = db::open(&dir.join("live.sqlite"), key_hex).unwrap();
        let planted = plant(&conn);
        let sidecar = crate::sidecar::SidecarManager::new(crate::sidecar::SidecarPaths {
            source_dir: dir.join("sidecar"),
            venv_dir: dir.join("venv"),
        });
        (
            AppState::new(Some(conn), None, sidecar, None, None),
            planted,
        )
    }

    fn snapshot_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("pm.sqlite");
        (dir, dest)
    }

    // ---- the tests -----------------------------------------------------------------------------

    #[test]
    fn the_scanner_can_fail_a_plain_snapshot_and_a_plain_delete_both_leak() {
        // The control for every test below: with the strip absent the values ARE in the decrypted
        // file, so "not found" later means something. First a bare VACUUM INTO (no strip at all)...
        let dir = tempfile::tempdir().unwrap();
        let key = fresh_key();
        let (state, p) = live_state(dir.path(), &key);
        let (_d, dest) = snapshot_dir();
        {
            let conn = state.conn().unwrap();
            crate::vault::migrate::vacuum_into(&conn, &dest).unwrap();
        }
        let bytes = decrypted_file(&dest, &key);
        for (what, v) in [
            ("secret", &p.secret),
            ("cache", &p.cache),
            ("doc data", &p.doc),
            ("folder mod content", &p.folder_content),
            ("folder mod secret", &p.folder_secret),
            ("mcp token", &p.mcp),
        ] {
            assert!(contains(&bytes, v), "{what} is in an unstripped snapshot");
        }
        // ...then a plain DELETE of the same rows, secure_delete explicitly off and no VACUUM: the
        // bytes stay in the file, which is why the strip needs both.
        {
            let conn = db::open_snapshot(&dest, &key).unwrap();
            conn.execute_batch(
                "PRAGMA secure_delete = OFF; DELETE FROM mod_secrets; DELETE FROM mcp_clients; \
                 DELETE FROM mod_doc_data; DELETE FROM mod_kv;",
            )
            .unwrap();
        }
        let bytes = decrypted_file(&dest, &key);
        assert!(
            contains(&bytes, &p.secret),
            "a plain delete leaves the secret behind"
        );
        assert!(contains(&bytes, &p.mcp), "and the token");
    }

    #[test]
    fn a_stripped_snapshot_holds_none_of_the_values_in_any_page() {
        let dir = tempfile::tempdir().unwrap();
        let key = fresh_key();
        let (state, p) = live_state(dir.path(), &key);
        let (_d, dest) = snapshot_dir();

        let mods = snapshot_for_leaving(&state, &dest, &key, true).unwrap();

        let bytes = decrypted_file(&dest, &key);
        for (what, v) in [
            ("mod secret", &p.secret),
            ("cache-class storage", &p.cache),
            ("per-document data", &p.doc),
            ("a folder mod's content", &p.folder_content),
            ("a folder mod's secret", &p.folder_secret),
            ("a connected app's token", &p.mcp),
        ] {
            assert!(!contains(&bytes, v), "{what} reached the file");
        }
        // What is meant to stay is there (so the scanner and the strip aren't both blind).
        assert!(
            contains(&bytes, &p.content),
            "content-class storage is kept"
        );
        assert!(
            contains(&bytes, &p.overlay),
            "the calendar overlay is not mod-private data"
        );
        assert!(
            !dest.with_file_name("pm.sqlite-wal").exists()
                && !dest.with_file_name("pm.sqlite-journal").exists(),
            "the one file is the whole snapshot"
        );
        // The list is the stripped snapshot's: no folder mod, the others as installed.
        assert_eq!(
            mods,
            vec![
                BackupMod {
                    id: "atlas.board".into(),
                    version: "1.2.3".into(),
                    tier: "first".into(),
                    source: "catalogue".into(),
                    signed: true,
                },
                BackupMod {
                    id: "friend.tool".into(),
                    version: "1.2.3".into(),
                    tier: "third".into(),
                    source: "file".into(),
                    signed: false,
                },
            ]
        );
        // The live store was not touched.
        let live = state.conn().unwrap();
        let count = |sql: &str| live.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(count("SELECT count(*) FROM mod_secrets"), 2);
        assert_eq!(count("SELECT count(*) FROM mcp_clients"), 1);
        assert_eq!(count("SELECT count(*) FROM mods"), 3);
    }

    #[test]
    fn the_toggle_off_drops_content_rows_and_on_keeps_them() {
        let key = fresh_key();
        let dir = tempfile::tempdir().unwrap();
        let (state, p) = live_state(dir.path(), &key);
        for keep in [true, false] {
            let (_d, dest) = snapshot_dir();
            snapshot_for_leaving(&state, &dest, &key, keep).unwrap();
            let bytes = decrypted_file(&dest, &key);
            assert_eq!(contains(&bytes, &p.content), keep, "keep_content = {keep}");
            // The mods themselves come back either way, so a restore finds them (empty).
            let conn = db::open_snapshot(&dest, &key).unwrap();
            let mods: i64 = conn
                .query_row("SELECT count(*) FROM mods", [], |r| r.get(0))
                .unwrap();
            assert_eq!(mods, 2);
            let kv: i64 = conn
                .query_row("SELECT count(*) FROM mod_kv", [], |r| r.get(0))
                .unwrap();
            assert_eq!(kv, if keep { 1 } else { 0 });
        }
    }

    #[test]
    fn the_setting_defaults_on_and_is_read_from_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _) = live_state(dir.path(), &fresh_key());
        assert!(
            keep_mod_data(&state).unwrap(),
            "on until the user turns it off"
        );
        db::set_bool(&state.conn().unwrap(), BACKUP_MOD_DATA_KEY, false).unwrap();
        assert!(!keep_mod_data(&state).unwrap());
        db::set_bool(&state.conn().unwrap(), BACKUP_MOD_DATA_KEY, true).unwrap();
        assert!(keep_mod_data(&state).unwrap());
    }

    #[test]
    fn a_strip_that_fails_fails_the_snapshot_and_leaves_no_file() {
        let key = fresh_key();
        // A wrong key can't open the snapshot.
        let dir = tempfile::tempdir().unwrap();
        let (state, _) = live_state(dir.path(), &key);
        let (_d, dest) = snapshot_dir();
        let err = snapshot_for_leaving(&state, &dest, &fresh_key(), true).unwrap_err();
        assert!(err.to_string().contains("nothing was saved"), "{err}");
        assert!(
            !dest.exists(),
            "no snapshot that might hold a secret is left"
        );

        // A store missing one of the tables can't be proved clean either.
        let dir = tempfile::tempdir().unwrap();
        let (state, _) = live_state(dir.path(), &key);
        {
            let conn = state.conn().unwrap();
            conn.execute_batch("DROP TABLE mcp_clients").unwrap();
        }
        let (_d, dest) = snapshot_dir();
        let err = snapshot_for_leaving(&state, &dest, &key, true).unwrap_err();
        assert!(err.to_string().contains("nothing was saved"), "{err}");
        assert!(!dest.exists());
    }

    #[test]
    fn a_folder_mods_rows_are_gone_from_every_table_that_hangs_off_mods() {
        let key = fresh_key();
        let dir = tempfile::tempdir().unwrap();
        let (state, _) = live_state(dir.path(), &key);
        let (_d, dest) = snapshot_dir();
        snapshot_for_leaving(&state, &dest, &key, true).unwrap();
        let conn = db::open_snapshot(&dest, &key).unwrap();
        for table in MOD_CHILD_TABLES {
            let orphans: i64 = conn
                .query_row(
                    &format!("SELECT count(*) FROM {table} WHERE mod_id = 'dev.sandbox'"),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(orphans, 0, "{table} still holds the folder mod");
        }
        // Every table whose rows hang off `mods` is named in the strip's list.
        let live = state.conn().unwrap();
        let tables: Vec<String> = live
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let mut referencing = Vec::new();
        for t in tables {
            let refs: i64 = live
                .query_row(
                    "SELECT count(*) FROM pragma_foreign_key_list(?1) WHERE \"table\" = 'mods'",
                    [&t],
                    |r| r.get(0),
                )
                .unwrap();
            if refs > 0 {
                referencing.push(t);
            }
        }
        referencing.sort();
        let mut named: Vec<String> = MOD_CHILD_TABLES.iter().map(|t| t.to_string()).collect();
        named.sort();
        assert_eq!(
            referencing, named,
            "a table that references mods needs the strip's delete"
        );
    }

    #[test]
    fn a_backup_made_through_the_strip_restores_with_its_mods_and_without_their_secrets() {
        // The whole path a backup takes: strip the snapshot, pack it, restore the archive. What
        // comes out is the store the strip left (the mods, their state, the kept content) and
        // none of what it took.
        use crate::backup::pack::{pack, PackInputs};
        use crate::backup::restore::restore;
        let dir = tempfile::tempdir().unwrap();
        let key = fresh_key();
        let (state, p) = live_state(dir.path(), &key);
        let root = dir.path().join("vault");
        std::fs::create_dir_all(&root).unwrap();
        let meta = crate::vault::ensure_device_meta(&root).unwrap();
        let (_snap_dir, snap) = snapshot_dir();
        let mods = snapshot_for_leaving(&state, &snap, &key, true).unwrap();
        let archive = dir.path().join("b.pmbackup");
        let cancel = std::sync::atomic::AtomicBool::new(false);
        pack(
            PackInputs {
                vault_root: root.clone(),
                db_snapshot: snap,
                markdown_dir: root.join("vault"),
                meta,
                db_key_hex: crate::secret::Secret::from(key.clone()),
                app_version: "test".into(),
                created_at: "2026-11-11T00:00:00Z".into(),
                mods_index: None,
                mods: Some(mods),
            },
            &archive,
            "correct-horse",
            |_, _| {},
            &cancel,
        )
        .unwrap();

        let target = dir.path().join("restored");
        let outcome = restore(&archive, "correct-horse", &target, |_, _| {}, &cancel).unwrap();

        // The restored store lists the two mods that were stripped to, in the states they had.
        let listed: Vec<(&str, &str)> = outcome
            .mods
            .iter()
            .map(|m| (m.id.as_str(), m.state.as_str()))
            .collect();
        assert_eq!(
            listed,
            [("atlas.board", "active"), ("friend.tool", "paused")]
        );
        // And the file itself, read page by page, has none of what must not leave.
        let bytes = decrypted_file(&target.join("pm.sqlite"), &key);
        for v in [
            &p.secret,
            &p.cache,
            &p.doc,
            &p.folder_content,
            &p.folder_secret,
            &p.mcp,
        ] {
            assert!(!contains(&bytes, v));
        }
        assert!(contains(&bytes, &p.content));
    }

    // ---- every site that lets a copy leave -------------------------------------------------------

    /// Production code of a source file: everything before its test module, less comment lines
    /// (the docs name `VACUUM INTO` where they explain it).
    fn production(source: &str) -> String {
        source
            .split("#[cfg(test)]")
            .next()
            .unwrap()
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn every_site_that_copies_the_store_out_goes_through_the_strip() {
        // The two backup sites (the typed one and the shared run behind scheduled, Proton and Drive
        // backups) take their snapshot through `snapshot_stripped`, which is the one caller of
        // `snapshot_for_leaving` in that file; the export calls it directly. None of them calls
        // `vacuum_into` itself.
        let backups = production(include_str!("../commands/backups.rs"));
        let export = production(include_str!("../commands/export.rs"));
        assert_eq!(
            backups.matches("snapshot_stripped(").count(),
            3,
            "its definition, create_local_backup and run_backup"
        );
        assert_eq!(backups.matches("snapshot_for_leaving(").count(), 1);
        assert_eq!(
            export.matches("snapshot_for_leaving(").count(),
            1,
            "export_all_data"
        );
        for (name, src) in [("backups.rs", &backups), ("export.rs", &export)] {
            assert!(
                !src.contains("vacuum_into("),
                "{name} snapshots without the strip"
            );
            assert!(
                !src.contains("VACUUM INTO"),
                "{name} snapshots without the strip"
            );
        }
        // And no other file in the app does it: a new site has to be added here on purpose.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let allowed = [
            "vault/migrate.rs", // the migration's own rollback copy, never leaves the vault folder
            "backup/strip.rs",
            "backup/restore.rs", // test fixtures
        ];
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let rel = path
                        .strip_prefix(&root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    if allowed.contains(&rel.as_str()) {
                        continue;
                    }
                    let text = std::fs::read_to_string(&path).unwrap();
                    let prod = production(&text);
                    assert!(
                        !prod.contains("vacuum_into(") && !prod.contains("VACUUM INTO"),
                        "{rel} takes a snapshot of the store: route it through backup::strip"
                    );
                }
            }
        }
    }
}
