// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The MCP server's half of the record that lives on this computer alone (Mods spec 1.16): whether the
//! server is switched on for this device in this vault, and which connected apps this device paired.
//!
//! **Why a file, not the store.** A connected app's row (`mcp_clients`) sits in the vault's store, which
//! a restore, a shared vault's other members or a copied database all carry to somewhere else. None
//! of them may switch the server on, or make a token work, on a computer that never did either. So
//! the store's rows are only half the answer, exactly as for developer mode (`mods::devmode`): the
//! other half is this one file in `data_dir`, and a client verifies only when BOTH agree.
//!
//! **The file** is `data_dir/mods-mcp.json`: `{v: 1, vaults: [{id, at, on, clients: [{id, hash,
//! scopes_hash}]}]}`.
//! - A vault's entry is its id AND the tag of the folder it sits in ([`Here`], the same one-way tag
//!   developer mode uses): a vault that merely claims an id this device switched on in (a copy, a
//!   hostile share) has another folder, so it reads as off.
//! - `hash` is the hex SHA-256 of the app's token; the token itself is shown once and never kept.
//!   `scopes_hash` is the hex SHA-256 of `id|hash|scopes` (the scopes sorted, joined by commas), so
//!   whoever can write the store's `scopes` column cannot widen what an app may read: the pair would
//!   no longer match, and only pairing again (which needs this device) writes a new one.
//! - It is written 0600 on Unix, by a temp file made with that mode, fsynced and renamed over the old
//!   one, so a crash leaves the old file or the new one and no other program running as another user
//!   can read it. (A program running as this user can; that is the module's stated limit, I-39.)
//!
//! **A bad file is off.** Missing, unreadable, not JSON this module wrote, a wrong version, a field it
//! does not know, over [`MAX_FILE_BYTES`], more than [`MAX_VAULTS`] vaults or [`MAX_CLIENTS_PER_VAULT`]
//! clients in one, a hash that is not 64 lowercase hex characters: the whole file verifies nobody and
//! nothing ever falls back to accepting. A write replaces a file that is not ours, and caps it (the
//! oldest vault falls off first and is then simply off), so a write can never make the file "not ours".
//!
//! **Forgetting is final.** [`all_off`] removes the vault's entry (the restore reset calls it, and fails
//! if it cannot). An old copy of the file put back afterwards would name pairs whose rows the reset
//! deleted and whose tokens nothing remembers, so it cannot re-enable anything that matters; and the
//! entry itself is gone, so there is nothing to flip back on.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::mods::devmode::Here;
use crate::mods::disk;

/// The device's file in `data_dir`.
pub const DEVICE_FILE: &str = "mods-mcp.json";

/// Most vaults the file remembers (the oldest falls off first). A file that lists more is not one PM
/// wrote.
pub const MAX_VAULTS: usize = 64;

/// Most apps one vault may have paired on this device.
pub const MAX_CLIENTS_PER_VAULT: usize = 16;

/// The most the file may weigh (read and written). A larger file is off for every vault.
pub const MAX_FILE_BYTES: u64 = 16 * 1024;

const FORMAT: u32 = 1;

/// The longest vault id or folder tag an entry keeps.
const MAX_TAG_LEN: usize = 128;

/// Reading and rewriting the file are one step at a time within this process.
static DEVICE_FILE_LOCK: Mutex<()> = Mutex::new(());

/// One app this device paired: its id in `mcp_clients`, the hash of its token and the hash that binds
/// its scopes to both (module doc).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedClient {
    pub id: i64,
    pub hash: String,
    pub scopes_hash: String,
}

/// One vault this device knows: its id, the tag of its folder, whether the server is on, and the
/// paired apps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpEntry {
    pub id: String,
    pub at: String,
    pub on: bool,
    pub clients: Vec<PairedClient>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    v: u32,
    vaults: Vec<McpEntry>,
}

/// What this device says about the vault it is in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceState {
    pub on: bool,
    pub clients: Vec<PairedClient>,
}

fn is_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The vaults a file's text names, or `None` when it isn't a file PM wrote (pure).
fn vaults_of(text: &str) -> Option<Vec<McpEntry>> {
    let r: Record = serde_json::from_str(text).ok()?;
    let tag = |f: &str| !f.is_empty() && f.len() <= MAX_TAG_LEN;
    let client = |c: &PairedClient| c.id > 0 && is_hash(&c.hash) && is_hash(&c.scopes_hash);
    let entry = |e: &McpEntry| {
        tag(&e.id)
            && tag(&e.at)
            && e.clients.len() <= MAX_CLIENTS_PER_VAULT
            && e.clients.iter().all(client)
            // One pair per app.
            && e.clients
                .iter()
                .enumerate()
                .all(|(i, c)| e.clients[..i].iter().all(|p| p.id != c.id))
    };
    let sane = r.v == FORMAT
        && r.vaults.len() <= MAX_VAULTS
        && r.vaults.iter().all(entry)
        // One entry per vault.
        && r.vaults
            .iter()
            .enumerate()
            .all(|(i, e)| r.vaults[..i].iter().all(|p| p.id != e.id));
    sane.then_some(r.vaults)
}

fn encode(vaults: &[McpEntry]) -> String {
    serde_json::to_string(&Record {
        v: FORMAT,
        vaults: vaults.to_vec(),
    })
    .unwrap_or_default()
}

/// The vaults the file lists. A missing file, or one that isn't what PM writes (corrupt, over a cap),
/// lists none; only a file that can't be *read* is an error.
fn read_vaults(dir: &Path) -> io::Result<Vec<McpEntry>> {
    let file = match File::open(dir.join(DEVICE_FILE)) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Ok(Vec::new());
    }
    Ok(std::str::from_utf8(&bytes)
        .ok()
        .and_then(vaults_of)
        .unwrap_or_default())
}

/// What this device says about `here`'s vault: on or off and the apps it paired. Any failure, and a
/// vault listed at another folder, is off with nobody paired.
pub fn device_state(here: &Here) -> DeviceState {
    let _guard = DEVICE_FILE_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    read_vaults(here.dir())
        .ok()
        .and_then(|vaults| {
            vaults
                .into_iter()
                .find(|e| e.id == here.vault() && e.at == here.place())
        })
        .map(|e| DeviceState {
            on: e.on,
            clients: e.clients,
        })
        .unwrap_or_default()
}

fn invalid(why: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, why)
}

/// Change `here`'s entry with `change` (a fresh, off one when there is none, or when the vault is
/// listed at another folder), move it to the newest place and write the file, capped.
fn update(here: &Here, change: impl FnOnce(&mut McpEntry) -> io::Result<()>) -> io::Result<()> {
    if here.place().is_empty() || here.place().len() > MAX_TAG_LEN {
        return Err(invalid("this vault's folder can't be kept"));
    }
    let _guard = DEVICE_FILE_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let mut vaults = read_vaults(here.dir())?;
    let mut entry = match vaults.iter().position(|e| e.id == here.vault()) {
        Some(i) => vaults.remove(i),
        None => McpEntry {
            id: here.vault().to_owned(),
            at: here.place().to_owned(),
            on: false,
            clients: Vec::new(),
        },
    };
    if entry.at != here.place() {
        // The same id in another folder is another vault: what that one held is not this one's.
        entry = McpEntry {
            id: here.vault().to_owned(),
            at: here.place().to_owned(),
            on: false,
            clients: Vec::new(),
        };
    }
    change(&mut entry)?;
    vaults.push(entry);
    let over = vaults.len().saturating_sub(MAX_VAULTS);
    vaults.drain(..over);
    // The file is rejected whole when it is too big to read back, so a write makes room first: the
    // oldest vault falls off (and is then simply off).
    let mut text = encode(&vaults);
    while text.len() as u64 > MAX_FILE_BYTES {
        if vaults.len() <= 1 {
            return Err(invalid("the device's MCP record is too large"));
        }
        vaults.remove(0);
        text = encode(&vaults);
    }
    write_private(&here.dir().join(DEVICE_FILE), text.as_bytes())
}

/// Switch the server on or off for `here`. Off keeps the paired apps (switching on again needs no new
/// token) and, in a vault the file doesn't list, writes nothing.
pub fn set_on(here: &Here, on: bool) -> io::Result<()> {
    if !on {
        let state = device_state(here);
        if !state.on {
            return Ok(());
        }
    }
    update(here, |e| {
        e.on = on;
        Ok(())
    })
}

/// Record that this device paired `client` (replacing an earlier pair of the same app). Refused with
/// [`MAX_CLIENTS_PER_VAULT`] already paired.
pub fn pair(here: &Here, client: PairedClient) -> io::Result<()> {
    if client.id <= 0 || !is_hash(&client.hash) || !is_hash(&client.scopes_hash) {
        return Err(invalid("that pairing isn't one the file would accept"));
    }
    update(here, |e| {
        e.clients.retain(|c| c.id != client.id);
        if e.clients.len() >= MAX_CLIENTS_PER_VAULT {
            return Err(invalid("this device has too many connected apps"));
        }
        e.clients.push(client);
        Ok(())
    })
}

/// Forget this device's pairing of app `id`. Nothing paired writes nothing.
pub fn unpair(here: &Here, id: i64) -> io::Result<()> {
    if !device_state(here).clients.iter().any(|c| c.id == id) {
        return Ok(());
    }
    update(here, |e| {
        e.clients.retain(|c| c.id != id);
        Ok(())
    })
}

/// Forget the vault altogether, wherever its folder was (the restore reset: a restore keeps the
/// vault id, so the entry would otherwise still say on). An error when the file can't be read or
/// rewritten, so the caller refuses the restore rather than leave a switch on.
pub fn all_off(dir: &Path, vault_id: &str) -> io::Result<()> {
    let _guard = DEVICE_FILE_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let mut vaults = read_vaults(dir)?;
    if !vaults.iter().any(|e| e.id == vault_id) {
        return Ok(());
    }
    vaults.retain(|e| e.id != vault_id);
    write_private(&dir.join(DEVICE_FILE), encode(&vaults).as_bytes())
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// `bytes` to `path`, atomically (temp file, fsync, rename) and, on Unix, readable by this user only:
/// the temp file is CREATED 0600, so the bytes are never visible under a looser mode.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.{seq}.tmp", std::process::id()));
    let tmp = path.with_file_name(name);
    let write = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(e) = write {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Some(parent) = path.parent() {
        let _ = disk::sync_dir(parent);
    }
    Ok(())
}
