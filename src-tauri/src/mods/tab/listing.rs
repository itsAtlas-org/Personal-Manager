// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `list_mods`: one DTO per installed row, built from the row and its stored manifest (Mods spec
//! §1.13). The manifest is read as stored (verbatim, as verified), tolerantly: a row whose manifest
//! no longer reads is still listed, parked, with what can be said of it, so the person can uninstall
//! it. Nothing here reads a file: whether an icon exists is a fact of the row's file list.
//!
//! Each permission carries its manifest "why", its class, when PM asks, the stored decision and the
//! access as the dispatcher would answer it now (`grants::decide`, D3 and the sealed engine
//! included), so the tab shows what is true and not what the policy table alone would say.

use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

use super::words::{self, RowFacts, Standing};
use crate::mods::ai::{self as money, MonthUse};
use crate::mods::api::grants::{self, Decision, Facts, Outcome, Refused};
use crate::mods::api::permissions::{self, Class, PolicyTier, When};
use crate::mods::autoinstall::{self, Pilot};
use crate::mods::index::Index;
use crate::mods::ledger::{self, Resumed, Waiting, WaitingKind};
use crate::mods::package::ApiRange;
use crate::mods::policy;
use crate::mods::secrets::{self, SlotStatus};
use crate::mods::store;

/// The largest icon PM will show (also `icon::ICON_CAP`): a small raster, not a photo.
pub const ICON_CAP: u64 = 256 << 10;

/// The file extensions an icon may have for the Mods tab: raster only. An SVG is a document that
/// can carry script and links, and the manifest allows it only to be shown through `<img>`; the tab
/// has no use for the risk.
pub const ICON_EXTENSIONS: [&str; 2] = ["png", "webp"];

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModDto {
    pub id: String,
    /// True for the one entry that is no row: a pilot PM is still installing for this store (a
    /// marker, no `mods` row; `autoinstall`). It has a tab and the "Installing" words and nothing
    /// else: no permissions, no files, nothing to uninstall.
    pub pending: bool,
    pub name: String,
    pub description: String,
    /// `first`, `second` or `third`: what the row says. Which tier PM *treats* it as is
    /// [`Self::signed`] first: an unsigned row is third party whatever its manifest claimed.
    pub tier: String,
    /// The publisher the pin speaks of.
    pub author: String,
    /// The people the manifest credits, in its order; empty when it names none. Display only.
    pub authors: Vec<String>,
    /// The source repo, `owner/repo`.
    pub repo: String,
    pub commit: String,
    pub version: String,
    pub installed_from: String,
    pub signed: bool,
    pub consented: bool,
    pub state: Standing,
    pub permissions: Vec<PermissionDto>,
    pub internet_hosts: Vec<String>,
    /// The key slots the manifest declares, each with its host, its header and whether a key is
    /// stored for it. Never the key.
    pub secrets: Vec<SlotStatus>,
    /// The monthly AI limit in dollars, or none until the person sets one (a paid call asks first).
    pub ai_cap_usd: Option<f64>,
    /// What counts against the limit now: this month's settled spend plus the bookings still held
    /// (a call in flight, or one a crash left, which counts to month end). The cap's own ledger,
    /// not the Usage and cost table's best-effort log.
    pub ai_spent_usd: f64,
    /// The month that number is for (`YYYY-MM`, the person's own time zone).
    pub ai_month: String,
    /// The sidebar tab's label, when the mod has one.
    pub nav_tab: Option<String>,
    pub calendar_overlay: Option<String>,
    /// The mod's own help (`contributions.help`), for PM's "About this mod" panel. Author-written
    /// text: the page draws the title as a plain line and the body only through its sanitizing
    /// Markdown component, never as HTML.
    pub help: Vec<HelpDto>,
    /// Whether the manifest names a PNG or WebP icon the file list holds, within the size cap;
    /// `mod_icon` answers the bytes.
    pub has_icon: bool,
    /// A newer version waiting for the user, and why.
    pub update: Option<UpdateDto>,
    /// The safety list turned this mod back on, for "X is back on".
    pub resumed: Option<ResumedDto>,
    pub installed_at: String,
    pub updated_at: String,
}

/// One help entry of a mod, as its manifest wrote it (already held to PM's plain-text rules at install).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelpDto {
    pub id: String,
    pub title: String,
    /// Markdown source; the page renders it through `markdown.tsx` only.
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionDto {
    pub id: String,
    /// The manifest's reason, shown at install (author-written plain text).
    pub why: String,
    /// `local`, `read`, `write`, `money` or `egress` (spec §1.5).
    pub class: &'static str,
    /// When PM asks for it for this mod's tier: `auto`, `install`, `install-then-first-use`,
    /// `first-use` or `ask`.
    pub when: &'static str,
    /// The stored decision: `granted`, `denied`, or none yet.
    pub decision: Option<&'static str>,
    /// What the dispatcher answers now: `allowed`, `asks` (a prompt at first use) or `off`.
    pub access: &'static str,
    /// Why it is `off`, in words, when it is.
    pub off_because: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDto {
    pub version: String,
    /// `approval` (it asks for more than was granted) or `newer-pm`.
    pub kind: &'static str,
    /// What it asks for beyond what was granted, as the policy names them (`permission:ai`,
    /// `host:example.com`).
    pub widenings: Vec<String>,
    /// The API (`M.m`) a `newer-pm` update needs at least.
    pub needs_api: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumedDto {
    pub version: String,
    pub at: String,
    pub text: String,
}

/// What a row holds, as the listing reads it from `mods`.
#[derive(Debug, Clone)]
pub struct RawRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub tier: String,
    pub author: String,
    pub source_repo: String,
    pub source_commit: String,
    pub signed: bool,
    pub installed_from: String,
    pub package_sha256: Option<String>,
    pub manifest: String,
    pub files: String,
    pub state: String,
    pub state_reason: Option<String>,
    pub consented: bool,
    pub installed_at: String,
    pub updated_at: String,
}

/// Everything the listing needs beyond the rows.
pub struct Ctx<'a> {
    pub index: Option<&'a Index>,
    pub engine_sealed: bool,
}

fn read_rows(conn: &Connection) -> rusqlite::Result<Vec<RawRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, version, tier, author, source_repo, source_commit, signed, \
                installed_from, package_sha256, manifest, files, state, state_reason, \
                consented_at IS NOT NULL, installed_at, updated_at \
         FROM mods ORDER BY name COLLATE NOCASE, id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(RawRow {
            id: r.get(0)?,
            name: r.get(1)?,
            version: r.get(2)?,
            tier: r.get(3)?,
            author: r.get(4)?,
            source_repo: r.get(5)?,
            source_commit: r.get(6)?,
            signed: r.get(7)?,
            installed_from: r.get(8)?,
            package_sha256: r.get(9)?,
            manifest: r.get(10)?,
            files: r.get(11)?,
            state: r.get(12)?,
            state_reason: r.get(13)?,
            consented: r.get(14)?,
            installed_at: r.get(15)?,
            updated_at: r.get(16)?,
        })
    })?;
    rows.collect()
}

/// The rows and the two notes the safety list keeps, for the strips (`status::notices`).
#[allow(clippy::type_complexity)]
pub fn read_all(
    conn: &Connection,
) -> rusqlite::Result<(Vec<RawRow>, BTreeMap<String, Waiting>, Vec<Resumed>)> {
    Ok((
        read_rows(conn)?,
        ledger::read_waiting(conn)?,
        ledger::read_resumed(conn)?,
    ))
}

/// Every installed mod, as the tab lists them. One read of the store: the rows, the stored
/// decisions, the waiting updates and the resumed record. The DB lock is held by the caller for
/// this call alone; nothing here touches a file.
pub fn list(conn: &Connection, ctx: &Ctx<'_>) -> rusqlite::Result<Vec<ModDto>> {
    let rows = read_rows(conn)?;
    let decisions = read_decisions(conn)?;
    let waiting = ledger::read_waiting(conn)?;
    let resumed = ledger::read_resumed(conn)?;
    // D3 judges the whole unsigned set together, so its holdings are read once, and only when an
    // unsigned mod is there to be judged.
    let held = if rows
        .iter()
        .any(|r| PolicyTier::of(r.signed, &r.tier) == PolicyTier::Unsigned)
    {
        grants::holdings(conn, PolicyTier::Unsigned)?
    } else {
        Vec::new()
    };
    // Which slots have a key: names, hosts and headers only, never the value column.
    let bound = secrets::bound_pairs(conn)?;
    // The limits and the month's use, by the cap's own rule for the month (`mods::ai::month_use`).
    let month = money::current_month(conn, Utc::now());
    let ledgers = money::read_ledgers(conn, &month)?;
    let mut out: Vec<ModDto> = rows
        .iter()
        .map(|row| {
            let own = decisions.get(&row.id);
            dto(
                row,
                own,
                waiting.get(&row.id),
                resumed.iter().rev().find(|r| r.id == row.id),
                &Shared {
                    held: &held,
                    bound: &bound,
                    ledgers: &ledgers,
                    month: &month,
                },
                ctx,
            )
        })
        .collect();
    // A pilot whose marker is set and whose row isn't there yet. Never a row: a fake one in a
    // truth-class table would carry an invented hash and version into backups and shared vaults.
    for pilot in autoinstall::pending(conn)? {
        if !rows.iter().any(|r| r.id == pilot.id) {
            out.push(pending_dto(pilot));
        }
    }
    Ok(out)
}

/// The entry for a pilot that is still installing: its tab and the "Installing" words.
fn pending_dto(pilot: &Pilot) -> ModDto {
    let state = words::standing(
        &RowFacts {
            id: pilot.id,
            name: pilot.name,
            version: "",
            package_sha256: None,
            state: store::STATE_INSTALLING,
            reason: None,
            consented: false,
            installed_from: store::FROM_CATALOGUE,
            api: None,
        },
        None,
    );
    ModDto {
        id: pilot.id.to_owned(),
        pending: true,
        name: pilot.name.to_owned(),
        description: String::new(),
        tier: "first".to_owned(),
        author: String::new(),
        authors: Vec::new(),
        repo: String::new(),
        commit: String::new(),
        version: String::new(),
        installed_from: store::FROM_CATALOGUE.to_owned(),
        signed: true,
        consented: false,
        state,
        permissions: Vec::new(),
        internet_hosts: Vec::new(),
        secrets: Vec::new(),
        ai_cap_usd: None,
        ai_spent_usd: 0.0,
        ai_month: String::new(),
        nav_tab: Some(pilot.tab_label.to_owned()),
        calendar_overlay: None,
        help: Vec::new(),
        has_icon: false,
        update: None,
        resumed: None,
        installed_at: String::new(),
        updated_at: String::new(),
    }
}

fn read_decisions(
    conn: &Connection,
) -> rusqlite::Result<BTreeMap<String, BTreeMap<String, Decision>>> {
    let mut stmt = conn.prepare("SELECT mod_id, permission, decision FROM mod_grants")?;
    let mut out: BTreeMap<String, BTreeMap<String, Decision>> = BTreeMap::new();
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (id, permission, decision) = row?;
        let decision = match decision.as_str() {
            store::GRANTED => Decision::Granted,
            store::DENIED => Decision::Denied,
            _ => continue,
        };
        out.entry(id).or_default().insert(permission, decision);
    }
    Ok(out)
}

/// What the stored manifest says, tolerantly read.
#[derive(Debug, Default)]
pub struct ManifestView {
    pub description: String,
    pub authors: Vec<String>,
    pub icon: Option<String>,
    pub permissions: Vec<(String, String)>,
    pub internet_hosts: Vec<String>,
    pub nav_tab: Option<String>,
    pub calendar_overlay: Option<String>,
    pub help: Vec<HelpDto>,
    pub api: Option<ApiRange>,
}

/// The install check's own limits on help (`package.rs`: 64 entries, an 80-character title, a
/// 2,000-character body). A stored row that was hand-edited past them reads as having less, so the
/// listing can't be made to carry a megabyte of "help" to the page.
const HELP_ENTRIES_MAX: usize = 64;
const HELP_TITLE_MAX: usize = 80;
const HELP_BODY_MAX: usize = 2000;
/// The same for `authors` (`package.rs`: 8 names of 64 characters).
const AUTHORS_MAX: usize = 8;
const AUTHOR_MAX: usize = 64;

/// Read a stored manifest (pure). Anything missing or oddly shaped reads as absent: the verifier
/// checked the manifest at install, so this only meets a hand-edited or damaged row.
pub fn read_manifest(manifest: &str) -> ManifestView {
    let Ok(v) = serde_json::from_str::<Value>(manifest) else {
        return ManifestView::default();
    };
    let text = |v: &Value| v.as_str().map(str::to_owned);
    let list = |v: &Value| v.as_array().cloned().unwrap_or_default();
    let api = v["apiVersion"]["min"]
        .as_str()
        .and_then(policy::parse_major_minor)
        .zip(
            v["apiVersion"]["target"]
                .as_str()
                .and_then(policy::parse_major_minor),
        )
        .map(|(min, target)| ApiRange { min, target });
    ManifestView {
        description: text(&v["description"]).unwrap_or_default(),
        authors: list(&v["authors"])
            .iter()
            .filter_map(text)
            .filter(|a| a.chars().count() <= AUTHOR_MAX)
            .take(AUTHORS_MAX)
            .collect(),
        icon: text(&v["icon"]),
        permissions: list(&v["permissions"])
            .iter()
            .filter_map(|p| Some((text(&p["id"])?, text(&p["why"]).unwrap_or_default())))
            .collect(),
        internet_hosts: list(&v["internetHosts"]).iter().filter_map(text).collect(),
        nav_tab: text(&v["contributions"]["nav.tab"]["label"]),
        calendar_overlay: text(&v["contributions"]["calendar.overlay"]["label"]),
        help: list(&v["contributions"]["help"])
            .iter()
            .filter_map(|h| {
                let (id, title, body) = (text(&h["id"])?, text(&h["title"])?, text(&h["body"])?);
                let fits = title.chars().count() <= HELP_TITLE_MAX
                    && body.chars().count() <= HELP_BODY_MAX;
                fits.then_some(HelpDto { id, title, body })
            })
            .take(HELP_ENTRIES_MAX)
            .collect(),
        api,
    }
}

/// Whether the manifest's `icon` is one the tab may show (pure): a PNG or WebP path that the row's
/// file list holds, no larger than [`ICON_CAP`]. The same rule `icon::read` applies before it
/// opens anything, so the flag and the command can't disagree.
pub fn icon_path(icon: Option<&str>, files_json: &str) -> Option<String> {
    let icon = icon?;
    let ext = icon.rsplit_once('.')?.1.to_ascii_lowercase();
    if !ICON_EXTENSIONS.contains(&ext.as_str()) {
        return None;
    }
    let files: Vec<Value> = serde_json::from_str(files_json).ok()?;
    let listed = files.iter().find(|f| f["path"].as_str() == Some(icon))?;
    (listed["size"].as_u64()? <= ICON_CAP).then(|| icon.to_owned())
}

pub(super) fn class_word(c: Class) -> &'static str {
    match c {
        Class::Local => "local",
        Class::Read => "read",
        Class::Write => "write",
        Class::Money => "money",
        Class::Egress => "egress",
    }
}

fn when_word(w: When) -> &'static str {
    match w {
        When::Auto => "auto",
        When::Install => "install",
        When::InstallThenFirstUse => "install-then-first-use",
        When::FirstUse => "first-use",
        When::Ask => "ask",
    }
}

fn decision_word(d: Decision) -> &'static str {
    d.stored()
}

/// The words for a permission that is off, by the rule that made it so.
pub fn off_words(why: Refused) -> &'static str {
    match why {
        Refused::Undeclared => "This mod didn't ask for it.",
        Refused::Denied => "You turned this off.",
        Refused::NotConsented => "It wasn't allowed when this mod was installed. Turn it on to allow it.",
        Refused::UnsealedEngine => {
            "Unsigned mods can't read your data until PM's web engine is sealed."
        }
        Refused::Exclusion => {
            "Unsigned mods may read your data or reach the internet, never both, and an unsigned mod \
             already holds the other."
        }
    }
}

/// One declared permission as the tab shows it (pure over the facts).
pub fn permission(
    id: &str,
    why: &str,
    tier: PolicyTier,
    decision: Option<Decision>,
    held: &[(PolicyTier, Class)],
    engine_sealed: bool,
) -> Option<PermissionDto> {
    let policy = permissions::policy(id)?;
    let outcome = grants::decide(
        policy,
        &Facts {
            tier,
            declared: true,
            decision,
            held,
            engine_sealed,
        },
    );
    let (access, off_because) = match outcome {
        Outcome::Granted => ("allowed", None),
        Outcome::NeedsPrompt => ("asks", None),
        Outcome::Refused(why) => ("off", Some(off_words(why).to_owned())),
    };
    Some(PermissionDto {
        id: policy.id.to_owned(),
        why: why.to_owned(),
        class: class_word(policy.class),
        when: when_word(policy.when(tier)),
        decision: decision.map(decision_word),
        access,
        off_because,
    })
}

/// What every row's DTO is built against, read once for the whole list.
struct Shared<'a> {
    /// D3's holdings of the unsigned set.
    held: &'a [(PolicyTier, Class)],
    /// The key slots that have a key (names and bindings, never a value).
    bound: &'a BTreeSet<(String, String, String, String)>,
    /// Each mod's monthly AI limit and what counts against it now.
    ledgers: &'a BTreeMap<String, (Option<f64>, MonthUse)>,
    /// The month today falls in, for a mod with no ledger row.
    month: &'a str,
}

fn dto(
    row: &RawRow,
    decisions: Option<&BTreeMap<String, Decision>>,
    waiting: Option<&Waiting>,
    resumed: Option<&Resumed>,
    shared: &Shared<'_>,
    ctx: &Ctx<'_>,
) -> ModDto {
    let Shared {
        held,
        bound,
        ledgers,
        month,
    } = shared;
    let ledger = ledgers.get(&row.id);
    let m = read_manifest(&row.manifest);
    let tier = PolicyTier::of(row.signed, &row.tier);
    // Only the unsigned set is judged together; a signed tier has no exclusion to read.
    let held_here: &[(PolicyTier, Class)] = if tier == PolicyTier::Unsigned {
        held
    } else {
        &[]
    };
    let permissions = m
        .permissions
        .iter()
        .filter_map(|(id, why)| {
            permission(
                id,
                why,
                tier,
                decisions.and_then(|d| d.get(id)).copied(),
                held_here,
                ctx.engine_sealed,
            )
        })
        .collect();
    let state = words::standing(
        &RowFacts {
            id: &row.id,
            name: &row.name,
            version: &row.version,
            package_sha256: row.package_sha256.as_deref(),
            state: &row.state,
            reason: row.state_reason.as_deref(),
            consented: row.consented,
            installed_from: &row.installed_from,
            api: m.api,
        },
        ctx.index,
    );
    ModDto {
        id: row.id.clone(),
        pending: false,
        name: row.name.clone(),
        description: m.description.clone(),
        tier: row.tier.clone(),
        author: row.author.clone(),
        authors: m.authors.clone(),
        repo: row.source_repo.clone(),
        commit: row.source_commit.clone(),
        version: row.version.clone(),
        installed_from: row.installed_from.clone(),
        signed: row.signed,
        consented: row.consented,
        state,
        permissions,
        internet_hosts: m.internet_hosts.clone(),
        secrets: secrets::statuses(&row.id, &row.manifest, bound),
        ai_cap_usd: ledger.and_then(|(cap, _)| *cap),
        ai_spent_usd: ledger.map_or(0.0, |(_, used)| used.used_usd),
        ai_month: ledger.map_or_else(|| (*month).to_owned(), |(_, used)| used.month.clone()),
        nav_tab: m.nav_tab.clone(),
        calendar_overlay: m.calendar_overlay.clone(),
        help: m.help.clone(),
        has_icon: icon_path(m.icon.as_deref(), &row.files).is_some(),
        update: waiting.and_then(|w| update_dto(&row.version, w)),
        resumed: resumed.map(|r| ResumedDto {
            version: r.version.clone(),
            at: r.at.clone(),
            text: format!("{} is back on, fixed in version {}.", row.name, r.version),
        }),
        installed_at: row.installed_at.clone(),
        updated_at: row.updated_at.clone(),
    }
}

/// A waiting update as the tab shows it, unless the installed version has caught up with it (a
/// stale note: the install that satisfied it clears it, but a version that doesn't parse is shown
/// rather than hidden).
pub fn update_dto(installed: &str, w: &Waiting) -> Option<UpdateDto> {
    if let (Ok(have), Ok(want)) = (
        semver::Version::parse(installed),
        semver::Version::parse(&w.version),
    ) {
        if have >= want {
            return None;
        }
    }
    let (kind, text) = match w.kind {
        WaitingKind::Approval => (
            "approval",
            format!(
                "Version {} asks for more than you have allowed, so it waits for you.",
                w.version
            ),
        ),
        WaitingKind::NewerPm => (
            "newer-pm",
            format!(
                "Version {} needs a newer PM. Update PM to get it.",
                w.version
            ),
        ),
    };
    Some(UpdateDto {
        version: w.version.clone(),
        kind,
        widenings: w.widenings.clone(),
        needs_api: w.needs_api.clone(),
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The ids in `dtos`, as a set.
    fn ids(dtos: &[ModDto]) -> BTreeSet<&str> {
        dtos.iter().map(|d| d.id.as_str()).collect()
    }

    use crate::mods::tab::testing::{insert, manifest, store as test_store, Row};
    use serde_json::json;

    #[test]
    fn the_stored_manifest_is_read_tolerantly() {
        let m = read_manifest(
            &json!({
                "description": "Shows your documents.",
                "icon": "app/icon.png",
                "permissions": [{"id": "documents.meta", "why": "to draw them"}, {"nope": 1}],
                "internetHosts": ["example.com", 7],
                "contributions": {"nav.tab": {"label": "Map"}, "calendar.overlay": {"label": "Dates"}},
                "apiVersion": {"min": "1.0", "target": "1.0"},
            })
            .to_string(),
        );
        assert_eq!(m.description, "Shows your documents.");
        assert_eq!(
            m.permissions,
            [("documents.meta".into(), "to draw them".into())]
        );
        assert_eq!(m.internet_hosts, ["example.com"]);
        assert_eq!(m.nav_tab.as_deref(), Some("Map"));
        assert_eq!(m.calendar_overlay.as_deref(), Some("Dates"));
        assert_eq!(
            m.api,
            Some(ApiRange {
                min: (1, 0),
                target: (1, 0)
            })
        );
        // Garbage reads as nothing, never a panic.
        for junk in ["", "{", "[]", "null", "\"x\"", "{\"permissions\": 3}"] {
            let m = read_manifest(junk);
            assert!(m.permissions.is_empty() && m.nav_tab.is_none(), "{junk}");
        }
    }

    #[test]
    fn an_icon_is_a_listed_png_or_webp_within_the_cap_and_nothing_else() {
        let files = |size: u64| {
            json!([{"path": "app/icon.png", "sha256": "0", "size": size},
                   {"path": "app/icon.svg", "sha256": "0", "size": 10},
                   {"path": "app/icon.WEBP", "sha256": "0", "size": 10},
                   {"path": "app/icon.jpg", "sha256": "0", "size": 10},
                   {"path": "app/noext", "sha256": "0", "size": 10}])
            .to_string()
        };
        assert!(icon_path(Some("app/icon.png"), &files(10)).is_some());
        assert!(icon_path(Some("app/icon.png"), &files(ICON_CAP)).is_some());
        assert!(icon_path(Some("app/icon.png"), &files(ICON_CAP + 1)).is_none());
        assert!(icon_path(Some("app/icon.WEBP"), &files(10)).is_some());
        for refused in [
            "app/icon.svg",
            "app/icon.jpg",
            "app/noext",
            "app/missing.png",
            "",
        ] {
            assert!(icon_path(Some(refused), &files(10)).is_none(), "{refused}");
        }
        assert!(icon_path(None, &files(10)).is_none());
        assert!(icon_path(Some("app/icon.png"), "not json").is_none());
        // A size that isn't a number can't be checked against the cap.
        let odd = json!([{"path": "app/icon.png", "sha256": "0", "size": "big"}]).to_string();
        assert!(icon_path(Some("app/icon.png"), &odd).is_none());
    }

    #[test]
    fn a_waiting_update_says_why_and_a_caught_up_one_is_hidden() {
        let w = |kind, v: &str| Waiting {
            version: v.into(),
            sha256: "0".repeat(64),
            kind,
            widenings: vec!["permission:ai".into()],
            needs_api: Some("1.2".into()),
        };
        let u = update_dto("1.0.0", &w(WaitingKind::Approval, "1.1.0")).unwrap();
        assert_eq!(
            (u.kind, u.widenings.as_slice()),
            ("approval", &["permission:ai".to_owned()][..])
        );
        assert!(u.text.contains("1.1.0") && u.text.contains("waits for you"));
        let u = update_dto("1.0.0", &w(WaitingKind::NewerPm, "1.1.0")).unwrap();
        assert_eq!((u.kind, u.needs_api.as_deref()), ("newer-pm", Some("1.2")));
        assert!(u.text.contains("Update PM"));
        assert!(update_dto("1.1.0", &w(WaitingKind::Approval, "1.1.0")).is_none());
        assert!(update_dto("2.0.0", &w(WaitingKind::Approval, "1.1.0")).is_none());
        // A version that doesn't compare is shown, not hidden.
        assert!(update_dto("weird", &w(WaitingKind::Approval, "1.1.0")).is_some());
    }

    #[test]
    fn a_permission_is_shown_as_the_dispatcher_would_answer_it() {
        let held_read = [(PolicyTier::Unsigned, Class::Read)];
        let show = |id, tier, decision, held: &[(PolicyTier, Class)], sealed| {
            permission(id, "why", tier, decision, held, sealed).unwrap()
        };
        // First party, no row: a read waits for its install-time grant, so it is off until given.
        let p = show("documents.meta", PolicyTier::First, None, &[], true);
        assert_eq!((p.access, p.class, p.when), ("off", "read", "install"));
        assert!(p.off_because.unwrap().contains("installed"));
        let p = show(
            "documents.meta",
            PolicyTier::First,
            Some(Decision::Granted),
            &[],
            true,
        );
        assert_eq!((p.access, p.decision), ("allowed", Some("granted")));
        let p = show(
            "documents.meta",
            PolicyTier::First,
            Some(Decision::Denied),
            &[],
            true,
        );
        assert_eq!(
            (p.access, p.off_because.as_deref()),
            ("off", Some("You turned this off."))
        );
        // Storage is automatic.
        assert_eq!(
            show("storage", PolicyTier::Second, None, &[], true).access,
            "allowed"
        );
        // A first-use permission asks.
        let p = show("ai", PolicyTier::Second, None, &[], true);
        assert_eq!((p.access, p.when, p.class), ("asks", "first-use", "money"));
        // A first-party mod's `ai` is allowed from the start; what asks is the limit, at the first
        // paid use (the handler's rule, not the table's).
        let p = show("ai", PolicyTier::First, None, &[], true);
        assert_eq!((p.access, p.when, p.class), ("allowed", "auto", "money"));
        // A no on record still refuses a first-party mod.
        let p = show("ai", PolicyTier::First, Some(Decision::Denied), &[], true);
        assert_eq!(p.access, "off");
        // D3: an unsigned mod's internet is off while another unsigned mod holds a read.
        let p = show(
            "internet",
            PolicyTier::Unsigned,
            Some(Decision::Granted),
            &held_read,
            true,
        );
        assert_eq!(p.access, "off");
        assert!(p.off_because.unwrap().contains("never both"));
        // And an unsigned read is off while the engine is unsealed, whatever was granted.
        let p = show(
            "documents.meta",
            PolicyTier::Unsigned,
            Some(Decision::Granted),
            &[],
            false,
        );
        assert_eq!(p.access, "off");
        assert!(p.off_because.unwrap().contains("sealed"));
        // An id the policy table lacks is left out, never invented.
        assert!(permission("made.up", "why", PolicyTier::First, None, &[], true).is_none());
    }

    const CTX: Ctx<'static> = Ctx {
        index: None,
        engine_sealed: true,
    };

    #[test]
    fn a_mods_help_is_read_in_order_and_whatever_is_malformed_or_oversized_is_left_out() {
        let m = read_manifest(
            &json!({"contributions": {"help": [
                {"id": "a", "title": "First", "body": "One.\nTwo."},
                {"id": "b", "title": 7, "body": "no title"},
                {"id": "c", "title": "No body"},
                "not an object",
                {"id": "d", "title": "x".repeat(HELP_TITLE_MAX + 1), "body": "long title"},
                {"id": "e", "title": "Long body", "body": "x".repeat(HELP_BODY_MAX + 1)},
                {"id": "f", "title": "Edge", "body": "x".repeat(HELP_BODY_MAX)},
            ]}})
            .to_string(),
        );
        let ids: Vec<&str> = m.help.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["a", "f"]);
        assert_eq!(m.help[0].body, "One.\nTwo.");
        // No help, or help that isn't a list, reads as none.
        for junk in [
            json!({}),
            json!({"contributions": {}}),
            json!({"contributions": {"help": "text"}}),
        ] {
            assert!(read_manifest(&junk.to_string()).help.is_empty(), "{junk}");
        }
        // The count is bounded like the install check's.
        let many: Vec<Value> = (0..HELP_ENTRIES_MAX + 5)
            .map(|i| json!({"id": format!("h{i}"), "title": "T", "body": "B"}))
            .collect();
        let m = read_manifest(&json!({"contributions": {"help": many}}).to_string());
        assert_eq!(m.help.len(), HELP_ENTRIES_MAX);
    }

    #[test]
    fn the_listing_carries_a_mods_help_to_the_page_in_camel_case() {
        let (_dir, c) = test_store();
        let mut manifest = manifest(&["storage"]);
        manifest["contributions"]["help"] =
            json!([{"id": "map-overview", "title": "Map", "body": "Hover a document."}]);
        insert(
            &c,
            &Row {
                manifest,
                ..Row::default()
            },
        );
        let listed = list(&c, &CTX).unwrap();
        let wire = serde_json::to_value(&listed[0]).unwrap();
        assert_eq!(
            wire["help"],
            json!([{"id": "map-overview", "title": "Map", "body": "Hover a document."}])
        );
        // A mod with none lists an empty array, never a missing field the page would have to guess at.
        let (_dir, c) = test_store();
        insert(&c, &Row::default());
        let wire = serde_json::to_value(&list(&c, &CTX).unwrap()[0]).unwrap();
        assert_eq!(wire["help"], json!([]));
    }

    #[test]
    fn the_listing_carries_the_credited_authors_beside_the_publisher() {
        let (_dir, c) = test_store();
        let mut manifest = manifest(&["storage"]);
        manifest["authors"] = json!(["Bobby Yu", "Ann Lee"]);
        insert(
            &c,
            &Row {
                manifest,
                ..Row::default()
            },
        );
        let wire = serde_json::to_value(&list(&c, &CTX).unwrap()[0]).unwrap();
        assert_eq!(wire["authors"], json!(["Bobby Yu", "Ann Lee"]));
        assert_eq!(wire["author"], "Atlas", "the publisher stays the pin's");
        // None named lists an empty array, and a damaged or oversized value reads as less.
        let (_dir, c) = test_store();
        insert(&c, &Row::default());
        let wire = serde_json::to_value(&list(&c, &CTX).unwrap()[0]).unwrap();
        assert_eq!(wire["authors"], json!([]));
        let long = "x".repeat(AUTHOR_MAX + 1);
        let names: Vec<Value> = (0..AUTHORS_MAX + 3)
            .map(|i| json!(format!("N{i}")))
            .collect();
        let m = read_manifest(&json!({"authors": names}).to_string());
        assert_eq!(m.authors.len(), AUTHORS_MAX);
        let m = read_manifest(&json!({"authors": ["Ok", long, 7, null]}).to_string());
        assert_eq!(m.authors, ["Ok"]);
        assert!(read_manifest(&json!({"authors": "Bobby"}).to_string())
            .authors
            .is_empty());
    }

    #[test]
    fn the_committed_map_manifests_help_reaches_the_listing_whole() {
        // B6's eight entries, through the same reader the listing uses: none is dropped by the
        // length limits or by the shape check.
        let text = include_str!("../../../../mods/atlas.map/manifest.json");
        let m = read_manifest(text);
        let ids: Vec<&str> = m.help.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "map-overview",
                "map-canvas",
                "map-arrangement",
                "map-cohesion",
                "map-navigate",
                "map-names",
                "map-settings",
                "map-updates"
            ]
        );
        assert!(m
            .help
            .iter()
            .all(|h| !h.title.is_empty() && !h.body.is_empty()));
        // The mod is the Document Map (not the geographical kind), and it credits its maker.
        assert_eq!(m.help[0].title, "Document Map");
        assert_eq!(m.authors, ["Bobby Yu"]);
    }

    #[test]
    fn the_listing_reads_rows_decisions_and_notes_from_one_store() {
        let (_dir, c) = test_store();
        insert(&c, &Row::default());
        c.execute(
            "INSERT INTO mod_grants(mod_id, permission, decision) \
             VALUES ('atlas.map', 'documents.meta', 'granted')",
            [],
        )
        .unwrap();
        // A waiting update and a resumed record for it, as slice 5 writes them.
        ledger::set_waiting(
            &c,
            "atlas.map",
            Waiting {
                version: "1.1.0".into(),
                sha256: "0".repeat(64),
                kind: WaitingKind::Approval,
                widenings: vec!["permission:ai".into()],
                needs_api: None,
            },
        )
        .unwrap();
        c.execute(
            "INSERT INTO settings(key, value) VALUES ('mods_resumed', ?1)",
            [
                json!([{"id": "atlas.map", "version": "1.0.0", "at": "2026-11-11T00:00:00.000Z"}])
                    .to_string(),
            ],
        )
        .unwrap();

        let listed = list(&c, &CTX).unwrap();
        assert_eq!(ids(&listed), BTreeSet::from(["atlas.map"]));
        let map = &listed[0];
        assert_eq!(
            (
                map.name.as_str(),
                map.tier.as_str(),
                map.version.as_str(),
                map.signed,
                map.consented
            ),
            ("Map", "first", "1.0.0", true, true)
        );
        assert_eq!(map.description, "Shows your documents.");
        assert_eq!(map.nav_tab.as_deref(), Some("Map"));
        assert!(map.has_icon);
        assert_eq!(map.state.kind, words::Kind::Running);
        let access: BTreeMap<&str, (&str, &str)> = map
            .permissions
            .iter()
            .map(|p| (p.id.as_str(), (p.access, p.why.as_str())))
            .collect();
        assert_eq!(access["documents.meta"], ("allowed", "for documents.meta"));
        assert_eq!(access["storage"], ("allowed", "for storage"));
        assert_eq!(map.update.as_ref().unwrap().kind, "approval");
        let resumed = map.resumed.as_ref().unwrap();
        assert_eq!(resumed.version, "1.0.0");
        assert_eq!(resumed.text, "Map is back on, fixed in version 1.0.0.");
    }

    #[test]
    fn a_pilot_still_installing_is_one_synthesized_entry_and_never_a_row() {
        let (_dir, c) = test_store();
        // No marker: no entry, and the wire carries `pending: false` for a real row.
        assert!(list(&c, &CTX).unwrap().is_empty());
        insert(&c, &Row::default());
        let wire = serde_json::to_value(&list(&c, &CTX).unwrap()[0]).unwrap();
        assert_eq!(wire["pending"], json!(false));
        c.execute("DELETE FROM mods", []).unwrap();

        c.execute(
            "INSERT INTO settings(key, value) VALUES ('mods_autoinstall_map', '1')",
            [],
        )
        .unwrap();
        let listed = list(&c, &CTX).unwrap();
        assert_eq!(listed.len(), 1);
        let map = &listed[0];
        assert!(map.pending);
        assert_eq!(
            (map.id.as_str(), map.name.as_str(), map.nav_tab.as_deref()),
            ("atlas.map", "Document Map", Some("Document Map"))
        );
        // The existing "Installing" words, nothing to turn on, nothing to run, no permissions.
        assert_eq!(map.state.kind, words::Kind::Installing);
        assert_eq!(map.state.fix, words::Fix::None);
        assert!(!map.state.running && !map.consented);
        assert!(
            map.state.text.starts_with("Installing."),
            "{}",
            map.state.text
        );
        assert!(map.permissions.is_empty() && map.help.is_empty() && map.update.is_none());
        // Listing writes nothing: no row, the marker stays.
        let (rows, marker): (i64, i64) = c
            .query_row(
                "SELECT (SELECT count(*) FROM mods), \
                        (SELECT count(*) FROM settings WHERE key = 'mods_autoinstall_map')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((rows, marker), (0, 1));

        // Once the mod has a row (any kind) the real row speaks for it, never both.
        insert(&c, &Row::default());
        let listed = list(&c, &CTX).unwrap();
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].pending);
        // A marker for no pilot in the table lists nothing.
        c.execute("DELETE FROM mods", []).unwrap();
        c.execute(
            "DELETE FROM settings WHERE key = 'mods_autoinstall_map'",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO settings(key, value) VALUES ('mods_autoinstall_nothing', '1')",
            [],
        )
        .unwrap();
        assert!(list(&c, &CTX).unwrap().is_empty());
    }

    #[test]
    fn a_row_whose_manifest_no_longer_reads_is_still_listed_so_it_can_be_uninstalled() {
        let (_dir, c) = test_store();
        insert(
            &c,
            &Row {
                state: "parked",
                reason: Some("record-unreadable"),
                ..Row::default()
            },
        );
        c.execute(
            "UPDATE mods SET manifest = 'not json', files = 'also not'",
            [],
        )
        .unwrap();
        let listed = list(&c, &CTX).unwrap();
        let map = &listed[0];
        assert!(map.permissions.is_empty() && !map.has_icon && map.nav_tab.is_none());
        assert_eq!(map.state.fix, words::Fix::Uninstall);
    }

    /// The row's key slots, each with whether a key is stored AND still bound to the slot: a
    /// moved or planted row reads as not set, and nothing in the DTO can carry the value.
    #[test]
    fn a_listed_mod_shows_its_key_slots_set_or_not_and_never_a_value() {
        let (_dir, c) = test_store();
        let mut m = manifest(&["internet"]);
        m["internetHosts"] = json!(["a.example.com"]);
        m["secrets"] = json!([
            {"name": "set", "host": "a.example.com", "header": "X-A"},
            {"name": "unset", "host": "a.example.com", "header": "X-B"},
            {"name": "moved", "host": "a.example.com", "header": "X-C"},
        ]);
        insert(
            &c,
            &Row {
                manifest: m,
                ..Row::default()
            },
        );
        c.execute(
            "INSERT INTO mod_secrets(mod_id, name, host, header, value) VALUES \
             ('atlas.map', 'set', 'a.example.com', 'X-A', 'sentinel-value-1'), \
             ('atlas.map', 'moved', 'old.example.com', 'X-C', 'sentinel-value-2')",
            [],
        )
        .unwrap();
        let listed = list(&c, &CTX).unwrap();
        let got: Vec<(&str, bool)> = listed[0]
            .secrets
            .iter()
            .map(|s| (s.name.as_str(), s.set))
            .collect();
        assert_eq!(got, [("set", true), ("unset", false), ("moved", false)]);
        assert_eq!(listed[0].secrets[0].host, "a.example.com");
        assert_eq!(listed[0].secrets[0].header, "X-A");
        let wire = serde_json::to_string(&listed).unwrap();
        assert!(!wire.contains("sentinel-value"), "{wire}");
        // A mod that declares none lists none.
        let (_d2, c2) = test_store();
        insert(&c2, &Row::default());
        assert!(list(&c2, &CTX).unwrap()[0].secrets.is_empty());
    }

    /// The row's monthly AI limit and what counts against it: this month's settled spend plus the
    /// bookings held, per mod; an earlier month reads as nothing used (the next call rolls it).
    #[test]
    fn a_listed_mod_shows_its_ai_limit_and_what_it_has_used_this_month() {
        let (_dir, c) = test_store();
        insert(&c, &Row::default());
        let listed = list(&c, &CTX).unwrap();
        assert_eq!(listed[0].ai_cap_usd, None, "no limit until one is set");
        assert_eq!(listed[0].ai_spent_usd, 0.0);
        let now = money::current_month(&c, Utc::now());
        assert_eq!(listed[0].ai_month, now);
        c.execute(
            "UPDATE mods SET ai_cap_usd = 2.0, ai_spent_usd = 0.5, ai_month = ?1",
            [&now],
        )
        .unwrap();
        c.execute(
            "INSERT INTO ai_reservations(mod_id, reserved_usd) VALUES ('atlas.map', 0.25)",
            [],
        )
        .unwrap();
        let listed = list(&c, &CTX).unwrap();
        assert_eq!(
            (listed[0].ai_cap_usd, listed[0].ai_spent_usd),
            (Some(2.0), 0.75)
        );
        let wire = serde_json::to_value(&listed[0]).unwrap();
        assert_eq!(wire["aiCapUsd"], json!(2.0));
        assert_eq!(wire["aiSpentUsd"], json!(0.75));
        assert_eq!(wire["aiMonth"], json!(now));
        // Last month's spend has rolled.
        c.execute("UPDATE mods SET ai_month = '2000-01'", [])
            .unwrap();
        let listed = list(&c, &CTX).unwrap();
        assert_eq!(
            (listed[0].ai_cap_usd, listed[0].ai_spent_usd),
            (Some(2.0), 0.0)
        );
        assert_eq!(listed[0].ai_month, now);
    }

    #[test]
    fn mods_are_listed_by_name_and_the_unsigned_set_is_judged_together() {
        let (_dir, c) = test_store();
        insert(
            &c,
            &Row {
                id: "zed.reader",
                name: "alpha reader",
                signed: false,
                tier: "third",
                installed_from: "file",
                manifest: manifest(&["documents.meta"]),
                ..Row::default()
            },
        );
        insert(
            &c,
            &Row {
                id: "acme.web",
                name: "Zebra web",
                signed: false,
                tier: "third",
                installed_from: "file",
                manifest: manifest(&["internet"]),
                ..Row::default()
            },
        );
        insert(&c, &Row::default());
        c.execute(
            "INSERT INTO mod_grants(mod_id, permission, decision) VALUES \
             ('zed.reader', 'documents.meta', 'granted')",
            [],
        )
        .unwrap();
        let listed = list(&c, &CTX).unwrap();
        let order: Vec<&str> = listed.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(
            order,
            ["zed.reader", "atlas.map", "acme.web"],
            "by name, case-insensitively"
        );
        // The other unsigned mod's read makes this one's internet off, with D3's words.
        let web = listed.iter().find(|d| d.id == "acme.web").unwrap();
        assert_eq!(web.permissions[0].access, "off");
        assert!(web.permissions[0]
            .off_because
            .as_ref()
            .unwrap()
            .contains("never both"));
        // A signed mod is not judged against them.
        let map = listed.iter().find(|d| d.id == "atlas.map").unwrap();
        assert!(map
            .permissions
            .iter()
            .all(|p| p.off_because.as_deref() != Some(off_words(Refused::Exclusion))));
    }
}
