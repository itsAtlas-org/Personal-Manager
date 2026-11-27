// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Restore a `.pmbackup` — the inverse of [`super::pack`], with a hard invariant: a
//! wrong passphrase, a truncated/tampered archive, a version mismatch, or a crash
//! mid-restore must NEVER touch the live vault. Everything materializes into a temp
//! staging directory and is validated (the DB opens with the embedded key and passes an
//! integrity check; the metadata matches the manifest) before it is promoted, by an
//! atomic rename, to a fresh target directory. Switching the active profile to the
//! restored vault is a separate, explicit step the caller performs afterwards.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305};

use crate::error::{Error, Result};
use crate::secret::Secret;
use crate::vault::kdf;
use crate::{db, vault};

use super::{bundle, format, manifest, BackupPhase, ProgressReader};

/// Cap on the in-memory manifest read — a manifest is tiny JSON, so anything larger is a
/// decompression bomb or corruption. The bundle framing caps the entry at its declared
/// `len`, so rejecting an oversized `len` is a sufficient guard.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

/// Headroom the unpack leaves free on the restore volume. On Windows the vault usually sits on the
/// system volume, where driving free space to literally zero destabilises the page file and every
/// other running app — so a restore that would take the last byte should stop and say so rather than
/// take the machine down with it. Small next to any real vault, large enough to keep the OS healthy.
const RESTORE_FREE_SPACE_RESERVE: u64 = 1024 * 1024 * 1024;

/// Whether `len` more bytes still fit inside `budget` after `unpacked`. Pure so the arithmetic — the
/// part that has to survive a hostile `len` — is testable without a disk. `None` means the free-space
/// probe had no answer, which reads as "no budget", exactly as before the budget existed.
fn fits(unpacked: u64, len: u64, budget: Option<u64>) -> bool {
    match budget {
        Some(budget) => unpacked.saturating_add(len) <= budget,
        None => true,
    }
}

/// Reject Argon2 cost parameters a hostile header could weaponise. The `argon2` crate does
/// not range-check `m_cost`/`t_cost`, so a multi-TiB `m_cost_kib` or a `u32::MAX` `t_cost`
/// would OOM or hang the app *during* derivation — which happens before the header AAD can
/// be checked, so the AAD binding can't save us. These ceilings sit comfortably above
/// `kdf::calibrate`'s strongest tier (256 MiB memory, up to ~10 passes).
fn validate_kdf_params(p: &kdf::KdfParams) -> Result<()> {
    const MAX_M_COST_KIB: u32 = 1024 * 1024; // 1 GiB
    let ok = p.algorithm == "argon2id"
        && p.version == 0x13
        && p.key_len as usize == kdf::KEY_LEN
        && (8..=MAX_M_COST_KIB).contains(&p.m_cost_kib)
        && (1..=16).contains(&p.t_cost)
        && (1..=16).contains(&p.p_cost);
    if !ok {
        return Err(Error::Other(
            "backup uses unsupported or unreasonable key-derivation parameters".into(),
        ));
    }
    Ok(())
}

/// The validated result of a restore. `db_key_hex` is kept in Rust only — the command
/// seeds it into this device's keychain (`vault_key::<id>`) so the restored vault can be
/// opened; it is never returned to the webview.
pub struct RestoreOutcome {
    pub vault_id: String,
    pub key_mode: String,
    pub markdown_encryption: String,
    pub app_version: String,
    pub created_at: String,
    pub target_dir: PathBuf,
    pub db_key_hex: Secret,
    /// The mod safety list the backup carried, for [`crate::mods::refresh::take_restored`]: the
    /// restore commands hand it to the acceptance rules, which keep the newer of it and the local
    /// copy. Not trusted until they verify it.
    pub mods_index: Option<manifest::ModsIndex>,
    /// The mods the restored store lists, read from its own rows (which carry each one's name and
    /// state; the manifest's list doesn't), for the restore card. Empty when they couldn't be read,
    /// which the card shows as no mods.
    pub mods: Vec<super::adopt_mods::ModRow>,
}

/// Restore `src` into a fresh `target_dir` using `passphrase`. `target_dir` must not
/// already contain a vault (an existing empty dir is fine and is reused). Blocking —
/// call inside `spawn_blocking`.
pub fn restore(
    src: &Path,
    passphrase: &str,
    target_dir: &Path,
    mut report: impl FnMut(BackupPhase, f32),
    cancel: &AtomicBool,
) -> Result<RestoreOutcome> {
    // Refuse a non-empty target up front, before doing any work.
    if target_dir.exists() {
        let non_empty = std::fs::read_dir(target_dir)?.next().is_some();
        if non_empty {
            return Err(Error::Other(
                "the restore target already contains files; choose an empty folder".into(),
            ));
        }
    }
    let target_parent = target_dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| Error::Other("invalid restore target location".into()))?;
    std::fs::create_dir_all(target_parent)?;

    // --- header (cleartext) → derive key --------------------------------------------
    let mut file =
        File::open(src).map_err(|_| Error::Other("could not open the backup file".into()))?;
    let (flags, header_json, header) = format::read_header(&mut file)?;
    if header.cipher != format::CIPHER_ID {
        return Err(Error::Other("unsupported backup cipher".into()));
    }
    if header.compression != format::COMPRESSION_ID {
        return Err(Error::Other("unsupported backup compression".into()));
    }
    let header_len = 16 + header_json.len() as u64; // magic(8)+ver(2)+flags(2)+len(4)+json

    validate_kdf_params(&header.kdf.params)?;
    let salt = B64
        .decode(&header.kdf.kdf_salt_b64)
        .map_err(|e| Error::Other(format!("corrupt backup salt: {e}")))?;
    let key = kdf::derive_master(passphrase, &salt, &header.kdf.params)?;
    let nonce_prefix = B64
        .decode(&header.stream_nonce_prefix_b64)
        .map_err(|e| Error::Other(format!("corrupt backup nonce: {e}")))?;
    // The archive's own version (format 1 or 2), which `read_header` checked both copies of.
    let aad = format::aad(header.format_version, flags, &header_json);
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key.as_slice()));

    // Progress is metered on archive (ciphertext) bytes consumed, whose total we know.
    let archive_len = std::fs::metadata(src)?.len();
    let payload_len = archive_len.saturating_sub(header_len);
    let mut done: u64 = 0;
    let mut last_pct: i32 = -1;
    let metered = ProgressReader {
        inner: file,
        done: &mut done,
        total: payload_len,
        last_pct: &mut last_pct,
        phase: BackupPhase::Restore,
        report: &mut report,
        cancel,
    };

    // --- decrypt → decompress → unbundle into staging -------------------------------
    let aead_r =
        format::ChunkedAeadReader::new(metered, cipher, &nonce_prefix, aad, header.chunk_size)?;
    let zstd_r = zstd::stream::read::Decoder::new(aead_r)
        .map_err(|e| Error::Other(format!("could not start decompression: {e}")))?;

    let staging = tempfile::Builder::new()
        .prefix("pm-restore-")
        .tempdir_in(target_parent)?;
    let staging_path = staging.path().to_path_buf();

    // What the unpack is allowed to write. Nothing downstream bounds it: `bundle::read_bundle`
    // streams each entry straight to disk, so a vault larger than the drive fills it and then fails
    // with a raw OS error. Fails OPEN — a probe with no answer must not be able to block a restore,
    // so an unrecognised volume behaves exactly as it did before there was a budget.
    let budget = crate::hardware::available_bytes(target_parent)
        .map(|free| free.saturating_sub(RESTORE_FREE_SPACE_RESERVE));
    let mut unpacked: u64 = 0;

    let mut manifest_bytes: Option<Vec<u8>> = None;
    bundle::read_bundle(zstd_r, |path, len, content| {
        if path == manifest::MANIFEST_ENTRY {
            // The bundle framing caps `content` at exactly `len` bytes, so refusing an
            // oversized `len` bounds this in-memory read against a decompression bomb.
            if len > MAX_MANIFEST_BYTES {
                return Err(Error::Other("backup manifest is unreasonably large".into()));
            }
            let mut buf = Vec::new();
            content.read_to_end(&mut buf)?;
            manifest_bytes = Some(buf);
            return Ok(());
        }
        // `path` is already validated by read_bundle; rebuild it component-by-component
        // (never trusting the '/' as an OS separator) under the staging root.
        let mut dest = staging_path.clone();
        for comp in path.split('/') {
            dest.push(comp);
        }
        // Checked against the DECLARED `len` before a byte is written, so one oversized entry is
        // refused up front rather than after it has filled the drive; charged by the ACTUAL count
        // afterwards, so a short entry isn't over-billed.
        if !fits(unpacked, len, budget) {
            return Err(Error::Other(
                "there is not enough free space on this drive to restore this backup; free up \
                 space and try again"
                    .into(),
            ));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = File::create(&dest)?;
        unpacked = unpacked.saturating_add(std::io::copy(content, &mut f)?);
        Ok(())
    })?;

    // --- validate before promoting --------------------------------------------------
    report(BackupPhase::Validate, 0.0);
    let manifest_bytes =
        manifest_bytes.ok_or_else(|| Error::Other("the backup is missing its manifest".into()))?;
    let man: manifest::BackupManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| Error::Other(format!("corrupt backup manifest: {e}")))?;
    if man.schema != manifest::SCHEMA {
        return Err(Error::Other(format!(
            "this backup uses an unsupported manifest version ({})",
            man.schema
        )));
    }

    // The restored DB must open with the embedded key and pass an integrity check. The staging tree
    // is promoted into the vault root verbatim below, so the archive entry name IS the on-disk name
    // — hence the layout constant here and at the packing end (`pack::enumerate_sources`), rather
    // than a retyped literal that could drift from it.
    let db_path = staging_path.join(vault::DB_FILENAME);
    let mods;
    {
        // Opening migrates the staged copy, so a pre-mods store arrives at this build's schema. A
        // store a newer PM wrote (inside an archive whose format this build reads) keeps its typed
        // refusal (I-28): nothing is promoted, and the backup itself is never touched.
        let conn = db::open(&db_path, &man.db_key_hex).map_err(|e| match e {
            Error::Vault(f) if f.code == crate::error::VaultFaultCode::NewerStore => {
                Error::Vault(f)
            }
            e => Error::Other(format!("the restored database could not be opened: {e}")),
        })?;
        let status: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(|e| Error::Other(format!("integrity check failed: {e}")))?;
        if status != "ok" {
            return Err(Error::Other(format!(
                "the restored database failed its integrity check: {status}"
            )));
        }
        // The copy keeps its vault's id, so it must not keep the store id that names the original
        // store's mods folder: it would sweep, prune and uninstall that store's files (I-30). It
        // mints its own when it first needs one. Its mods come back as exact copies from the store
        // it was made from when that store is on this machine (`mods::disk::recover`), and
        // otherwise park until repaired there.
        crate::mods::ledger::forget_store_id(&conn).map_err(|e| {
            Error::Other(format!("the restored database could not be prepared: {e}"))
        })?;
        // What the restore card says about its mods. Not worth failing a restore over: without the
        // list the card simply shows none, and the mods themselves load from these rows regardless.
        mods = super::adopt_mods::read_rows(&conn).unwrap_or_else(|e| {
            eprintln!("restore: the restored store's mod list couldn't be read ({e})");
            Vec::new()
        });
    }
    // The metadata must belong to the same vault the manifest claims.
    let meta = vault::load_meta(&staging_path)?
        .ok_or_else(|| Error::Other("the backup is missing its vault metadata".into()))?;
    if meta.vault_id != man.source_vault_id {
        return Err(Error::Other(
            "backup is inconsistent (metadata vault id does not match the manifest)".into(),
        ));
    }
    report(BackupPhase::Validate, 1.0);

    // --- promote staging → target (atomic; old vault untouched until now) -----------
    // Persist the staging dir (disable auto-delete) so we can rename it into place.
    let staged = staging.keep();
    // A pre-existing empty target dir would make rename fail on some platforms; drop it.
    if target_dir.exists() {
        let _ = std::fs::remove_dir(target_dir);
    }
    if let Err(rename_err) = std::fs::rename(&staged, target_dir) {
        // Cross-volume (or a racing dir): fall back to the shared verified recursive copy,
        // then clean up the persisted staging dir so a decrypted copy isn't left behind.
        // The cleanup runs on the FAILURE path too — `keep()` above turned off the TempDir's own
        // auto-delete, so returning early through `?` here used to leave a fully decrypted vault
        // sitting on disk, which is precisely what the line below exists to prevent.
        let copied = crate::vault::migrate::copy_tree_verified(&staged, target_dir);
        let _ = std::fs::remove_dir_all(&staged);
        copied.map_err(|e| {
            Error::Other(format!(
                "could not place the restored vault: {e} (rename: {rename_err})"
            ))
        })?;
    }

    Ok(RestoreOutcome {
        vault_id: man.source_vault_id,
        key_mode: man.source_key_mode,
        markdown_encryption: man.source_markdown_encryption,
        app_version: man.app_version,
        created_at: man.created_at,
        target_dir: target_dir.to_path_buf(),
        db_key_hex: Secret::from(man.db_key_hex),
        mods_index: man.mods_index,
        mods,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::pack::{pack, PackInputs};
    use std::sync::atomic::AtomicBool;

    /// Every durable file a real vault holds, relative to the vault root, with its bytes.
    ///
    /// The fixture used to be a single flat `vault/note.md`, which meant `collect_tree`'s
    /// recursion and both `push_optional` calls in pack.rs were dead code as far as the suite was
    /// concerned — a backup that silently dropped the chats subfolder, the photo originals, or
    /// either encrypted sidecar would have passed all three round-trip tests. The bytes are
    /// deliberately non-UTF-8 for the photo, because a photo original is a byte file and a
    /// text-mode copy would corrupt it invisibly.
    fn vault_contents() -> Vec<(&'static str, &'static [u8])> {
        vec![
            ("vault/note.md", b"# hello\n\nbody" as &[u8]),
            // A nested level — the only thing that exercises collect_tree's recursion.
            (
                "vault/chats/chat-01-01-2026-a.md",
                b"# chat\n\n**You:** hi\n",
            ),
            // A photo original: raw bytes, not text, under a second subfolder.
            (
                "vault/photos/0f1e2d3c.png.pmenc",
                &[0x00, 0x01, 0xff, 0xfe, 0x80, 0x0a, 0x0d, 0x1a],
            ),
            // The two always-encrypted sidecars pack.rs pushes optionally.
            (crate::entities::RULES_FILENAME, b"\x01encrypted-rules"),
            (
                crate::index_only::MANIFEST_FILENAME,
                b"\x01encrypted-manifest",
            ),
        ]
    }

    // A device-mode vault fixture: a real (tiny) encrypted DB + a meta + the tree above,
    // packed and then restored, proving the archive is self-describing and portable.
    fn make_source_vault(dir: &Path) -> (vault::VaultMeta, String) {
        std::fs::create_dir_all(dir).unwrap();
        let meta = vault::ensure_device_meta(dir).unwrap();
        // A device vault's DB key is a random 64-hex; make one and open a DB with it.
        let key_hex = "11".repeat(32);
        let db_path = dir.join("pm.sqlite");
        {
            let conn = db::open(&db_path, &key_hex).unwrap();
            conn.execute_batch(
                "CREATE TABLE t(x); INSERT INTO t VALUES (42);
                 INSERT INTO settings(key, value) VALUES ('mods_store_id', '0123456789abcdef');",
            )
            .unwrap();
        }
        for (rel, bytes) in vault_contents() {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();
        }
        (meta, key_hex)
    }

    #[test]
    fn the_unpack_budget_stops_before_it_fills_the_drive() {
        // `read_bundle` streams every entry straight to disk with nothing counting the total, so a
        // vault larger than the volume filled it and then died on a raw OS error. The arithmetic is
        // the part worth pinning: it runs against a DECLARED length, which is the one number a
        // damaged or hostile archive controls, so it must not be able to overflow into a pass.
        assert!(
            fits(0, u64::MAX, None),
            "no probe answer ⇒ no budget, as before"
        );
        assert!(
            fits(900, 100, Some(1000)),
            "exactly filling the budget is allowed"
        );
        assert!(!fits(900, 101, Some(1000)), "one byte past it is not");
        assert!(
            !fits(1, u64::MAX, Some(1000)),
            "a declared length near u64::MAX saturates rather than wrapping to a pass"
        );
        assert!(!fits(0, 1, Some(0)), "a full drive admits nothing");
    }

    #[test]
    fn pack_then_restore_round_trips_and_validates() {
        let root = tempfile::tempdir().unwrap();
        let src_vault = root.path().join("src");
        let (meta, key_hex) = make_source_vault(&src_vault);

        // A separate VACUUM snapshot, exactly as the command produces.
        let snap = root.path().join("snap.sqlite");
        {
            let conn = db::open(&src_vault.join("pm.sqlite"), &key_hex).unwrap();
            let escaped = snap.to_string_lossy().replace('\'', "''");
            conn.execute_batch(&format!("VACUUM INTO '{escaped}'"))
                .unwrap();
        }

        let archive = root.path().join("backup.pmbackup");
        let inputs = PackInputs {
            vault_root: src_vault.clone(),
            db_snapshot: snap,
            markdown_dir: src_vault.join("vault"),
            meta: meta.clone(),
            db_key_hex: Secret::from(key_hex.clone()),
            app_version: "test".into(),
            created_at: "2026-07-02T00:00:00Z".into(),
            mods_index: Some(crate::backup::manifest::ModsIndex {
                index_b64: "aW5kZXg=".into(),
                signature_b64: "c2ln".into(),
            }),
            mods: None,
        };
        let cancel = AtomicBool::new(false);
        pack(inputs, &archive, "correct-horse", |_, _| {}, &cancel).unwrap();

        // Restore into a fresh dir with the right passphrase.
        let target = root.path().join("restored");
        let outcome = restore(&archive, "correct-horse", &target, |_, _| {}, &cancel).unwrap();
        assert_eq!(outcome.vault_id, meta.vault_id);
        assert_eq!(outcome.db_key_hex.expose(), key_hex);
        // The mod safety list the backup carried comes out as it went in (the commands run it
        // through the acceptance rules; a backup made before the field has none).
        assert_eq!(
            outcome.mods_index,
            Some(crate::backup::manifest::ModsIndex {
                index_b64: "aW5kZXg=".into(),
                signature_b64: "c2ln".into(),
            })
        );

        // The restored DB opens with the embedded key and holds our row.
        let conn = db::open(&target.join("pm.sqlite"), outcome.db_key_hex.expose()).unwrap();
        let x: i64 = conn.query_row("SELECT x FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(x, 42);
        // A restored copy is a store of its own: the id that names the original's mods folder
        // doesn't come with it (the original keeps it).
        assert_eq!(
            crate::mods::ledger::read_store_id(&conn).unwrap(),
            None,
            "the restored copy doesn't share the original's mods folder"
        );
        let src = db::open(&src_vault.join("pm.sqlite"), &key_hex).unwrap();
        assert_eq!(
            crate::mods::ledger::read_store_id(&src).unwrap().as_deref(),
            Some("0123456789abcdef")
        );

        // Every durable file came across byte-identically — the nested chat, the photo original's
        // raw bytes, and both encrypted sidecars, not just the one flat markdown file.
        for (rel, expected) in vault_contents() {
            let got = std::fs::read(target.join(rel))
                .unwrap_or_else(|e| panic!("{rel} did not survive the round trip: {e}"));
            assert_eq!(got, expected, "{rel} came back with different bytes");
        }
        // …and the metadata that makes the restored vault self-describing.
        assert!(target.join(vault::META_FILENAME).exists());
    }

    #[test]
    fn wrong_passphrase_fails_and_leaves_no_target() {
        let root = tempfile::tempdir().unwrap();
        let src_vault = root.path().join("src");
        let (meta, key_hex) = make_source_vault(&src_vault);
        let snap = root.path().join("snap.sqlite");
        {
            let conn = db::open(&src_vault.join("pm.sqlite"), &key_hex).unwrap();
            let escaped = snap.to_string_lossy().replace('\'', "''");
            conn.execute_batch(&format!("VACUUM INTO '{escaped}'"))
                .unwrap();
        }
        let archive = root.path().join("backup.pmbackup");
        let inputs = PackInputs {
            vault_root: src_vault.clone(),
            db_snapshot: snap,
            markdown_dir: src_vault.join("vault"),
            meta,
            db_key_hex: Secret::from(key_hex),
            app_version: "test".into(),
            created_at: "2026-07-02T00:00:00Z".into(),
            mods_index: None,
            mods: None,
        };
        let cancel = AtomicBool::new(false);
        pack(inputs, &archive, "right", |_, _| {}, &cancel).unwrap();

        let target = root.path().join("restored");
        assert!(restore(&archive, "wrong", &target, |_, _| {}, &cancel).is_err());
        // Nothing was promoted.
        assert!(!target.exists());
    }

    #[test]
    fn truncated_archive_fails() {
        let root = tempfile::tempdir().unwrap();
        let src_vault = root.path().join("src");
        let (meta, key_hex) = make_source_vault(&src_vault);
        let snap = root.path().join("snap.sqlite");
        {
            let conn = db::open(&src_vault.join("pm.sqlite"), &key_hex).unwrap();
            let escaped = snap.to_string_lossy().replace('\'', "''");
            conn.execute_batch(&format!("VACUUM INTO '{escaped}'"))
                .unwrap();
        }
        let archive = root.path().join("backup.pmbackup");
        let inputs = PackInputs {
            vault_root: src_vault.clone(),
            db_snapshot: snap,
            markdown_dir: src_vault.join("vault"),
            meta,
            db_key_hex: Secret::from(key_hex),
            app_version: "test".into(),
            created_at: "2026-07-02T00:00:00Z".into(),
            mods_index: None,
            mods: None,
        };
        let cancel = AtomicBool::new(false);
        pack(inputs, &archive, "pw", |_, _| {}, &cancel).unwrap();

        // Chop the archive in half.
        let bytes = std::fs::read(&archive).unwrap();
        std::fs::write(&archive, &bytes[..bytes.len() / 2]).unwrap();
        let target = root.path().join("restored");
        assert!(restore(&archive, "pw", &target, |_, _| {}, &cancel).is_err());
        assert!(!target.exists());
    }

    /// A source vault whose store sits at exactly schema `version` (authentic, from the real
    /// migrations) with one task in it, and its `VACUUM INTO` snapshot taken WITHOUT migrating it.
    fn vault_at_version(root: &Path, version: usize) -> (PackInputs, String) {
        vault_at_version_holding(root, version, false)
    }

    /// The same, optionally with one document in the store too (what v61 reads to mark a Map user).
    fn vault_at_version_holding(
        root: &Path,
        version: usize,
        document: bool,
    ) -> (PackInputs, String) {
        vault_at_version_seeded(root, version, document, None)
    }

    /// The same, optionally with a Pinboard board row (`settings['pinboard']`) in the store too
    /// (what v62 reads to mark a Pinboard user).
    fn vault_at_version_seeded(
        root: &Path,
        version: usize,
        document: bool,
        board: Option<&str>,
    ) -> (PackInputs, String) {
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let meta = vault::ensure_device_meta(&src).unwrap();
        let key_hex = "22".repeat(32);
        let snap = root.join("snap.sqlite");
        {
            let conn = db::open_at_version(&src.join("pm.sqlite"), &key_hex, version).unwrap();
            conn.execute("INSERT INTO tasks(title) VALUES ('call Sam')", [])
                .unwrap();
            // A row from before the usage detail columns, so a restore that climbs the ladder
            // shows what v64 does to a row that already exists.
            conn.execute(
                "INSERT INTO usage_log(model, kind, prompt_tokens, completion_tokens) \
                 VALUES ('old/model', 'chat', 10, 5)",
                [],
            )
            .unwrap();
            // A person's daily spend limit, which a `.pmbackup` carries like any other setting.
            conn.execute(
                "INSERT INTO settings(key, value) VALUES ('daily_limit_anthropic_usd', '7.5')",
                [],
            )
            .unwrap();
            if document {
                conn.execute(
                    "INSERT INTO documents(vault_path, title, content_hash) \
                     VALUES ('a.md', 'A', 'ha')",
                    [],
                )
                .unwrap();
            }
            if let Some(board) = board {
                conn.execute(
                    "INSERT INTO settings(key, value) VALUES ('pinboard', ?1)",
                    [board],
                )
                .unwrap();
            }
            let escaped = snap.to_string_lossy().replace('\'', "''");
            conn.execute_batch(&format!("VACUUM INTO '{escaped}'"))
                .unwrap();
        }
        let inputs = PackInputs {
            vault_root: src.clone(),
            db_snapshot: snap,
            markdown_dir: src.join("vault"),
            meta,
            db_key_hex: Secret::from(key_hex.clone()),
            app_version: "test".into(),
            created_at: "2026-07-02T00:00:00Z".into(),
            mods_index: None,
            mods: None,
        };
        (inputs, key_hex)
    }

    fn schema_version(path: &Path, key_hex: &str) -> i64 {
        db::open_unmigrated(path, key_hex)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }

    fn latest_version() -> i64 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("probe.sqlite");
        db::open(&path, &"33".repeat(32)).unwrap();
        schema_version(&path, &"33".repeat(32))
    }

    /// Spec §1.11: every backup made before mods is format 1 around a pre-v60 store. This build
    /// still restores it, and the restore's open carries the store up to this build's schema (v60's
    /// mod tables, v61's marker and drop) with the user's data intact.
    #[test]
    fn a_pre_mods_format_1_backup_restores_and_migrates_to_this_builds_schema() {
        let root = tempfile::tempdir().unwrap();
        let (inputs, key_hex) = vault_at_version(root.path(), 59);
        assert_eq!(
            schema_version(&inputs.db_snapshot, &key_hex),
            59,
            "the archive really holds a pre-mods store"
        );
        let archive = root.path().join("old.pmbackup");
        let cancel = AtomicBool::new(false);
        crate::backup::pack::pack_as(inputs, &archive, "pw", |_, _| {}, &cancel, 1).unwrap();
        // What an older PM wrote: format 1 in the preamble, and so in the AAD.
        let (_, _, header) = format::read_header(&mut File::open(&archive).unwrap()).unwrap();
        assert_eq!(header.format_version, 1);

        let target = root.path().join("restored");
        restore(&archive, "pw", &target, |_, _| {}, &cancel).unwrap();
        let restored = target.join("pm.sqlite");
        assert_eq!(schema_version(&restored, &key_hex), latest_version());
        let conn = db::open_unmigrated(&restored, &key_hex).unwrap();
        let mods: i64 = conn
            .query_row("SELECT count(*) FROM mods", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mods, 0, "v60's tables are there, empty");
        let title: String = conn
            .query_row("SELECT title FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "call Sam", "the user's data came through the ladder");
    }

    /// A backup taken by the build before v61 (a format-2 archive around a v60 store) restores on
    /// this one: the open migrates it to this build's schema (v61 drops the Map's cache table and
    /// writes the Document Map's marker only when the store holds a document, then v62 runs).
    /// `FORMAT_VERSION` did not change for v61: a format-2 archive may hold a store at v60 or later.
    #[test]
    fn a_v60_backup_restores_and_migrates_to_this_builds_schema() {
        for (document, marker) in [(false, None), (true, Some("1".to_string()))] {
            let root = tempfile::tempdir().unwrap();
            let (inputs, key_hex) = vault_at_version_holding(root.path(), 60, document);
            assert_eq!(schema_version(&inputs.db_snapshot, &key_hex), 60);
            let archive = root.path().join("v60.pmbackup");
            let cancel = AtomicBool::new(false);
            pack(inputs, &archive, "pw", |_, _| {}, &cancel).unwrap();

            let target = root.path().join("restored");
            restore(&archive, "pw", &target, |_, _| {}, &cancel).unwrap();
            let restored = target.join("pm.sqlite");
            assert_eq!(
                schema_version(&restored, &key_hex),
                latest_version(),
                "document: {document}"
            );
            let conn = db::open_unmigrated(&restored, &key_hex).unwrap();
            let found: Option<String> = conn
                .query_row(
                    "SELECT value FROM settings WHERE key = 'mods_autoinstall_map'",
                    [],
                    |r| r.get(0),
                )
                .ok();
            assert_eq!(found, marker, "document: {document}");
            let layout: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name = 'doc_layout'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(layout, 0, "the Map's cache table is gone");
            let title: String = conn
                .query_row("SELECT title FROM tasks", [], |r| r.get(0))
                .unwrap();
            assert_eq!(title, "call Sam");
        }
    }

    /// A backup taken one build before the usage detail columns (a v62 store, a whole-store snapshot
    /// of the v36-patched `usage_log`) restores here: the open runs v64, so the old row reads NULL
    /// for every new column, the index on the patched table exists and the table still takes a row
    /// that carries them. The restore itself touches no usage column (`strip.rs` only clears `mod_id`).
    #[test]
    fn a_v62_backup_restores_with_the_usage_detail_columns_and_their_index() {
        let root = tempfile::tempdir().unwrap();
        let (inputs, key_hex) = vault_at_version(root.path(), 62);
        assert_eq!(schema_version(&inputs.db_snapshot, &key_hex), 62);
        let archive = root.path().join("v62.pmbackup");
        let cancel = AtomicBool::new(false);
        pack(inputs, &archive, "pw", |_, _| {}, &cancel).unwrap();

        let target = root.path().join("restored");
        restore(&archive, "pw", &target, |_, _| {}, &cancel).unwrap();
        let restored = target.join("pm.sqlite");
        assert_eq!(schema_version(&restored, &key_hex), latest_version());
        let conn = db::open_unmigrated(&restored, &key_hex).unwrap();
        let nulls: i64 = conn
            .query_row(
                "SELECT count(*) FROM usage_log WHERE model = 'old/model' \
                   AND served_provider IS NULL AND cached_tokens IS NULL \
                   AND cache_write_tokens IS NULL AND thinking_tokens IS NULL \
                   AND outcome IS NULL AND cost_note IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nulls, 1, "the old row reads NULL in every new column");
        let index: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master \
                 WHERE type = 'index' AND name = 'idx_usage_log_provider_created'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(index, 1, "the index on the patched table came through");
        conn.execute(
            "INSERT INTO usage_log(model, kind, served_provider, thinking_tokens) \
             VALUES ('openai/m', 'chat', 'openai', 7)",
            [],
        )
        .unwrap();
    }

    /// The same restore from a store AT v63 (Contacts' build, the one everyone on 3.202.0 has): only
    /// v64 runs, the old row reads NULL in every new column, the index exists, and the people tables
    /// Contacts added survive the restore.
    #[test]
    fn a_v63_backup_restores_with_the_usage_detail_columns_and_the_people_tables() {
        let root = tempfile::tempdir().unwrap();
        let (inputs, key_hex) = vault_at_version(root.path(), 63);
        assert_eq!(schema_version(&inputs.db_snapshot, &key_hex), 63);
        let archive = root.path().join("v63.pmbackup");
        let cancel = AtomicBool::new(false);
        pack(inputs, &archive, "pw", |_, _| {}, &cancel).unwrap();

        let target = root.path().join("restored");
        restore(&archive, "pw", &target, |_, _| {}, &cancel).unwrap();
        let restored = target.join("pm.sqlite");
        assert_eq!(schema_version(&restored, &key_hex), latest_version());
        let conn = db::open_unmigrated(&restored, &key_hex).unwrap();
        let nulls: i64 = conn
            .query_row(
                "SELECT count(*) FROM usage_log WHERE model = 'old/model' \
                   AND served_provider IS NULL AND cached_tokens IS NULL \
                   AND cache_write_tokens IS NULL AND thinking_tokens IS NULL \
                   AND outcome IS NULL AND cost_note IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nulls, 1, "the old row reads NULL in every new column");
        let objects: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE \
                   (type = 'index' AND name = 'idx_usage_log_provider_created') \
                   OR (type = 'table' AND name IN ('people', 'person_details', 'interactions'))",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(objects, 4, "the index and Contacts' tables are all there");
    }

    /// A daily spend limit is a `settings` row, so it travels in a backup and a restore keeps it: the
    /// restored store reads the same limit through the one reader the gateway uses, and the brake is
    /// still on after a restore onto a new machine.
    #[test]
    fn a_daily_limit_survives_a_backup_and_restore() {
        let root = tempfile::tempdir().unwrap();
        let (inputs, key_hex) = vault_at_version(root.path(), 62);
        let archive = root.path().join("limit.pmbackup");
        let cancel = AtomicBool::new(false);
        pack(inputs, &archive, "pw", |_, _| {}, &cancel).unwrap();
        let target = root.path().join("restored");
        restore(&archive, "pw", &target, |_, _| {}, &cancel).unwrap();
        let conn = db::open_unmigrated(&target.join("pm.sqlite"), &key_hex).unwrap();
        assert_eq!(
            crate::daily_limit::read_setting(&conn, crate::llm_gateway::CloudProvider::Anthropic)
                .unwrap(),
            crate::daily_limit::Setting::On(7.5)
        );
        assert_eq!(
            crate::daily_limit::read_setting(&conn, crate::llm_gateway::CloudProvider::OpenRouter)
                .unwrap(),
            crate::daily_limit::Setting::Off
        );
    }

    /// Pack a store at `version` (optionally holding a board), restore it, and report the schema
    /// version, the Pinboard's two markers (auto-install, pending) and the board row after.
    fn restore_with_board(
        version: usize,
        board: Option<&str>,
    ) -> (i64, (Option<String>, Option<String>), Option<String>) {
        let root = tempfile::tempdir().unwrap();
        let (inputs, key_hex) = vault_at_version_seeded(root.path(), version, false, board);
        assert_eq!(
            schema_version(&inputs.db_snapshot, &key_hex),
            version as i64
        );
        let archive = root.path().join("old.pmbackup");
        let cancel = AtomicBool::new(false);
        pack(inputs, &archive, "pw", |_, _| {}, &cancel).unwrap();

        let target = root.path().join("restored");
        restore(&archive, "pw", &target, |_, _| {}, &cancel).unwrap();
        let restored = target.join("pm.sqlite");
        let conn = db::open_unmigrated(&restored, &key_hex).unwrap();
        let get = |key: &str| -> Option<String> {
            conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .ok()
        };
        (
            schema_version(&restored, &key_hex),
            (
                get("mods_autoinstall_pinboard"),
                get("pinboard_move_pending"),
            ),
            get("pinboard"),
        )
    }

    /// A backup taken at v61 (the build before the Pinboard's markers) restores here: the open
    /// runs v62 over the restored content, so a board with a widget is marked for the install
    /// and the copy, exactly as an upgrade marks it, and the board's bytes come through. v62
    /// changed no table, so `FORMAT_VERSION` stays 2.
    #[test]
    fn a_v61_backup_with_a_board_restores_with_both_pinboard_markers() {
        let board = r#"{"version":9,"widgets":[{"id":"w1"}]}"#;
        let (version, markers, kept) = restore_with_board(61, Some(board));
        assert_eq!(version, latest_version());
        assert!(version >= 62);
        assert_eq!(markers, (Some("1".to_string()), Some("1".to_string())));
        assert_eq!(kept.as_deref(), Some(board), "the board came through");
    }

    /// A v60 backup climbs through v61 and v62: marked when it holds a board, and left
    /// unmarked when it does not.
    #[test]
    fn a_v60_backup_restores_with_pinboard_markers_only_when_it_has_a_board() {
        let board = r#"{"widgets":[{"id":"w1"}]}"#;
        let (version, markers, kept) = restore_with_board(60, Some(board));
        assert_eq!(version, latest_version());
        assert_eq!(markers, (Some("1".to_string()), Some("1".to_string())));
        assert_eq!(kept.as_deref(), Some(board));

        let (version, markers, kept) = restore_with_board(60, None);
        assert_eq!(version, latest_version());
        assert_eq!(markers, (None, None), "no board and no note, no marker");
        assert_eq!(kept, None);
    }

    /// A store a newer PM wrote is refused at restore with its typed fault (I-28): nothing is
    /// promoted, and the archive is left exactly as it was.
    #[test]
    fn a_backup_holding_a_newer_store_is_refused_and_promotes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let (inputs, key_hex) = vault_at_version(root.path(), latest_version() as usize);
        let newer = latest_version() + 1;
        db::open_unmigrated(&inputs.db_snapshot, &key_hex)
            .unwrap()
            .pragma_update(None, "user_version", newer)
            .unwrap();
        let archive = root.path().join("new.pmbackup");
        let cancel = AtomicBool::new(false);
        pack(inputs, &archive, "pw", |_, _| {}, &cancel).unwrap();
        let before = std::fs::read(&archive).unwrap();

        let target = root.path().join("restored");
        let err = match restore(&archive, "pw", &target, |_, _| {}, &cancel) {
            Ok(_) => panic!("a newer store must not restore"),
            Err(e) => e,
        };
        match err {
            Error::Vault(f) => assert_eq!(f.code, crate::error::VaultFaultCode::NewerStore),
            other => panic!("expected the newer-store fault, got {other:?}"),
        }
        assert!(!target.exists(), "nothing was promoted");
        assert_eq!(std::fs::read(&archive).unwrap(), before);
    }
}
