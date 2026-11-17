// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the Mods tab's unit tests share: a real store (SQLCipher, every migration) and `mods` rows
//! written with a manifest as full as the listing reads one.

use rusqlite::{params, Connection};
use serde_json::{json, Value};

const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

/// A fresh, fully migrated store in a temporary folder (kept alive by the returned guard).
pub fn store() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
    (dir, conn)
}

/// A manifest as the ledger stores it, with `permissions` declared.
pub fn manifest(permissions: &[&str]) -> Value {
    json!({
        "description": "Shows your documents.",
        "icon": "app/icon.png",
        "permissions": permissions.iter().map(|p| json!({"id": p, "why": format!("for {p}")})).collect::<Vec<_>>(),
        "internetHosts": [],
        "contributions": {"nav.tab": {"label": "Map"}},
        "apiVersion": {"min": "1.0", "target": "1.0"},
        "data": {"keys": [{"key": "layout", "class": "cache"}, {"key": "notes", "class": "content"}]},
        "entry": {"page": "app/index.html"},
    })
}

pub struct Row {
    pub id: &'static str,
    pub name: &'static str,
    pub version: &'static str,
    pub tier: &'static str,
    pub signed: bool,
    pub installed_from: &'static str,
    pub manifest: Value,
    pub files: Value,
    pub state: &'static str,
    pub reason: Option<&'static str>,
    pub consented: bool,
}

impl Default for Row {
    fn default() -> Self {
        Self {
            id: "atlas.map",
            name: "Map",
            version: "1.0.0",
            tier: "first",
            signed: true,
            installed_from: "catalogue",
            manifest: manifest(&["storage", "documents.meta"]),
            files: json!([
                {"path": "app/index.html", "sha256": "0".repeat(64), "size": 10},
                {"path": "app/icon.png", "sha256": "0".repeat(64), "size": 10},
            ]),
            state: "active",
            reason: None,
            consented: true,
        }
    }
}

pub fn insert(conn: &Connection, row: &Row) {
    conn.execute(
        "INSERT INTO mods(id, name, version, tier, author, source_repo, source_commit, signed, \
                          install_id, installed_from, package_sha256, manifest, files, state, \
                          state_reason, consented_at) \
         VALUES (?1, ?2, ?3, ?4, 'Atlas', 'itsAtlas-org/Personal-Manager', ?5, ?6, \
                 '0123456789abcdef', ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            row.id,
            row.name,
            row.version,
            row.tier,
            "c".repeat(40),
            row.signed,
            row.installed_from,
            "a".repeat(64),
            row.manifest.to_string(),
            row.files.to_string(),
            row.state,
            row.reason,
            row.consented.then_some("2026-11-11T00:00:00.000Z"),
        ],
    )
    .unwrap();
}
