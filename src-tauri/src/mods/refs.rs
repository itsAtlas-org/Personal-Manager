// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `data.projectRefs` (Mods spec §4): the JSON paths into a mod's own stored values that hold a
//! project's name, and the rewrite PM does for the mod when a project is renamed, merged or deleted.
//!
//! **The grammar** (one definition, checked at install by `package.rs`, at pack time by
//! `scripts/pack-mod.mjs`, and again here whenever a rewrite runs; the vectors every verifier must
//! agree on are `tests/mod-project-ref-paths.json`):
//!
//! ```text
//! path    = "$" segment+
//! segment = "." key        a member of an object
//!         | "[*]"          every element of an array
//!         | ".." key       a member `key` of the current value or of anything under it, down to
//!                          eight levels (an array is a level)
//! key     = [A-Za-z0-9_-]{1,64}
//! ```
//!
//! The last segment names an object key (never `[*]`): the value found there is the one that may be
//! a project's name. A path is at most 256 bytes, eight segments and two `..` segments, so what a
//! hostile value can make one path cost is bounded by the shape of the path and not only by the
//! value.
//!
//! **The rewrite** ([`rewrite_all`]) runs inside the transaction of the rename, merge or delete that
//! caused it (`projects::rename_project_satellites`, `delete_project_satellites`), so it holds the
//! DB mutex and its input is another person's data (a shared vault's member can store anything a mod
//! may store). Its rules:
//!
//! 1. Whose data: a mod whose row declares `projects.read` and holds `projectRefs`, when the grant is
//!    held **or the mod has not consented yet** (`consented_at IS NULL`). A pilot that was installed
//!    for the user holds no grant until 'Turn on'; its data must not miss a rename made in that
//!    window. A mod that was asked and denied keeps its string. A paused mod's data is rewritten too.
//! 2. The stored `project_refs` column is parsed again with this grammar, and an entry that doesn't
//!    parse (or names a key the manifest doesn't declare) is skipped. So is a row, a manifest or a
//!    value that is not the text it should be (SQLite keeps a BLOB or an invalid UTF-8 byte string
//!    in a TEXT column as it is): it costs that one mod's rewrite, counted, and never the rename.
//! 3. A value over [`MAX_VALUE_BYTES`] is skipped and counted, never parsed, so a rename can't hold
//!    the mutex on a hostile value. Neither can the sum: values are taken smallest first (one mod's
//!    big values can't use the budget up before a board is reached) and past [`MAX_PASS_BYTES`]
//!    parsed in one pass the rest are skipped and counted; the stored column is read for no more than [`MAX_REFS`]
//!    entries (the manifest's own limit; a repeated entry counts once), and a value whose rewrite
//!    would grow past [`MAX_VALUE_BYTES`] (many short matches, a long new name) is left whole.
//! 4. PM chooses the text it writes: a string equal to the old name becomes the new name PM was given
//!    (a rename) or JSON `null` (a delete, as the Pinboard's own unbinding does). Nothing else in
//!    the value is changed, and nothing a mod supplied is ever written.
//! 5. A value nested deeper than serde_json's limit (128) doesn't parse and is left as it is.
//!
//! Each value written gets its `mod_kv` revision bumped and its `updated_at` stamped, so a frame
//! holding the old copy is refused by the revision check (`MOD-4001`) instead of writing it back.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::types::Value as Sql;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use super::api::grants;
use super::ids::ModId;

/// The permission a mod must declare to have project names in its data rewritten.
pub const PERMISSION: &str = "projects.read";

/// The longest path, in bytes.
pub const MAX_PATH_BYTES: usize = 256;
/// The most segments after the `$`.
pub const MAX_SEGMENTS: usize = 8;
/// The most `..` segments in one path.
pub const MAX_DEEP_SEGMENTS: usize = 2;
/// The longest key in a path, in bytes.
pub const MAX_KEY_BYTES: usize = 64;
/// How many levels below the current value a `..` segment looks.
pub const MAX_DEPTH: usize = 8;
/// The largest stored value a rewrite will parse: 8 MiB.
pub const MAX_VALUE_BYTES: u64 = 8 * 1024 * 1024;
/// The most stored value a single rewrite pass parses in all: 64 MiB.
pub const MAX_PASS_BYTES: u64 = 64 * 1024 * 1024;
/// The longest stored `project_refs` column read: 64 entries of the longest key and path come to
/// under 24 KiB.
pub const MAX_COLUMN_BYTES: usize = 64 * 1024;
/// The most `projectRefs` entries read from a stored column: what the manifest allows.
pub const MAX_REFS: usize = 64;

/// One step of a path.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    Key(String),
    Each,
    Deep(String),
}

/// A path that parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefPath(Vec<Seg>);

fn is_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_KEY_BYTES
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Read `path` against the grammar. The error is a fixed phrase for the manifest's refusal, never
/// the path's own text.
pub fn parse(path: &str) -> Result<RefPath, &'static str> {
    if path.len() > MAX_PATH_BYTES {
        return Err("is too long");
    }
    let Some(mut rest) = path.strip_prefix('$') else {
        return Err("must start with $");
    };
    let mut segs = Vec::new();
    while !rest.is_empty() {
        let (seg, tail) = if let Some(tail) = rest.strip_prefix("[*]") {
            (Seg::Each, tail)
        } else if let Some(tail) = rest.strip_prefix("..") {
            let (key, tail) = split_key(tail);
            (Seg::Deep(key.to_owned()), tail)
        } else if let Some(tail) = rest.strip_prefix('.') {
            let (key, tail) = split_key(tail);
            (Seg::Key(key.to_owned()), tail)
        } else {
            return Err("has something that is not .key, [*] or ..key");
        };
        if let Seg::Key(k) | Seg::Deep(k) = &seg {
            if !is_key(k) {
                return Err("has a key that is empty, too long or uses other characters");
            }
        }
        segs.push(seg);
        rest = tail;
    }
    if segs.is_empty() {
        return Err("names nothing after the $");
    }
    if segs.len() > MAX_SEGMENTS {
        return Err("has too many segments");
    }
    if segs.iter().filter(|s| matches!(s, Seg::Deep(_))).count() > MAX_DEEP_SEGMENTS {
        return Err("has too many ..key segments");
    }
    if matches!(segs.last(), Some(Seg::Each)) {
        return Err("must end on an object key");
    }
    Ok(RefPath(segs))
}

/// The key a segment starts with and what follows it: up to the next `.` or `[`.
fn split_key(s: &str) -> (&str, &str) {
    let end = s.find(['.', '[']).unwrap_or(s.len());
    s.split_at(end)
}

impl RefPath {
    /// Run `hit` on the value at every object key the path selects in `root`; true when any call
    /// changed something.
    pub fn each_member(&self, root: &mut Value, hit: &mut dyn FnMut(&mut Value) -> bool) -> bool {
        walk(root, &self.0, hit)
    }
}

fn walk(node: &mut Value, segs: &[Seg], hit: &mut dyn FnMut(&mut Value) -> bool) -> bool {
    let Some((seg, rest)) = segs.split_first() else {
        return hit(node);
    };
    match seg {
        Seg::Key(key) => node
            .as_object_mut()
            .and_then(|m| m.get_mut(key))
            .is_some_and(|v| walk(v, rest, hit)),
        Seg::Each => match node.as_array_mut() {
            Some(items) => items
                .iter_mut()
                .fold(false, |ch, v| walk(v, rest, hit) | ch),
            None => false,
        },
        Seg::Deep(key) => descend(node, key, rest, 0, hit),
    }
}

/// `..key`: the member `key` of `node` and of everything under it, down to [`MAX_DEPTH`] levels.
fn descend(
    node: &mut Value,
    key: &str,
    rest: &[Seg],
    level: usize,
    hit: &mut dyn FnMut(&mut Value) -> bool,
) -> bool {
    if level > MAX_DEPTH {
        return false;
    }
    let mut changed = false;
    match node {
        Value::Object(members) => {
            if let Some(v) = members.get_mut(key) {
                changed |= walk(v, rest, hit);
            }
            for v in members.values_mut() {
                changed |= descend(v, key, rest, level + 1, hit);
            }
        }
        Value::Array(items) => {
            for v in items.iter_mut() {
                changed |= descend(v, key, rest, level + 1, hit);
            }
        }
        _ => {}
    }
    changed
}

/// What a rename, a merge or a delete did to a project's name.
#[derive(Debug, Clone, Copy)]
pub enum Change<'a> {
    /// A string equal to `from` becomes `to`.
    Rename { from: &'a str, to: &'a str },
    /// A string equal to `name` becomes `null`.
    Unbind { name: &'a str },
}

impl Change<'_> {
    fn apply(&self, value: &mut Value) -> bool {
        match *self {
            Self::Rename { from, to } if value.as_str() == Some(from) => {
                *value = Value::String(to.to_owned());
                true
            }
            Self::Unbind { name } if value.as_str() == Some(name) => {
                *value = Value::Null;
                true
            }
            _ => false,
        }
    }
}

/// What rewriting one stored value came to.
#[derive(Debug, PartialEq, Eq)]
enum Rewrite {
    /// Not JSON, or nothing in it matched.
    Unchanged,
    /// The new text.
    Done(String),
    /// The result would have passed [`MAX_VALUE_BYTES`]: nothing is written.
    TooBig,
}

/// Rewrite `text` (a stored value) by `change` along `paths`.
fn rewrite_text(text: &str, paths: &[RefPath], change: &Change<'_>) -> Rewrite {
    let Ok(mut value) = serde_json::from_str::<Value>(text) else {
        return Rewrite::Unchanged;
    };
    // What the writes add, counted as they are made so a value full of short matches and a long new
    // name stops before it is built.
    let (grow, shrink) = match *change {
        Change::Rename { from, to } => (to.len().saturating_sub(from.len()) as u64, 0),
        Change::Unbind { name } => (0, name.len() as u64),
    };
    let mut size = text.len() as u64;
    let mut too_big = false;
    let mut changed = false;
    for path in paths {
        changed |= path.each_member(&mut value, &mut |v| {
            if too_big {
                return false;
            }
            let hit = change.apply(v);
            if hit {
                size = (size + grow).saturating_sub(shrink);
                too_big = size > MAX_VALUE_BYTES;
            }
            hit
        });
        if too_big {
            return Rewrite::TooBig;
        }
    }
    if !changed {
        return Rewrite::Unchanged;
    }
    match serde_json::to_string(&value) {
        Ok(out) if out.len() as u64 <= MAX_VALUE_BYTES => Rewrite::Done(out),
        Ok(_) => Rewrite::TooBig,
        Err(_) => Rewrite::Unchanged,
    }
}

/// What one pass over the store did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    /// Values rewritten (each with its revision bumped).
    pub rewritten: usize,
    /// Values left as they were for their size: over [`MAX_VALUE_BYTES`], past [`MAX_PASS_BYTES`]
    /// for the pass, or that the rewrite would have grown past [`MAX_VALUE_BYTES`].
    pub skipped_large: usize,
    /// Mods and values left as they were because they were not the text they should be.
    pub skipped_unreadable: usize,
}

/// The entries of a stored `project_refs` column that parse, grouped by the key they name and kept
/// only for keys `declared` lists. A column that isn't the list it should be yields nothing.
fn stored_paths(column: &str, declared: impl Fn(&str) -> bool) -> BTreeMap<String, Vec<RefPath>> {
    let mut out: BTreeMap<String, Vec<RefPath>> = BTreeMap::new();
    let Ok(Value::Array(entries)) = serde_json::from_str::<Value>(column) else {
        return out;
    };
    // The manifest allows 64 entries, so the column is read for no more, and an entry repeated is
    // one path's work.
    let mut seen: BTreeSet<(&str, &str)> = BTreeSet::new();
    for entry in entries.iter().take(MAX_REFS) {
        let (Some(key), Some(path)) = (entry["key"].as_str(), entry["path"].as_str()) else {
            continue;
        };
        if !declared(key) || !seen.insert((key, path)) {
            continue;
        }
        if let Ok(path) = parse(path) {
            out.entry(key.to_owned()).or_default().push(path);
        }
    }
    out
}

/// Whether `err` says a column held another type than the one read (a BLOB in a TEXT column, bytes
/// that are not UTF-8): data that is not what it should be, as against a database that failed.
pub(crate) fn is_wrong_type(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::InvalidColumnType(..)
            | rusqlite::Error::FromSqlConversionFailure(..)
            | rusqlite::Error::Utf8Error(..)
            | rusqlite::Error::IntegralValueOutOfRange(..)
    )
}

/// Whether the data of the mod `facts` describes is rewritten (rule 1).
fn is_rewritten(conn: &Connection, facts: &grants::ModFacts) -> bool {
    if !facts.declared.contains(PERMISSION) {
        return false;
    }
    // Sealed-engine rule passed as satisfied: it decides whether a mod may RUN with a read grant, not
    // whether the person said yes to it.
    !facts.consented || grants::check(conn, facts, PERMISSION, true).is_ok()
}

/// Apply `change` to every mod's declared project references, inside the caller's transaction.
pub fn rewrite_all(conn: &Connection, change: &Change<'_>) -> rusqlite::Result<Report> {
    rewrite_within(conn, change, MAX_PASS_BYTES)
}

/// [`rewrite_all`] with the pass's parse budget given (a test passes a small one).
fn rewrite_within(
    conn: &Connection,
    change: &Change<'_>,
    pass_budget: u64,
) -> rusqlite::Result<Report> {
    let mut report = Report::default();
    // Read as SQLite types, not as text: a cell planted as a BLOB is skipped, never an error that
    // would roll the rename back.
    let rows: Vec<rusqlite::Result<(Sql, Sql)>> = {
        let mut stmt =
            conn.prepare("SELECT id, project_refs FROM mods WHERE project_refs <> '[]'")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Vec<_>>();
        rows
    };
    // Every value the pass could rewrite, found by size alone: nothing is parsed until the whole
    // list is known, so the order below is the pass's, not the order mods were planted in.
    let mut work: Vec<(u64, ModId, String, Vec<RefPath>)> = Vec::new();
    for row in rows {
        let (id, column) = match row {
            Ok(row) => row,
            Err(e) if is_wrong_type(&e) => {
                report.skipped_unreadable += 1;
                continue;
            }
            Err(e) => return Err(e),
        };
        let (Sql::Text(id), Sql::Text(column)) = (id, column) else {
            report.skipped_unreadable += 1;
            continue;
        };
        if column.len() > MAX_COLUMN_BYTES {
            report.skipped_unreadable += 1;
            continue;
        }
        let Ok(id) = ModId::parse(&id) else { continue };
        let facts = match grants::read_mod(conn, &id) {
            Ok(Some(facts)) => facts,
            Ok(None) => continue,
            Err(e) if is_wrong_type(&e) => {
                report.skipped_unreadable += 1;
                continue;
            }
            Err(e) => return Err(e),
        };
        if !is_rewritten(conn, &facts) {
            continue;
        }
        let paths = stored_paths(&column, |k| facts.data_classes.contains_key(k));
        for (key, paths) in &paths {
            let size: Option<i64> = conn
                .query_row(
                    "SELECT length(CAST(value AS BLOB)) FROM mod_kv WHERE mod_id = ?1 AND key = ?2",
                    params![id.as_str(), key],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(size) = size else { continue };
            let size = size as u64;
            if size > MAX_VALUE_BYTES {
                report.skipped_large += 1;
                continue;
            }
            work.push((size, id.clone(), key.clone(), paths.clone()));
        }
    }
    // Smallest first, so one mod's big values (or many mods') can't use the budget up before a small
    // honest value (a board is some KB) is reached; the budget runs out on the largest instead.
    work.sort_by_key(|(size, ..)| *size);
    let mut parsed_bytes = 0u64;
    for (size, id, key, paths) in work {
        if parsed_bytes + size > pass_budget {
            report.skipped_large += 1;
            continue;
        }
        let text: Option<String> = match conn
            .query_row(
                "SELECT value FROM mod_kv WHERE mod_id = ?1 AND key = ?2",
                params![id.as_str(), key],
                |r| r.get(0),
            )
            .optional()
        {
            Ok(text) => text,
            Err(e) if is_wrong_type(&e) => {
                report.skipped_unreadable += 1;
                continue;
            }
            Err(e) => return Err(e),
        };
        let Some(text) = text else { continue };
        parsed_bytes += size;
        match rewrite_text(&text, &paths, change) {
            Rewrite::Unchanged => {}
            Rewrite::TooBig => report.skipped_large += 1,
            Rewrite::Done(rewritten) => {
                conn.execute(
                    "UPDATE mod_kv SET value = ?3, revision = revision + 1, \
                         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') \
                     WHERE mod_id = ?1 AND key = ?2",
                    params![id.as_str(), key, rewritten],
                )?;
                report.rewritten += 1;
            }
        }
    }
    if report.skipped_large + report.skipped_unreadable > 0 {
        // Counts only: what the value held is never logged.
        eprintln!(
            "mods: {} stored value(s) too large and {} unreadable were left out of a project rewrite",
            report.skipped_large, report.skipped_unreadable
        );
    }
    Ok(report)
}

#[cfg(test)]
mod tests;
