// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The per-machine record of the signed index (Mods spec §1.10): `data_dir/mods-index.json`.
//!
//! **Why a file, not the DB.** A `.pmbackup` restore is a verbatim copy of the whole DB, so a seq
//! kept there would go back with it, and an old but authentic index could then un-blacklist a mod.
//! The file belongs to the machine: switching or wiping a vault leaves it (only "remove PM
//! completely", which takes the whole data folder, removes it), and it sits beside `data_dir/mods/`,
//! not in it, because that folder's contents are per vault and the ledger's alone (I-30).
//!
//! **What it holds:** the highest seq accepted and that index's sha256; the key ids seen revoked;
//! when the last good fetch was; and the newest accepted index with its signature, so the blacklist
//! applies at startup with no network. The cached pair is verified again on every read, with the
//! trusted keys its own revocations leave.
//!
//! **The seq never goes down and a revocation is never undone** ([`plan`]): the one exception is a
//! rollover-signed revocation, which may set a lower seq ([`super::index::judge`]). A restore runs
//! the backup's copy of the index through the same rules ([`Origin::Restored`]), so it takes the
//! newer of the two and can't lower anything.
//!
//! **Writing** is atomic: a new file beside it (`create_new`, so nothing there is followed or
//! reused), written, fsynced, renamed over the old one, then the folder fsynced. A crash leaves the
//! old file or the new one, and a temp file the next write removes.
//!
//! **A second copy** ([`BACKUP_FILE`], written after every save with the same bytes) is what a
//! damaged or unreadable main file falls back on, so a truncated copy, a lock or a downgrade can't
//! make the machine forget its seq or a revoked key. The main file wins whenever it is whole.
//!
//! **Reading fails closed.** No file at all is a first run: nothing seen, the floor is
//! [`MIN_INDEX_SEQ`]. A file that is there but can't be read, won't parse or breaks a rule is a
//! *fault*: the backup copy stands in if it is whole, and if it isn't ([`Loaded::fault`]) no
//! fetched index is taken at all ([`accept`] refuses with [`Stale::RecordUnusable`]), because
//! "nothing seen" would let a replayed authentic index roll the seq back, switch a blacklisted mod
//! on and trust a revoked key. The fix is to delete the file (the log line says so). A cached pair
//! that no longer verifies is dropped alone, keeping the seq and revocations, which only restrict.
//! A file that can't be *read* (an I/O error, not a parse error) is never overwritten: a passing
//! lock (an antivirus scan) must not turn into "nothing seen" on disk.

// The fetcher (slice 5's second part) and the restore commit are the callers; until they land,
// only the tests are.
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::disk::{self, failpoint, Step};
use super::index::{
    judge, parse_time, time_text, verify_index, IndexError, Seen, Stale, Verdict, VerifiedIndex,
    INDEX_CAP, MAX_SEQ, MIN_INDEX_SEQ, SIGNATURE_CAP,
};
use super::keys::{KeyId, KeyRing};

/// The file's name in `data_dir`.
pub const STATE_FILE: &str = "mods-index.json";
/// The copy of it a damaged main file falls back on (module doc).
pub const BACKUP_FILE: &str = "mods-index.json.bak";
const STATE_FORMAT: u32 = 1;
/// Both cached files in base64, plus room for the rest.
const STATE_CAP: usize = (INDEX_CAP + SIGNATURE_CAP) / 3 * 4 + (64 << 10);

/// Every read-decide-write of the file holds this, so two fetches can't each write over the other's
/// revocation.
static STATE_LOCK: Mutex<()> = Mutex::new(());

/// What this machine has seen of the index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexState {
    /// The highest seq accepted; 0 when nothing has been.
    pub seq: u64,
    /// That index's sha256 (lowercase hex).
    pub sha256: Option<String>,
    /// Key ids seen revoked; only ever grows.
    pub revoked: BTreeSet<KeyId>,
    pub last_good_fetch: Option<DateTime<Utc>>,
    /// The newest accepted index, as fetched.
    pub cached: Option<CachedIndex>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedIndex {
    pub index: Vec<u8>,
    pub signature: Vec<u8>,
}

impl IndexState {
    /// The lowest seq the next index may have: the seen one, never below the release's minimum.
    pub fn floor(&self) -> u64 {
        self.seq.max(MIN_INDEX_SEQ)
    }

    pub fn seen(&self) -> Seen<'_> {
        Seen {
            seq: self.seq,
            sha256: self.sha256.as_deref(),
            revoked: &self.revoked,
        }
    }
}

/// The file as written. Fields this PM doesn't know are ignored, so a later PM can add some and an
/// older one still reads its seq and revocations.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StateFile {
    format: u32,
    seq: u64,
    index_sha256: Option<String>,
    revoked_key_ids: Vec<String>,
    last_good_fetch: Option<String>,
    /// `index.json`, base64.
    index: Option<String>,
    /// `index.json.minisig`, base64.
    signature: Option<String>,
}

/// The file's bytes for `state`.
pub fn encode(state: &IndexState) -> io::Result<Vec<u8>> {
    let file = StateFile {
        format: STATE_FORMAT,
        seq: state.seq,
        index_sha256: state.sha256.clone(),
        revoked_key_ids: state.revoked.iter().map(KeyId::to_string).collect(),
        last_good_fetch: state.last_good_fetch.map(time_text),
        index: state.cached.as_ref().map(|c| B64.encode(&c.index)),
        signature: state.cached.as_ref().map(|c| B64.encode(&c.signature)),
    };
    serde_json::to_vec_pretty(&file).map_err(io::Error::other)
}

/// What a read found.
#[derive(Debug, Clone, Default)]
pub struct Loaded {
    pub state: IndexState,
    /// The cached index, verified again, for the blacklist at startup.
    pub index: Option<VerifiedIndex>,
    /// What was wrong with the file, if anything (already logged by [`load`]).
    pub problem: Option<String>,
    /// The file is there but couldn't be read, so it mustn't be written over.
    pub unread: bool,
    /// The record exists but is unusable (unreadable, damaged, or a format this PM doesn't read)
    /// and nothing stands in for it: the seq and revoked keys are unknown, so `state` is empty and
    /// no index may be taken against it ([`accept`]). Never set for "no file".
    pub fault: bool,
}

fn nothing_seen(problem: String) -> Loaded {
    Loaded {
        problem: Some(problem),
        fault: true,
        ..Loaded::default()
    }
}

/// What the file's bytes say, with `ring` for the cached index. Pure.
pub fn interpret(bytes: &[u8], ring: &KeyRing) -> Loaded {
    let file: StateFile = match serde_json::from_slice(bytes) {
        Ok(f) => f,
        Err(e) => return nothing_seen(format!("is unreadable ({e})")),
    };
    if file.format != STATE_FORMAT {
        return nothing_seen(format!("is format {}, and this PM reads 1", file.format));
    }
    if file.seq > MAX_SEQ {
        return nothing_seen(format!("holds seq {}, out of range", file.seq));
    }
    if let Some(sha) = &file.index_sha256 {
        if super::package::parse_sha256(sha).is_none() {
            return nothing_seen("holds a malformed sha256".into());
        }
    }
    let mut revoked = BTreeSet::new();
    for s in &file.revoked_key_ids {
        match KeyId::parse(s) {
            Some(id) => revoked.insert(id),
            None => return nothing_seen("holds a malformed key id".into()),
        };
    }
    let last_good_fetch = match file.last_good_fetch.as_deref().map(parse_time) {
        None => None,
        Some(Some(t)) => Some(t),
        Some(None) => return nothing_seen("holds a malformed fetch time".into()),
    };
    let cache = match (&file.index, &file.signature) {
        (None, None) => None,
        (Some(i), Some(s)) => match (B64.decode(i), B64.decode(s)) {
            (Ok(index), Ok(signature)) => Some(CachedIndex { index, signature }),
            _ => return nothing_seen("holds a cached index that isn't base64".into()),
        },
        _ => return nothing_seen("holds half of a cached index".into()),
    };

    let mut state = IndexState {
        seq: file.seq,
        sha256: file.index_sha256,
        revoked,
        last_good_fetch,
        cached: None,
    };
    let Some(cache) = cache else {
        return Loaded {
            state,
            ..Loaded::default()
        };
    };
    match check_cache(&state, &cache, ring) {
        Ok(v) => {
            state.cached = Some(cache);
            Loaded {
                state,
                index: Some(v),
                ..Loaded::default()
            }
        }
        Err(why) => Loaded {
            state,
            problem: Some(format!("held a cached index that was dropped ({why})")),
            ..Loaded::default()
        },
    }
}

/// The cached pair verifies with the keys the state still trusts, and is no newer than the seq on
/// record (and, at that seq, is the index on record).
fn check_cache(
    state: &IndexState,
    cache: &CachedIndex,
    ring: &KeyRing,
) -> Result<VerifiedIndex, String> {
    let v = verify_index(
        &cache.index,
        &cache.signature,
        &ring.trusted(&state.revoked),
    )
    .map_err(|e| e.to_string())?;
    if v.index.seq > state.seq {
        return Err("it is newer than the seq on record".into());
    }
    if v.index.seq == state.seq && state.sha256.as_deref() != Some(v.sha256.as_str()) {
        return Err("it isn't the index on record".into());
    }
    Ok(v)
}

pub fn state_path(data_dir: &Path) -> PathBuf {
    data_dir.join(STATE_FILE)
}

pub fn backup_path(data_dir: &Path) -> PathBuf {
    data_dir.join(BACKUP_FILE)
}

/// One of the two files: `None` when there is no such file, else what it says (a fault included).
fn read_one(path: &Path, ring: &KeyRing) -> Option<Loaded> {
    match read_capped(path) {
        Ok(None) => None,
        Ok(Some(bytes)) => Some(interpret(&bytes, ring)),
        Err(e) if e.kind() == io::ErrorKind::FileTooLarge => {
            Some(nothing_seen("is too large to be PM's".into()))
        }
        Err(e) => Some(Loaded {
            unread: true,
            ..nothing_seen(format!("couldn't be read ({e})"))
        }),
    }
}

/// Which of the two files' readings is the record (pure; module doc, "Reading fails closed"): the
/// main file when it is whole; else the backup when that is; else, if either file exists, a fault;
/// else a first run. A main file that is missing while the backup is whole is recovered from the
/// backup too. `unread` stays set when the main file couldn't be read, so it isn't written over.
fn choose(main: Option<Loaded>, backup: Option<Loaded>) -> Loaded {
    if main.as_ref().is_some_and(|m| !m.fault) {
        return main.unwrap_or_default();
    }
    match (main, backup) {
        (main, Some(b)) if !b.fault => {
            let (why, unread) = match main {
                Some(m) => (m.problem.unwrap_or_default(), m.unread),
                None => ("is missing".to_owned(), false),
            };
            Loaded {
                problem: Some(format!("{why}; its backup copy was used")),
                unread,
                ..b
            }
        }
        (None, None) => Loaded::default(),
        (Some(m), _) => m,
        (None, Some(b)) => b,
    }
}

/// Read the record (see the module doc for what each failure reads as). Logs a problem.
pub fn load(data_dir: &Path, ring: &KeyRing) -> Loaded {
    let main = read_one(&state_path(data_dir), ring);
    // The backup is read only when the main file isn't whole.
    let backup = if main.as_ref().is_some_and(|m| !m.fault) {
        None
    } else {
        read_one(&backup_path(data_dir), ring)
    };
    let loaded = choose(main, backup);
    if let Some(problem) = &loaded.problem {
        let then = if loaded.fault {
            format!(
                "no index is taken until it is readable again (delete {STATE_FILE} and {BACKUP_FILE} in the data folder to start over)"
            )
        } else {
            format!("the safety list floor is seq {}", loaded.state.floor())
        };
        eprintln!("mods: {STATE_FILE} {problem}; {then}");
    }
    loaded
}

/// The file's bytes, `None` when there is no file. Stops reading past the cap.
fn read_capped(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(STATE_CAP as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > STATE_CAP {
        return Err(io::Error::new(io::ErrorKind::FileTooLarge, "too large"));
    }
    Ok(Some(bytes))
}

/// Write `state` atomically (module doc), then the same bytes as the backup copy. The backup comes
/// second so that a crash between the two leaves a whole main file and a backup one save behind,
/// never a backup ahead of the main file it would be preferred to. A backup that can't be written
/// is logged, not an error: the record itself is in place.
pub fn save(data_dir: &Path, state: &IndexState) -> io::Result<()> {
    let bytes = encode(state)?;
    write_atomic(data_dir, STATE_FILE, &bytes)?;
    if let Err(e) = write_atomic(data_dir, BACKUP_FILE, &bytes) {
        eprintln!("mods: the backup copy of {STATE_FILE} wasn't written ({e})");
    }
    Ok(())
}

/// Start over from a record nothing stands in for: when the record is unusable ([`Loaded::fault`]:
/// the main file and its backup are both damaged, or can't be read), delete both files, so the next
/// index is taken as a first run. `Ok(false)` when the record is usable, and then nothing is touched
/// (a healthy record is never reset: it holds the seen seq and the revoked keys). The cost is real
/// and is why the Mods tab asks first: the seen seq goes back to the release's `MIN_INDEX_SEQ` and
/// revocations are forgotten until an index names them again. Under the same lock as every read and
/// write of the record, so a fetch can't land between the check and the delete.
pub fn reset_unusable(data_dir: &Path, ring: &KeyRing) -> io::Result<bool> {
    let _guard = STATE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    if !load(data_dir, ring).fault {
        return Ok(false);
    }
    for path in [state_path(data_dir), backup_path(data_dir)] {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    remove_temps(data_dir, STATE_FILE);
    remove_temps(data_dir, BACKUP_FILE);
    let _ = disk::sync_dir(data_dir);
    eprintln!("mods: {STATE_FILE} and {BACKUP_FILE} were deleted at the user's request, to start the safety list over");
    Ok(true)
}

/// `.<name>.<16 hex>.tmp`: dot-named, so nothing takes it for PM data.
fn temp_name(name: &str, random: &str) -> String {
    format!(".{name}.{random}.tmp")
}

fn is_temp_name(candidate: &str, name: &str) -> bool {
    candidate
        .strip_prefix(&format!(".{name}."))
        .and_then(|rest| rest.strip_suffix(".tmp"))
        .is_some_and(|r| r.len() == 16 && r.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    remove_temps(dir, name);
    let tmp = dir.join(temp_name(name, &disk::random_name()?));
    let written = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        failpoint(Step::Write)?;
        file.write_all(bytes)?;
        failpoint(Step::SyncFile)?;
        file.sync_all()?;
        drop(file);
        failpoint(Step::Rename)?;
        fs::rename(&tmp, dir.join(name))
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    // The new file is in place; flush the folder so the rename survives a crash too.
    disk::sync_dir(dir)
}

/// Temp files an interrupted write left. Only names this module makes, and only plain files.
fn remove_temps(dir: &Path, name: &str) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let is_ours = entry
            .file_name()
            .to_str()
            .is_some_and(|n| is_temp_name(n, name));
        if is_ours && entry.file_type().is_ok_and(|t| t.is_file()) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

// ---- Deciding (pure) ----------------------------------------------------------------------------

/// Where an index pair came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Fetched from PM-Mods now.
    Fetched,
    /// The copy a `.pmbackup` carried, at restore commit: judged the same way, so the newer of the
    /// two wins, but never a fetch, good or not.
    Restored,
}

/// What accepting an index changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub verdict: Verdict,
    /// The state to write, `None` when nothing changes.
    pub next: Option<IndexState>,
}

/// The state after `fetched` is judged against `state` (`minimum`: the release's [`MIN_INDEX_SEQ`]).
/// The seq follows the index taken; revocations are only ever added; the last good fetch moves only
/// on a good fetch.
pub fn plan(
    state: &IndexState,
    fetched: &VerifiedIndex,
    now: DateTime<Utc>,
    minimum: u64,
    origin: Origin,
) -> Plan {
    let verdict = judge(&state.seen(), fetched, now, minimum);
    let fetch_time = |good: bool| {
        if good && origin == Origin::Fetched {
            Some(now)
        } else {
            state.last_good_fetch
        }
    };
    let cached = Some(CachedIndex {
        index: fetched.bytes.clone(),
        signature: fetched.signature.clone(),
    });
    let next = match verdict {
        Verdict::Refused(_) => None,
        Verdict::Same { good } => Some(IndexState {
            last_good_fetch: fetch_time(good),
            cached,
            ..state.clone()
        }),
        Verdict::Newer { good, .. } => Some(IndexState {
            seq: fetched.index.seq,
            sha256: Some(fetched.sha256.clone()),
            revoked: state.revoked.union(&fetched.revokes).copied().collect(),
            last_good_fetch: fetch_time(good),
            cached,
        }),
    };
    Plan {
        verdict,
        next: next.filter(|n| n != state),
    }
}

// ---- The edge -----------------------------------------------------------------------------------

/// What [`accept`] did.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// Refused at verification, or the acceptance verdict.
    pub verdict: Result<Verdict, IndexError>,
    /// The index in force now: the one just taken, else the cached one (none when the record is
    /// unusable, [`Stale::RecordUnusable`]). Its blacklist always applies; its catalogue only while
    /// [`super::index::Index::current_at`] holds.
    pub current: Option<VerifiedIndex>,
    /// The state as it now stands.
    pub state: IndexState,
    /// Why the new state isn't on disk, when it isn't. `current` still applies for this run.
    pub unsaved: Option<String>,
}

/// Verify a fetched (or restored) `index.json` and its signature with the keys this machine still
/// trusts, judge it, and record the result: read, decide and write under one lock. Blocking file
/// I/O: call it off the async runtime, and never under the DB lock.
pub fn accept(
    data_dir: &Path,
    ring: &KeyRing,
    index: &[u8],
    signature: &[u8],
    now: DateTime<Utc>,
    origin: Origin,
) -> Outcome {
    accept_with_minimum(data_dir, ring, index, signature, now, origin, MIN_INDEX_SEQ)
}

fn accept_with_minimum(
    data_dir: &Path,
    ring: &KeyRing,
    index: &[u8],
    signature: &[u8],
    now: DateTime<Utc>,
    origin: Origin,
    minimum: u64,
) -> Outcome {
    let _guard = STATE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let loaded = load(data_dir, ring);
    if loaded.fault {
        // Fail closed: with the seq and the revoked keys unknown, any authentic index (a replay
        // included) would look new, so none is taken, written, or used for this run.
        return Outcome {
            verdict: Ok(Verdict::Refused(Stale::RecordUnusable)),
            current: None,
            state: loaded.state,
            unsaved: None,
        };
    }
    let fetched = match verify_index(index, signature, &ring.trusted(&loaded.state.revoked)) {
        Ok(v) => v,
        Err(e) => {
            return Outcome {
                verdict: Err(e),
                current: loaded.index,
                state: loaded.state,
                unsaved: None,
            }
        }
    };
    let plan = plan(&loaded.state, &fetched, now, minimum, origin);
    let current = match plan.verdict {
        Verdict::Refused(_) => loaded.index,
        Verdict::Same { .. } | Verdict::Newer { .. } => Some(fetched),
    };
    let Some(next) = plan.next else {
        return Outcome {
            verdict: Ok(plan.verdict),
            current,
            state: loaded.state,
            unsaved: None,
        };
    };
    let unsaved = if loaded.unread {
        Some(format!(
            "{STATE_FILE} couldn't be read, so it isn't written over"
        ))
    } else {
        save(data_dir, &next).err().map(|e| e.to_string())
    };
    if let Some(why) = &unsaved {
        eprintln!("mods: the accepted index wasn't recorded: {why}");
    }
    Outcome {
        verdict: Ok(plan.verdict),
        current,
        state: next,
        unsaved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::disk::fail_at;
    use crate::mods::index::tests::{good_index, signed, t, verified_by, Keys};
    use crate::mods::index::{self, Stale};
    use serde_json::{json, Value};

    const NOW: &str = "2027-01-15T00:00:00Z";

    fn scratch() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn rotation(keys: &Keys, seq: u64) -> Value {
        let mut index = good_index(seq);
        index["revokedKeyIds"] = json!([keys.main_id().to_string()]);
        index
    }

    // ---- The pure planner ----------------------------------------------------------------------

    #[test]
    fn a_newer_index_replaces_the_record_and_a_good_one_stamps_the_fetch() {
        let keys = Keys::new();
        let v = verified_by(&good_index(3), &keys.main, &keys);
        let now = t(NOW);
        let p = plan(&IndexState::default(), &v, now, 1, Origin::Fetched);
        assert_eq!(
            p.verdict,
            Verdict::Newer {
                good: true,
                rotation: false
            }
        );
        let next = p.next.unwrap();
        assert_eq!(
            (next.seq, next.sha256.as_deref()),
            (3, Some(v.sha256.as_str()))
        );
        assert_eq!(next.last_good_fetch, Some(now));
        assert_eq!(next.cached.as_ref().unwrap().index, v.bytes);
        assert_eq!(next.floor(), 3);

        // The same index again: only the fetch time moves.
        let later = now + chrono::Duration::hours(6);
        let again = plan(&next, &v, later, 1, Origin::Fetched);
        assert_eq!(again.verdict, Verdict::Same { good: true });
        assert_eq!(
            again.next.unwrap(),
            IndexState {
                last_good_fetch: Some(later),
                ..next.clone()
            }
        );
        // Nothing to write when nothing changes (an expired copy of the index already held).
        let expired = plan(&next, &v, t("2027-03-01T00:00:00Z"), 1, Origin::Fetched);
        assert_eq!(
            (expired.verdict, expired.next),
            (Verdict::Same { good: false }, None)
        );
    }

    #[test]
    fn a_refused_index_changes_nothing() {
        let keys = Keys::new();
        let now = t(NOW);
        let held = plan(
            &IndexState::default(),
            &verified_by(&good_index(5), &keys.main, &keys),
            now,
            1,
            Origin::Fetched,
        )
        .next
        .unwrap();
        // Older, the same seq with other bytes, below the minimum.
        let older = verified_by(&good_index(4), &keys.main, &keys);
        assert_eq!(
            plan(&held, &older, now, 1, Origin::Fetched),
            Plan {
                verdict: Verdict::Refused(Stale::Older { seq: 4, seen: 5 }),
                next: None
            }
        );
        let mut other = good_index(5);
        other["mods"] = json!([]);
        let other = verified_by(&other, &keys.main, &keys);
        assert_eq!(
            plan(&held, &other, now, 1, Origin::Fetched).verdict,
            Verdict::Refused(Stale::Equivocation { seq: 5 })
        );
        assert_eq!(
            plan(&IndexState::default(), &older, now, 9, Origin::Fetched),
            Plan {
                verdict: Verdict::Refused(Stale::BelowMinimum { seq: 4, minimum: 9 }),
                next: None
            }
        );
    }

    #[test]
    fn an_expired_newer_index_is_recorded_without_counting_as_a_fetch() {
        let keys = Keys::new();
        let first = verified_by(&good_index(1), &keys.main, &keys);
        let held = plan(&IndexState::default(), &first, t(NOW), 1, Origin::Fetched)
            .next
            .unwrap();
        let late = t("2027-03-01T00:00:00Z");
        let v2 = verified_by(&good_index(2), &keys.main, &keys);
        let p = plan(&held, &v2, late, 1, Origin::Fetched);
        assert_eq!(
            p.verdict,
            Verdict::Newer {
                good: false,
                rotation: false
            }
        );
        let next = p.next.unwrap();
        assert_eq!(next.seq, 2);
        assert_eq!(next.last_good_fetch, Some(t(NOW)));
    }

    #[test]
    fn revocations_only_ever_grow() {
        let keys = Keys::new();
        let now = t(NOW);
        let r = verified_by(&rotation(&keys, 21), &keys.rollover, &keys);
        let held = IndexState {
            seq: 40,
            sha256: Some("x".repeat(64)),
            ..IndexState::default()
        };
        let p = plan(&held, &r, now, 1, Origin::Fetched);
        assert_eq!(
            p.verdict,
            Verdict::Newer {
                good: true,
                rotation: true
            }
        );
        let after = p.next.unwrap();
        // The rotation set the seq it carried, lower than the one held.
        assert_eq!(after.seq, 21);
        assert_eq!(after.revoked, BTreeSet::from([keys.main_id()]));
        // A later index without the revocation (signed by the rollover; the main key is refused
        // now) keeps it.
        let plain = verified_by(&good_index(22), &keys.rollover, &keys);
        let later = plan(&after, &plain, now, 1, Origin::Fetched).next.unwrap();
        assert_eq!(later.seq, 22);
        assert_eq!(later.revoked, BTreeSet::from([keys.main_id()]));
    }

    #[test]
    fn a_restored_copy_is_judged_like_a_fetch_but_never_counts_as_one() {
        let keys = Keys::new();
        let now = t(NOW);
        let local = plan(
            &IndexState::default(),
            &verified_by(&good_index(7), &keys.main, &keys),
            now,
            1,
            Origin::Fetched,
        )
        .next
        .unwrap();
        // An older backup's copy is refused: the local record stands.
        let old = verified_by(&good_index(5), &keys.main, &keys);
        assert_eq!(plan(&local, &old, now, 1, Origin::Restored).next, None);
        // A newer one raises the floor, but isn't a fetch.
        let newer = verified_by(&good_index(9), &keys.main, &keys);
        let later = now + chrono::Duration::days(1);
        let next = plan(&local, &newer, later, 1, Origin::Restored)
            .next
            .unwrap();
        assert_eq!(next.seq, 9);
        assert_eq!(next.last_good_fetch, Some(now));
        let fresh = plan(&IndexState::default(), &newer, later, 1, Origin::Restored);
        assert_eq!(fresh.next.unwrap().last_good_fetch, None);
    }

    // ---- The file ------------------------------------------------------------------------------

    fn held_state(keys: &Keys) -> IndexState {
        let mut rotated = rotation(keys, 4);
        rotated["mods"] = json!([]);
        let v = verified_by(&rotated, &keys.rollover, keys);
        plan(&IndexState::default(), &v, t(NOW), 1, Origin::Fetched)
            .next
            .unwrap()
    }

    #[test]
    fn the_file_round_trips_and_the_cache_is_verified_again() {
        let keys = Keys::new();
        let state = held_state(&keys);
        let loaded = interpret(&encode(&state).unwrap(), &keys.ring());
        assert_eq!(loaded.problem, None);
        assert_eq!(loaded.state, state);
        let v = loaded.index.unwrap();
        assert_eq!(v.index.seq, 4);
        assert_eq!(v.signer, keys.rollover_id());
        // The format, pinned: what a person finds in the file.
        let file: Value = serde_json::from_slice(&encode(&state).unwrap()).unwrap();
        assert_eq!(file["format"], 1);
        assert_eq!(file["seq"], 4);
        assert_eq!(file["revokedKeyIds"], json!([keys.main_id().to_string()]));
        assert_eq!(file["lastGoodFetch"], NOW);
        assert_eq!(file["indexSha256"], json!(state.sha256));
        // Fields a later PM adds are ignored.
        let mut more = file.clone();
        more["somethingNew"] = json!({"x": 1});
        let loaded = interpret(&serde_json::to_vec(&more).unwrap(), &keys.ring());
        assert_eq!((loaded.problem, loaded.state), (None, state));
        // Nothing cached is fine too.
        let bare = IndexState {
            seq: 3,
            ..IndexState::default()
        };
        assert_eq!(interpret(&encode(&bare).unwrap(), &keys.ring()).state, bare);
    }

    #[test]
    fn a_corrupt_file_reads_as_a_fault_with_nothing_seen() {
        let keys = Keys::new();
        let good: Value = serde_json::from_slice(&encode(&held_state(&keys)).unwrap()).unwrap();
        let broken = |change: &dyn Fn(&mut Value)| {
            let mut v = good.clone();
            change(&mut v);
            serde_json::to_vec(&v).unwrap()
        };
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("not json", b"{\"format\":1,".to_vec()),
            ("empty", Vec::new()),
            (
                "truncated",
                encode(&held_state(&keys)).unwrap()[..40].to_vec(),
            ),
            ("format", broken(&|v| v["format"] = json!(2))),
            ("seq type", broken(&|v| v["seq"] = json!("4"))),
            ("seq range", broken(&|v| v["seq"] = json!(MAX_SEQ + 1))),
            ("sha", broken(&|v| v["indexSha256"] = json!("ABC"))),
            ("key id", broken(&|v| v["revokedKeyIds"] = json!(["nope"]))),
            ("time", broken(&|v| v["lastGoodFetch"] = json!("yesterday"))),
            ("half a cache", broken(&|v| v["signature"] = Value::Null)),
            ("not base64", broken(&|v| v["index"] = json!("%%%"))),
        ];
        for (what, bytes) in cases {
            let loaded = interpret(&bytes, &keys.ring());
            assert!(loaded.problem.is_some(), "{what}");
            assert_eq!(loaded.state, IndexState::default(), "{what}");
            assert!(loaded.index.is_none() && !loaded.unread, "{what}");
            assert!(loaded.fault, "{what}: nothing may be taken against it");
            assert_eq!(loaded.state.floor(), MIN_INDEX_SEQ, "{what}");
        }
    }

    #[test]
    fn a_cache_that_no_longer_verifies_is_dropped_alone() {
        let keys = Keys::new();
        let state = held_state(&keys);
        // Another machine's keys: the pair doesn't verify, the seq and revocations stay.
        let stranger = Keys::new();
        let loaded = interpret(&encode(&state).unwrap(), &stranger.ring());
        assert!(loaded.problem.is_some());
        assert!(loaded.index.is_none());
        assert_eq!(
            loaded.state,
            IndexState {
                cached: None,
                ..state.clone()
            }
        );
        // A pair newer than the record, or not the index on record at its seq.
        for tampered in [
            IndexState {
                seq: 3,
                ..state.clone()
            },
            IndexState {
                sha256: Some("0".repeat(64)),
                ..state.clone()
            },
        ] {
            let loaded = interpret(&encode(&tampered).unwrap(), &keys.ring());
            assert!(loaded.problem.is_some());
            assert!(loaded.index.is_none() && loaded.state.cached.is_none());
            assert_eq!(loaded.state.seq, tampered.seq);
        }
        // A pair older than the seq on record (PM never writes one, but a higher floor only
        // restricts) is kept.
        let raised = IndexState {
            seq: 9,
            sha256: None,
            ..state
        };
        let loaded = interpret(&encode(&raised).unwrap(), &keys.ring());
        assert_eq!(loaded.problem, None);
        assert_eq!(loaded.index.unwrap().index.seq, 4);
    }

    #[test]
    fn save_and_load_through_the_disk() {
        let dir = scratch();
        let keys = Keys::new();
        assert_eq!(load(dir.path(), &keys.ring()).state, IndexState::default());
        let state = held_state(&keys);
        save(dir.path(), &state).unwrap();
        let loaded = load(dir.path(), &keys.ring());
        assert_eq!((loaded.problem, loaded.state), (None, state.clone()));
        // Written again over the old file, with no temp file left beside it.
        let next = IndexState {
            seq: 5,
            cached: None,
            sha256: None,
            ..state
        };
        save(dir.path(), &next).unwrap();
        assert_eq!(load(dir.path(), &keys.ring()).state, next);
        let mut names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, [STATE_FILE, BACKUP_FILE]);
        assert_eq!(
            fs::read(state_path(dir.path())).unwrap(),
            fs::read(backup_path(dir.path())).unwrap()
        );
    }

    #[test]
    fn a_failed_write_at_any_step_leaves_the_old_file_whole() {
        let dir = scratch();
        let keys = Keys::new();
        let old = held_state(&keys);
        save(dir.path(), &old).unwrap();
        let before = fs::read(state_path(dir.path())).unwrap();
        let new = IndexState {
            seq: 99,
            ..IndexState::default()
        };
        for step in [Step::Write, Step::SyncFile, Step::Rename] {
            fail_at(Some((step, 0)));
            let got = save(dir.path(), &new);
            fail_at(None);
            assert!(got.is_err(), "{step:?}");
            assert_eq!(
                fs::read(state_path(dir.path())).unwrap(),
                before,
                "{step:?}"
            );
            assert_eq!(
                fs::read_dir(dir.path()).unwrap().count(),
                2,
                "{step:?}: the record and its backup, no temp file left"
            );
        }
        // The backup's own write failing leaves the new record in place, and says nothing is wrong
        // with the save.
        fail_at(Some((Step::Write, 1)));
        assert!(save(dir.path(), &new).is_ok());
        fail_at(None);
        assert_eq!(load(dir.path(), &keys.ring()).state, new);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
        save(dir.path(), &old).unwrap();
        // A failed folder flush comes after the rename: the new file is in place, and the error
        // still says the write isn't known to be durable.
        fail_at(Some((Step::SyncDir, 0)));
        assert!(save(dir.path(), &new).is_err());
        fail_at(None);
        assert_eq!(load(dir.path(), &keys.ring()).state, new);
    }

    #[test]
    fn a_crash_s_temp_file_is_ignored_and_then_removed() {
        let dir = scratch();
        let keys = Keys::new();
        let state = held_state(&keys);
        save(dir.path(), &state).unwrap();
        // A crash between the write and the rename leaves a whole or partial temp file.
        let leftover = dir.path().join(temp_name(STATE_FILE, "0123456789abcdef"));
        fs::write(&leftover, b"{\"format\":1,\"seq\":9999").unwrap();
        // Names that aren't this module's are never touched. The upper-case one uses different
        // digits from the leftover on purpose: NTFS and APFS fold case, so with the SAME digits it
        // is the leftover's own file there (written over, then rightly removed with it).
        let others = [
            ".mods-index.json.FEDCBA9876543210.tmp",
            ".mods-index.json.tmp",
            "mods-index.json.0123456789abcdef.tmp",
            ".vault-pointer.json.0123456789abcdef.tmp",
        ];
        for o in others {
            fs::write(dir.path().join(o), b"x").unwrap();
        }
        assert_eq!(load(dir.path(), &keys.ring()).state, state);
        save(dir.path(), &state).unwrap();
        assert!(!leftover.exists());
        for o in others {
            assert!(dir.path().join(o).exists(), "{o}");
        }
        assert!(is_temp_name(
            &temp_name(STATE_FILE, &disk::random_name().unwrap()),
            STATE_FILE
        ));
        // The matcher itself is exact-case on every platform (we only ever make lower-case names),
        // so the upper-case spelling of the leftover's name is not ours even where a filesystem
        // would call it the same file.
        assert!(is_temp_name(
            ".mods-index.json.0123456789abcdef.tmp",
            STATE_FILE
        ));
        assert!(!is_temp_name(
            ".mods-index.json.0123456789ABCDEF.tmp",
            STATE_FILE
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_file_that_cant_be_read_is_never_written_over() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch();
        let keys = Keys::new();
        let state = held_state(&keys);
        save(dir.path(), &state).unwrap();
        let path = state_path(dir.path());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::File::open(&path).is_ok() {
            // Running as root: permissions can't make it unreadable here.
            return;
        }
        // The backup copy stands in: the seq and the revocation are not forgotten.
        let loaded = load(dir.path(), &keys.ring());
        assert!(loaded.unread && loaded.problem.is_some() && !loaded.fault);
        assert_eq!(loaded.state, state);
        let (bytes, sig) = signed(&good_index(5), &keys.rollover);
        let out = accept(
            dir.path(),
            &keys.ring(),
            &bytes,
            &sig,
            t(NOW),
            Origin::Fetched,
        );
        // Judged against the backup and taken for this run, but the unreadable file is left alone.
        assert_eq!(
            out.verdict,
            Ok(Verdict::Newer {
                good: true,
                rotation: false
            })
        );
        assert!(out.unsaved.is_some());
        assert_eq!(out.current.unwrap().index.seq, 5);
        let (old, old_sig) = signed(&good_index(2), &keys.rollover);
        let replay = accept(
            dir.path(),
            &keys.ring(),
            &old,
            &old_sig,
            t(NOW),
            Origin::Fetched,
        );
        assert_eq!(
            replay.verdict,
            Ok(Verdict::Refused(Stale::Older { seq: 2, seen: 4 }))
        );

        // With no backup either, nothing is known: no index is taken, and the file is left alone.
        fs::remove_file(backup_path(dir.path())).unwrap();
        let loaded = load(dir.path(), &keys.ring());
        assert!(loaded.unread && loaded.fault);
        let out = accept(
            dir.path(),
            &keys.ring(),
            &bytes,
            &sig,
            t(NOW),
            Origin::Fetched,
        );
        assert_eq!(out.verdict, Ok(Verdict::Refused(Stale::RecordUnusable)));
        assert!(out.current.is_none());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(load(dir.path(), &keys.ring()).state, state);
    }

    #[test]
    fn an_oversized_file_is_a_fault() {
        let dir = scratch();
        let keys = Keys::new();
        let file = fs::File::create(state_path(dir.path())).unwrap();
        file.set_len(STATE_CAP as u64 + 1).unwrap();
        let loaded = load(dir.path(), &keys.ring());
        assert!(loaded.problem.is_some() && !loaded.unread && loaded.fault);
        assert_eq!(loaded.state, IndexState::default());
    }

    // ---- accept, end to end --------------------------------------------------------------------

    #[test]
    fn accept_records_what_it_takes_and_keeps_the_rest() {
        let dir = scratch();
        let keys = Keys::new();
        let ring = keys.ring();
        let now = t(NOW);
        let fetch = |index: &Value, key| {
            let (bytes, sig) = signed(index, key);
            accept(dir.path(), &ring, &bytes, &sig, now, Origin::Fetched)
        };

        let first = fetch(&good_index(3), &keys.main);
        assert_eq!(
            first.verdict,
            Ok(Verdict::Newer {
                good: true,
                rotation: false
            })
        );
        assert_eq!(first.unsaved, None);
        assert_eq!(load(dir.path(), &ring).state, first.state);

        // A replay: refused, the record stands, the cached index stays in force.
        let replay = fetch(&good_index(2), &keys.main);
        assert_eq!(
            replay.verdict,
            Ok(Verdict::Refused(Stale::Older { seq: 2, seen: 3 }))
        );
        assert_eq!(replay.current.unwrap().index.seq, 3);
        assert_eq!(load(dir.path(), &ring).state, first.state);

        // A forgery: refused at verification, nothing written.
        let stranger = crate::mods::package::tests::TestKey::new();
        let forged = fetch(&good_index(50), &stranger);
        assert_eq!(
            forged.verdict.unwrap_err().refusal,
            index::IndexRefusal::UntrustedKey
        );
        assert_eq!(forged.current.unwrap().index.seq, 3);
        assert_eq!(load(dir.path(), &ring).state.seq, 3);

        // The rotation: the main key is refused from then on, for good.
        let rotated = fetch(&rotation(&keys, 4), &keys.rollover);
        assert_eq!(
            rotated.verdict,
            Ok(Verdict::Newer {
                good: true,
                rotation: true
            })
        );
        assert_eq!(
            load(dir.path(), &ring).state.revoked,
            BTreeSet::from([keys.main_id()])
        );
        let by_main = fetch(&good_index(5), &keys.main);
        assert_eq!(
            by_main.verdict.unwrap_err().refusal,
            index::IndexRefusal::UntrustedKey
        );
        assert!(!ring
            .trusted(&load(dir.path(), &ring).state.revoked)
            .trusts(keys.main_id()));
        // Revocation can't be undone by a later index that leaves it out.
        let plain = fetch(&good_index(6), &keys.rollover);
        assert_eq!(
            plain.verdict,
            Ok(Verdict::Newer {
                good: true,
                rotation: false
            })
        );
        assert_eq!(
            load(dir.path(), &ring).state.revoked,
            BTreeSet::from([keys.main_id()])
        );
        // And the cached index is what startup reads, verified with the keys left.
        let at_startup = load(dir.path(), &ring);
        assert_eq!(at_startup.index.unwrap().index.seq, 6);
    }

    #[test]
    fn accept_honours_the_minimum_and_a_corrupt_file_never_lowers_it() {
        let dir = scratch();
        let keys = Keys::new();
        let ring = keys.ring();
        // No file: a first run, held to the release's minimum.
        let (bytes, sig) = signed(&good_index(4), &keys.main);
        let out = accept_with_minimum(dir.path(), &ring, &bytes, &sig, t(NOW), Origin::Fetched, 5);
        assert_eq!(
            out.verdict,
            Ok(Verdict::Refused(Stale::BelowMinimum { seq: 4, minimum: 5 }))
        );
        assert!(!state_path(dir.path()).exists());
        let (bytes, sig) = signed(&good_index(5), &keys.main);
        let out = accept_with_minimum(dir.path(), &ring, &bytes, &sig, t(NOW), Origin::Fetched, 5);
        assert!(out.verdict.unwrap().is_good());
        assert_eq!(load(dir.path(), &ring).state.seq, 5);
    }

    /// The replay a damaged record must not allow: seq 40 with the main key revoked, the file
    /// damaged or unreadable, then an old authentic index (and one signed by the revoked key).
    fn seq_40_with_main_revoked(dir: &Path, keys: &Keys) -> IndexState {
        let mut rotated = rotation(keys, 40);
        rotated["mods"] = json!([]);
        let (bytes, sig) = signed(&rotated, &keys.rollover);
        let out = accept(dir, &keys.ring(), &bytes, &sig, t(NOW), Origin::Fetched);
        assert_eq!(out.unsaved, None);
        out.state
    }

    #[test]
    fn a_damaged_record_with_no_backup_takes_no_index_and_is_not_written_over() {
        let keys = Keys::new();
        let ring = keys.ring();
        for damage in ["truncate", "format-2", "empty"] {
            let dir = scratch();
            seq_40_with_main_revoked(dir.path(), &keys);
            fs::remove_file(backup_path(dir.path())).unwrap();
            let bytes = fs::read(state_path(dir.path())).unwrap();
            let damaged = match damage {
                "truncate" => bytes[..bytes.len() / 2].to_vec(),
                "format-2" => {
                    let mut v: Value = serde_json::from_slice(&bytes).unwrap();
                    v["format"] = json!(2);
                    serde_json::to_vec(&v).unwrap()
                }
                _ => b"".to_vec(),
            };
            fs::write(state_path(dir.path()), &damaged).unwrap();
            assert!(load(dir.path(), &ring).fault, "{damage}");
            // The older authentic index (seq 12), a newer one, and the revoked key's own.
            for (index, key) in [
                (good_index(12), &keys.rollover),
                (good_index(41), &keys.rollover),
                (good_index(99), &keys.main),
            ] {
                let (b, s) = signed(&index, key);
                let out = accept(dir.path(), &ring, &b, &s, t(NOW), Origin::Fetched);
                assert_eq!(
                    out.verdict,
                    Ok(Verdict::Refused(Stale::RecordUnusable)),
                    "{damage}"
                );
                assert!(out.current.is_none() && out.unsaved.is_none(), "{damage}");
            }
            // Nothing was written: the damaged file is as it was, and no backup appeared.
            assert_eq!(
                fs::read(state_path(dir.path())).unwrap(),
                damaged,
                "{damage}"
            );
            assert!(!backup_path(dir.path()).exists(), "{damage}");
            // Deleting the damaged file starts over, as the log line says.
            fs::remove_file(state_path(dir.path())).unwrap();
            let (b, s) = signed(&good_index(MIN_INDEX_SEQ.max(1)), &keys.rollover);
            let out = accept(dir.path(), &ring, &b, &s, t(NOW), Origin::Fetched);
            assert!(out.verdict.unwrap().is_good(), "{damage}");
        }
    }

    #[test]
    fn a_damaged_main_file_falls_back_on_the_backup_so_nothing_is_forgotten() {
        let keys = Keys::new();
        let ring = keys.ring();
        let dir = scratch();
        let before = seq_40_with_main_revoked(dir.path(), &keys);
        // The main file is truncated; the backup stands in with the seq, the revocation and the
        // cached index, so the replay is judged against them.
        let bytes = fs::read(state_path(dir.path())).unwrap();
        fs::write(state_path(dir.path()), &bytes[..bytes.len() / 2]).unwrap();
        let loaded = load(dir.path(), &ring);
        assert!(!loaded.fault && loaded.problem.is_some());
        assert_eq!(loaded.state, before);
        assert_eq!(loaded.index.unwrap().index.seq, 40);
        replay_is_refused_against_backup(dir.path(), &keys);
        // A newer index is taken, and the main file is whole again (the backup is not stale).
        let (b, s) = signed(&good_index(41), &keys.rollover);
        let out = accept(dir.path(), &ring, &b, &s, t(NOW), Origin::Fetched);
        assert!(out.verdict.unwrap().is_good());
        let loaded = load(dir.path(), &ring);
        assert_eq!((loaded.problem, loaded.state.seq), (None, 41));
        assert_eq!(loaded.state.revoked, before.revoked);

        // A missing main file with a whole backup is recovered the same way.
        fs::remove_file(state_path(dir.path())).unwrap();
        let loaded = load(dir.path(), &ring);
        assert!(!loaded.fault && loaded.state.seq == 41 && !loaded.unread);
        // Both files damaged: a fault.
        fs::write(state_path(dir.path()), b"{").unwrap();
        fs::write(backup_path(dir.path()), b"{").unwrap();
        assert!(load(dir.path(), &ring).fault);
        // A main file missing with a damaged backup: a fault too, not a first run.
        fs::remove_file(state_path(dir.path())).unwrap();
        assert!(load(dir.path(), &ring).fault);
        // No file of either kind is a first run.
        fs::remove_file(backup_path(dir.path())).unwrap();
        let first = load(dir.path(), &ring);
        assert!(!first.fault && first.problem.is_none());
    }

    #[test]
    fn a_reset_deletes_a_record_nothing_stands_in_for_and_the_next_index_is_a_first_run() {
        let dir = scratch();
        let keys = Keys::new();
        let ring = keys.ring();
        // An index taken and saved, then both files damaged: a fault.
        let (bytes, sig) = signed(&good_index(40), &keys.main);
        accept(dir.path(), &ring, &bytes, &sig, t(NOW), Origin::Fetched);
        fs::write(state_path(dir.path()), b"{").unwrap();
        fs::write(backup_path(dir.path()), b"{").unwrap();
        // A leftover temp file of a crashed write goes with them.
        let temp = dir.path().join(temp_name(STATE_FILE, "0123456789abcdef"));
        fs::write(&temp, b"x").unwrap();
        assert!(load(dir.path(), &ring).fault);
        assert!(reset_unusable(dir.path(), &ring).unwrap());
        assert!(!state_path(dir.path()).exists());
        assert!(!backup_path(dir.path()).exists());
        assert!(!temp.exists());
        let after = load(dir.path(), &ring);
        assert!(!after.fault && after.state == IndexState::default());
        // The next index is taken as the first one, whatever its seq.
        let (older, sig) = signed(&good_index(12), &keys.main);
        let out = accept(dir.path(), &ring, &older, &sig, t(NOW), Origin::Fetched);
        assert!(
            matches!(out.verdict, Ok(Verdict::Newer { .. })),
            "{:?}",
            out.verdict
        );
    }

    #[test]
    fn a_reset_never_touches_a_record_that_is_whole_or_stood_in_for_or_not_there() {
        let dir = scratch();
        let keys = Keys::new();
        let ring = keys.ring();
        // No file: a first run, nothing to reset.
        assert!(!reset_unusable(dir.path(), &ring).unwrap());
        let (bytes, sig) = signed(&good_index(40), &keys.main);
        accept(dir.path(), &ring, &bytes, &sig, t(NOW), Origin::Fetched);
        let main = fs::read(state_path(dir.path())).unwrap();
        assert!(!reset_unusable(dir.path(), &ring).unwrap());
        assert_eq!(fs::read(state_path(dir.path())).unwrap(), main);
        // The main file damaged but the backup whole: the backup stands in, and a reset would only
        // throw away the seq and the revoked keys it holds.
        fs::write(state_path(dir.path()), b"{").unwrap();
        assert!(!reset_unusable(dir.path(), &ring).unwrap());
        assert!(backup_path(dir.path()).exists());
        assert_eq!(load(dir.path(), &ring).state.seq, 40);
    }

    fn replay_is_refused_against_backup(dir: &Path, keys: &Keys) {
        let ring = keys.ring();
        let (older, sig) = signed(&good_index(12), &keys.rollover);
        let out = accept(dir, &ring, &older, &sig, t(NOW), Origin::Fetched);
        assert_eq!(
            out.verdict,
            Ok(Verdict::Refused(Stale::Older { seq: 12, seen: 40 }))
        );
        assert_eq!(out.current.unwrap().index.seq, 40);
        let (by_main, sig) = signed(&good_index(99), &keys.main);
        let out = accept(dir, &ring, &by_main, &sig, t(NOW), Origin::Fetched);
        assert_eq!(
            out.verdict.unwrap_err().refusal,
            index::IndexRefusal::UntrustedKey
        );
    }
}
