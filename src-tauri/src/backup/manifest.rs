// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `backup-manifest.json` that rides *inside* the encrypted layer of a `.pmbackup`.
//! It carries the one thing the cleartext header must never hold — the source vault's
//! raw DB key (= hex of the master) — so a restore is single-secret (only the backup
//! passphrase) and portable to any machine. It also records provenance the restore
//! uses to validate the archive against its own `vault-meta.json`.

use serde::{Deserialize, Serialize};

/// Manifest schema version (bumped only on a breaking manifest change).
pub const SCHEMA: u32 = 1;
/// The reserved bundle entry name for the manifest; read into memory (never to disk).
pub const MANIFEST_ENTRY: &str = "backup-manifest.json";

/// Provenance + the embedded key, serialized into the encrypted bundle. Not `Serialize`
/// via `Secret` because it must round-trip verbatim; the plaintext exists only briefly
/// in memory (pack) or inside the auto-cleaned staging dir (restore).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupManifest {
    pub schema: u32,
    pub source_vault_id: String,
    /// `"device"` or `"passphrase"` (the source vault's key mode at backup time).
    pub source_key_mode: String,
    /// The 64-hex SQLCipher key (= hex of the master). The crux of portability.
    pub db_key_hex: String,
    /// `"none"` or `"xchacha20poly1305"` — the source's Markdown-at-rest policy.
    pub source_markdown_encryption: String,
    /// App version that produced the archive (for diagnostics / future migrations).
    pub app_version: String,
    /// RFC3339 creation timestamp.
    pub created_at: String,
    /// The mod safety list's newest accepted index as the backing-up machine held it (Mods spec
    /// §1.10, INVARIANTS.md I-31), so a restore can never roll the seen seq back: at restore it is
    /// run through the ordinary acceptance rules and the newer of it and the local copy wins. The
    /// signed pair rather than a bare seq, because a number from a backup proves nothing, and a
    /// crafted one would lock the real index out. Additive: absent in every earlier backup, and an
    /// older PM ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mods_index: Option<ModsIndex>,
}

/// `index.json` and `index.json.minisig` as accepted, each base64 (the index is under 1 MiB, so the
/// manifest stays far below `restore::MAX_MANIFEST_BYTES`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModsIndex {
    pub index_b64: String,
    pub signature_b64: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trips() {
        let m = BackupManifest {
            schema: SCHEMA,
            source_vault_id: "vault-123".into(),
            source_key_mode: "device".into(),
            db_key_hex: "00".repeat(32),
            source_markdown_encryption: "none".into(),
            app_version: "2.64.0-alpha".into(),
            created_at: "2026-07-02T00:00:00Z".into(),
            mods_index: None,
        };
        let bytes = serde_json::to_vec(&m).unwrap();
        let back: BackupManifest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(m, back);
        assert_eq!(back.db_key_hex.len(), 64);
        // Without a safety list the field isn't written at all, so an archive is byte-for-byte what
        // it was before the field existed.
        assert!(!String::from_utf8(bytes).unwrap().contains("mods_index"));
    }

    #[test]
    fn the_mods_index_is_additive_and_round_trips() {
        let old = r#"{"schema":1,"source_vault_id":"v","source_key_mode":"device",
            "db_key_hex":"00","source_markdown_encryption":"none","app_version":"1",
            "created_at":"t"}"#;
        let m: BackupManifest = serde_json::from_str(old).unwrap();
        assert_eq!(
            m.mods_index, None,
            "a manifest from before the field reads as none"
        );
        let with = BackupManifest {
            mods_index: Some(ModsIndex {
                index_b64: "aW5kZXg=".into(),
                signature_b64: "c2ln".into(),
            }),
            ..m
        };
        let bytes = serde_json::to_vec(&with).unwrap();
        assert_eq!(
            serde_json::from_slice::<BackupManifest>(&bytes).unwrap(),
            with
        );
    }
}
