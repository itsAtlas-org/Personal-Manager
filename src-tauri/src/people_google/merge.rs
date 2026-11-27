// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The three-way merge (spec 7.3, "The three-way merge", and 7.4). **One engine**: it merges
//! three [`Card`]s (the base, PM's person now, and the other side's new copy), knows nothing of
//! Google, and is the engine a later file merge reuses.
//!
//! Pure: no DB, no network, and no clock (`now` comes in).
//!
//! - **Single values** (each name part, the birthday, notes, the star) compare as values.
//! - **Lists** (every detail kind, and the labels) pair items by (kind, `value_key`) as a multiset in
//!   order of appearance. A PM row with a non-empty `src_key` pairs by that instead, so a value PM
//!   edited still pairs with the item it came from. A pair is compared attribute by attribute (the
//!   value, the label, each key of `parts`, the dates and the current flag of a job or school, and
//!   whether it is the primary one). Position is never compared, only primary or not.
//! - **Outcomes.** The other side changed, PM didn't: [`Apply`] it to PM. PM changed, the other side
//!   didn't: [`Send`] it (the source is dirty, and the outbound path writes it). Both changed to the
//!   same thing: nothing. Both changed differently: a [`Conflict`], on the field or on the item's
//!   attribute. An item added on one side is added; removed on one side and untouched on the other is
//!   removed; removed on one side and edited on the other is a conflict. PM edited a to b (its
//!   `src_key` still names a) while Google edited a to c: one conflict on the item, offering
//!   Keep PM's, Use Google's or Keep both. A PM row with no `src_key` pairing is a removal plus an
//!   addition, so both new values are kept.
//! - **An open conflict freezes its field** (7.4). The engine applies neither side of it, leaves it
//!   out of [`Outcome::send`] (and so out of every `updatePersonFields` list), refreshes only the
//!   entry's `google` value, and carries the entry over. The snapshot it returns is a pure copy of
//!   the other side and may advance, because no base is needed while a field is frozen.
//!
//! Nothing here derives `Debug` for a type holding contact values (spec 8).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::people::card::{Card, CardDetail};
use crate::people::fold::{fold_key, value_key};

use super::map::{address_parts, address_with, MAPPED_KINDS};

/// The list kind that is the card's labels (by name).
pub const LABEL_KIND: &str = "label";

/// One side of a merge. `src_keys` is parallel to `card.details`: only PM's side has any (the
/// `person_details.src_key` of each row, empty when none); an empty slice means none.
pub struct Side<'a> {
    pub card: &'a Card,
    pub starred: bool,
    pub src_keys: &'a [String],
}

/// The values compared as a whole (spec 7.3: each name part, birthday, notes, star).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Single {
    GivenName,
    MiddleName,
    FamilyName,
    NamePrefix,
    NameSuffix,
    BornOn,
    Notes,
    Starred,
}

impl Single {
    const ALL: [Single; 8] = [
        Single::GivenName,
        Single::MiddleName,
        Single::FamilyName,
        Single::NamePrefix,
        Single::NameSuffix,
        Single::BornOn,
        Single::Notes,
        Single::Starred,
    ];

    /// The word the field has in a stored conflict.
    pub fn word(self) -> &'static str {
        match self {
            Single::GivenName => "given_name",
            Single::MiddleName => "middle_name",
            Single::FamilyName => "family_name",
            Single::NamePrefix => "name_prefix",
            Single::NameSuffix => "name_suffix",
            Single::BornOn => "birthday",
            Single::Notes => "notes",
            Single::Starred => "starred",
        }
    }

    #[allow(dead_code)] // the conflict answers (g5) read a stored field word; until then only the tests do
    pub fn from_word(word: &str) -> Option<Single> {
        Single::ALL.into_iter().find(|s| s.word() == word)
    }

    /// The Google array this field is written through (the `updatePersonFields` word).
    #[allow(dead_code)] // the outbound pass (g4) names the fields it changed; until then nothing does
    pub fn google_field(self) -> &'static str {
        match self {
            Single::GivenName
            | Single::MiddleName
            | Single::FamilyName
            | Single::NamePrefix
            | Single::NameSuffix => "names",
            Single::BornOn => "birthdays",
            Single::Notes => "biographies",
            Single::Starred => "memberships",
        }
    }

    /// The field's value on one side; `None` when empty. The star is `Some("1")` or nothing.
    pub fn get(self, card: &Card, starred: bool) -> Option<String> {
        let text = |s: &str| (!s.is_empty()).then(|| s.to_string());
        match self {
            Single::GivenName => text(&card.given_name),
            Single::MiddleName => text(&card.middle_name),
            Single::FamilyName => text(&card.family_name),
            Single::NamePrefix => text(&card.name_prefix),
            Single::NameSuffix => text(&card.name_suffix),
            Single::BornOn => card.born_on.clone(),
            Single::Notes => text(&card.notes),
            Single::Starred => starred.then(|| "1".to_string()),
        }
    }
}

/// One question the merge could not answer: both sides changed a field or an item differently.
/// JSON: `{ field, item_key?, attribute?, pm, google, seen_at }` (spec 7.4). `field` is a
/// [`Single::word`], a detail kind, or `"label"`; an item's `pm` and `google` are the item as an
/// object (or null where that side has none), an attribute's are its two texts.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Conflict {
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribute: Option<String>,
    pub pm: Value,
    pub google: Value,
    pub seen_at: String,
}

/// A change to make to PM's person.
#[derive(Clone)]
#[allow(dead_code)] // g3's sync applies these to PM's rows; until then only the tests read them
pub enum Apply {
    /// Set a single value (`None` clears it).
    Set(Single, Option<String>),
    /// Add a detail; `primary` puts it first of its kind.
    AddItem {
        detail: CardDetail,
        primary: bool,
    },
    /// Change PM's row number `row` (an index into the card's details): `to` is the row as it
    /// should read, `attrs` the attributes that changed; `primary` is the new primary flag when
    /// `attrs` has `"primary"`.
    EditItem {
        row: usize,
        attrs: Vec<String>,
        to: CardDetail,
        primary: bool,
    },
    /// Remove PM's row number `row`.
    RemoveItem {
        row: usize,
    },
    AddLabel(String),
    RemoveLabel(String),
}

/// A change PM made that the other side doesn't have yet.
#[derive(Clone)]
#[allow(dead_code)] // g4's outbound pass plans these into requests; until then only the tests read them
pub enum Send {
    Set(Single, Option<String>),
    /// A row PM added (`row` is its index in the card's details).
    AddItem {
        row: usize,
        detail: CardDetail,
        primary: bool,
    },
    /// PM changed `attrs` of a row that pairs with the other side's item `google_key`.
    EditItem {
        row: usize,
        kind: String,
        google_key: String,
        attrs: Vec<String>,
        to: CardDetail,
        primary: bool,
    },
    /// PM removed the row the other side's item `google_key` pairs with.
    RemoveItem {
        kind: String,
        google_key: String,
    },
    AddLabel(String),
    RemoveLabel(String),
}

/// What a merge decided.
#[allow(dead_code)] // g3 and g4 read every field after a merge; until then only the tests do
pub struct Outcome {
    /// Changes to make to PM.
    pub apply: Vec<Apply>,
    /// What PM changed and the other side lacks (frozen fields left out).
    pub send: Vec<Send>,
    /// Every open conflict: the ones carried over (their `google` refreshed) and the new ones.
    pub conflicts: Vec<Conflict>,
    /// PM rows (indices into the card's details) held by an open item conflict: leave their
    /// `src_key` as it is. This is not the whole reset exception: see [`Outcome::keep_src_rows`].
    pub frozen_rows: Vec<usize>,
    /// The new snapshot: the other side's copy.
    pub snapshot: Card,
    pub snapshot_starred: bool,
}

impl Outcome {
    /// The PM rows whose `src_key` must stay as it is after this merge: those held by an open item
    /// conflict, and those with a send still pending (an edit not yet written to Google, or an
    /// add). Resetting a pending edit's `src_key` to its new value would stop it pairing with the
    /// Google item it came from, and the next merge would send a removal plus an addition, which
    /// drops every attribute of the Google item PM does not model. Every other row's `src_key`
    /// becomes its current `value_key` after the agreement. Sorted, without repeats.
    #[allow(dead_code)] // the sync (g3) resets src_keys by this; until then only the tests do
    pub fn keep_src_rows(&self) -> Vec<usize> {
        let mut rows: BTreeSet<usize> = self.frozen_rows.iter().copied().collect();
        for s in &self.send {
            if let Send::EditItem { row, .. } | Send::AddItem { row, .. } = s {
                rows.insert(*row);
            }
        }
        rows.into_iter().collect()
    }

    /// The fields PM changed and the other side lacks, as [`Single::word`]s and detail kinds
    /// (`"label"` for the labels), sorted and without repeats.
    #[allow(dead_code)] // the outbound pass (g4) lists what it sends; until then only the tests do
    pub fn changed_fields(&self) -> Vec<String> {
        let mut out = BTreeSet::new();
        for s in &self.send {
            out.insert(match s {
                Send::Set(f, _) => f.word().to_string(),
                Send::AddItem { detail, .. } => detail.kind.clone(),
                Send::EditItem { kind, .. } | Send::RemoveItem { kind, .. } => kind.clone(),
                Send::AddLabel(_) | Send::RemoveLabel(_) => LABEL_KIND.to_string(),
            });
        }
        out.into_iter().collect()
    }
}

// --- items -------------------------------------------------------------------------------------

/// One list item on one side.
struct Item {
    /// Its index in the card's details (or labels).
    row: usize,
    key: String,
    d: CardDetail,
    primary: bool,
    src: String,
}

impl Item {
    /// The key it pairs with a base item by: `src_key` when it has one.
    fn pair_key(&self) -> &str {
        if self.src.is_empty() {
            &self.key
        } else {
            &self.src
        }
    }
}

fn items_of(side: &Side<'_>, kind: &str) -> Vec<Item> {
    if kind == LABEL_KIND {
        return side
            .card
            .labels
            .iter()
            .enumerate()
            .map(|(row, name)| Item {
                row,
                key: fold_key(name),
                d: CardDetail {
                    kind: LABEL_KIND.into(),
                    value: name.clone(),
                    ..Default::default()
                },
                primary: false,
                src: String::new(),
            })
            .collect();
    }
    let mut out: Vec<Item> = side
        .card
        .details
        .iter()
        .enumerate()
        .filter(|(_, d)| d.kind == kind)
        .map(|(row, d)| Item {
            row,
            key: value_key(kind, &d.value, d.parts.as_ref()),
            d: d.clone(),
            primary: false,
            src: side.src_keys.get(row).cloned().unwrap_or_default(),
        })
        .collect();
    // The first of a kind is the primary one; with one item there is nothing to choose.
    if out.len() >= 2 {
        out[0].primary = true;
    }
    out
}

/// An item's attributes as comparable text (absent reads as empty).
fn attrs_of(kind: &str, it: &Item) -> BTreeMap<String, String> {
    attrs_for(kind, &it.d, it.primary)
}

fn attrs_for(kind: &str, d: &CardDetail, primary: bool) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    if kind == LABEL_KIND {
        return m;
    }
    if kind == "address" {
        let parts = address_parts(d);
        if parts.is_empty() {
            m.insert("value".into(), d.value.clone());
        }
        for (k, v) in parts {
            m.insert(format!("parts.{k}"), v);
        }
    } else {
        m.insert("value".into(), d.value.clone());
        if matches!(kind, "work" | "school") {
            // A job has a title and a department; a school only its field of study (`parts.field`,
            // where PM's editor, vCard and dedupe all keep it).
            let keys: &[&str] = if kind == "school" {
                &["field"]
            } else {
                &["title", "department"]
            };
            for key in keys.iter().copied() {
                let v = d
                    .parts
                    .as_ref()
                    .and_then(|p| p.get(key))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                m.insert(format!("parts.{key}"), v.to_string());
            }
            m.insert("from".into(), d.from_date.clone().unwrap_or_default());
            m.insert("to".into(), d.to_date.clone().unwrap_or_default());
            m.insert(
                "current".into(),
                if d.is_current { "1" } else { "0" }.into(),
            );
        }
    }
    // Labels are the kind of text people retype in another case; they compare folded.
    m.insert("label".into(), fold_key(&d.label));
    m.insert("primary".into(), if primary { "1" } else { "0" }.into());
    m
}

/// Whether `b` could be `a` edited on its value alone: every other attribute is the same. Google's
/// array order carries no meaning, so this is the only sign an unmatched new item is the edited
/// twin of a lost one rather than something unrelated that happens to be new.
fn value_edit_of(kind: &str, a: &CardDetail, b: &CardDetail) -> bool {
    let (ma, mb) = (attrs_for(kind, a, false), attrs_for(kind, b, false));
    ma.keys()
        .chain(mb.keys())
        .filter(|k| *k != "value" && *k != "primary")
        .all(|k| attr_text(&ma, k) == attr_text(&mb, k))
}

fn attr_text(m: &BTreeMap<String, String>, attr: &str) -> String {
    m.get(attr).cloned().unwrap_or_else(|| match attr {
        "primary" | "current" => "0".to_string(),
        _ => String::new(),
    })
}

/// Whether two items differ in anything but whether they are the primary one.
fn edited(kind: &str, a: &Item, b: &Item) -> bool {
    let (ma, mb) = (attrs_of(kind, a), attrs_of(kind, b));
    ma.keys()
        .chain(mb.keys())
        .filter(|k| *k != "primary")
        .any(|k| attr_text(&ma, k) != attr_text(&mb, k))
}

/// Copy one attribute from `src` onto `dst`.
fn copy_attr(dst: &mut CardDetail, src: &CardDetail, attr: &str) {
    match attr {
        "value" => dst.value = src.value.clone(),
        "label" => dst.label = src.label.clone(),
        "from" => dst.from_date = src.from_date.clone(),
        "to" => dst.to_date = src.to_date.clone(),
        "current" => dst.is_current = src.is_current,
        a if a.starts_with("parts.") => {
            let key = &a["parts.".len()..];
            if dst.kind == "address" {
                let v = address_parts(src).get(key).cloned().unwrap_or_default();
                *dst = address_with(dst, key, &v);
            } else {
                let v = src
                    .parts
                    .as_ref()
                    .and_then(|p| p.get(key))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let mut obj = dst
                    .parts
                    .as_ref()
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                if v.is_empty() {
                    obj.remove(key);
                } else {
                    obj.insert(key.to_string(), Value::String(v));
                }
                dst.parts = (!obj.is_empty()).then_some(Value::Object(obj));
            }
        }
        _ => {}
    }
}

fn item_json(d: &CardDetail) -> Value {
    serde_json::to_value(d).unwrap_or(Value::Null)
}

fn key_of_json(kind: &str, v: &Value) -> Option<String> {
    if kind == LABEL_KIND {
        return v.get("value").and_then(Value::as_str).map(fold_key);
    }
    let d: CardDetail = serde_json::from_value(v.clone()).ok()?;
    Some(value_key(kind, &d.value, d.parts.as_ref()))
}

// --- the engine --------------------------------------------------------------------------------------

struct Ctx<'a> {
    now: &'a str,
    apply: Vec<Apply>,
    send: Vec<Send>,
    /// The open conflicts handed in, `google` refreshed as the merge goes.
    carried: Vec<Conflict>,
    new: Vec<Conflict>,
    frozen_rows: Vec<usize>,
    /// Whether the kind being merged has two or more items on every side, so "primary" can differ.
    primary_ok: bool,
}

/// Merge `pm` and `google` against `base` (`None`: no base, as if both sides had added everything).
/// `open` are the conflicts stored for the source; `now` stamps new ones.
#[allow(dead_code)] // the sync and the outbound pass (g3, g4) run every merge; until then only the tests do
pub fn merge(
    base: Option<&Side<'_>>,
    pm: &Side<'_>,
    google: &Side<'_>,
    open: &[Conflict],
    now: &str,
) -> Outcome {
    let empty = Card::default();
    let no_base = Side {
        card: &empty,
        starred: false,
        src_keys: &[],
    };
    let base = base.unwrap_or(&no_base);
    let mut ctx = Ctx {
        now,
        apply: Vec::new(),
        send: Vec::new(),
        carried: open.to_vec(),
        new: Vec::new(),
        frozen_rows: Vec::new(),
        primary_ok: false,
    };

    for single in Single::ALL {
        merge_single(
            &mut ctx,
            single,
            single.get(base.card, base.starred),
            single.get(pm.card, pm.starred),
            single.get(google.card, google.starred),
        );
    }
    for kind in MAPPED_KINDS.iter().copied().chain([LABEL_KIND]) {
        merge_kind(
            &mut ctx,
            kind,
            &items_of(base, kind),
            &items_of(pm, kind),
            &items_of(google, kind),
        );
    }

    let mut conflicts = ctx.carried;
    conflicts.extend(ctx.new);
    ctx.frozen_rows.sort_unstable();
    ctx.frozen_rows.dedup();
    Outcome {
        apply: ctx.apply,
        send: ctx.send,
        conflicts,
        frozen_rows: ctx.frozen_rows,
        snapshot: google.card.clone(),
        snapshot_starred: google.starred,
    }
}

fn text_value(v: &Option<String>) -> Value {
    v.as_ref().map_or(Value::Null, |s| json!(s))
}

fn merge_single(
    ctx: &mut Ctx<'_>,
    f: Single,
    b: Option<String>,
    p: Option<String>,
    g: Option<String>,
) {
    // An open conflict freezes the field: only its google value moves.
    if let Some(c) = ctx
        .carried
        .iter_mut()
        .find(|c| c.field == f.word() && c.item_key.is_none() && c.attribute.is_none())
    {
        c.google = text_value(&g);
        return;
    }
    if p == g {
        return;
    }
    if g == b {
        ctx.send.push(Send::Set(f, p));
    } else if p == b {
        ctx.apply.push(Apply::Set(f, g));
    } else {
        ctx.new.push(Conflict {
            field: f.word().to_string(),
            item_key: None,
            attribute: None,
            pm: text_value(&p),
            google: text_value(&g),
            seen_at: ctx.now.to_string(),
        });
    }
}

#[allow(clippy::too_many_lines)] // one pass over one kind; split by phase in the comments
fn merge_kind(ctx: &mut Ctx<'_>, kind: &str, b: &[Item], p: &[Item], g: &[Item]) {
    ctx.primary_ok = b.len() >= 2 && p.len() >= 2 && g.len() >= 2;
    let mut skip_p: HashSet<usize> = HashSet::new();
    let mut skip_g: HashSet<usize> = HashSet::new();
    let mut skip_attr: HashSet<(String, String)> = HashSet::new();
    let mut repoint: Vec<usize> = Vec::new();

    // 1. What an open conflict holds is out of the merge.
    let open_here: Vec<usize> = (0..ctx.carried.len())
        .filter(|i| ctx.carried[*i].field == kind)
        .collect();
    for ci in &open_here {
        let (item_key, attribute) = {
            let c = &ctx.carried[*ci];
            (c.item_key.clone(), c.attribute.clone())
        };
        match (item_key, attribute) {
            (Some(k), None) => {
                if let Some(pi) = p
                    .iter()
                    .enumerate()
                    .position(|(i, it)| !skip_p.contains(&i) && it.pair_key() == k)
                {
                    skip_p.insert(pi);
                    ctx.frozen_rows.push(p[pi].row);
                }
                if let Some(gk) = key_of_json(kind, &ctx.carried[*ci].google) {
                    match g
                        .iter()
                        .enumerate()
                        .position(|(i, it)| !skip_g.contains(&i) && it.key == gk)
                    {
                        Some(gi) => {
                            skip_g.insert(gi);
                            ctx.carried[*ci].google = item_json(&g[gi].d);
                        }
                        None => repoint.push(*ci),
                    }
                }
            }
            (Some(k), Some(a)) => {
                skip_attr.insert((k, a));
            }
            _ => {}
        }
    }

    // 2. Pair each side's items with the base's, in order of appearance.
    let mut taken_p = vec![false; b.len()];
    let mut taken_g = vec![false; b.len()];
    let mut p_of_base: Vec<Option<usize>> = vec![None; b.len()];
    let mut g_of_base: Vec<Option<usize>> = vec![None; b.len()];
    let mut p_paired = vec![false; p.len()];
    let mut g_paired = vec![false; g.len()];
    for (i, it) in p.iter().enumerate() {
        if skip_p.contains(&i) {
            continue;
        }
        let hit = b
            .iter()
            .enumerate()
            .position(|(bi, x)| !taken_p[bi] && x.key == it.pair_key())
            .or_else(|| {
                (!it.src.is_empty())
                    .then(|| {
                        b.iter()
                            .enumerate()
                            .position(|(bi, x)| !taken_p[bi] && x.key == it.key)
                    })
                    .flatten()
            });
        if let Some(bi) = hit {
            taken_p[bi] = true;
            p_of_base[bi] = Some(i);
            p_paired[i] = true;
        }
    }
    for (i, it) in g.iter().enumerate() {
        if skip_g.contains(&i) {
            continue;
        }
        if let Some(bi) = b
            .iter()
            .enumerate()
            .position(|(bi, x)| !taken_g[bi] && x.key == it.key)
        {
            taken_g[bi] = true;
            g_of_base[bi] = Some(i);
            g_paired[i] = true;
        }
    }

    // 3. Items on both sides that the base never had: the same new item agrees.
    let p_new: Vec<usize> = (0..p.len())
        .filter(|i| !p_paired[*i] && !skip_p.contains(i))
        .collect();
    let mut g_new: Vec<usize> = (0..g.len())
        .filter(|i| !g_paired[*i] && !skip_g.contains(i))
        .collect();
    let mut p_lonely: Vec<usize> = Vec::new();
    let mut agreed: Vec<(usize, usize)> = Vec::new();
    for pi in p_new {
        match g_new.iter().position(|gi| g[*gi].key == p[pi].key) {
            Some(at) => agreed.push((pi, g_new.remove(at))),
            None => p_lonely.push(pi),
        }
    }

    // 4. Each base item: both sides still have it, one lost it, or both did.
    for (bi, bitem) in b.iter().enumerate() {
        match (p_of_base[bi], g_of_base[bi]) {
            (Some(pi), Some(gi)) => merge_attrs(
                ctx,
                kind,
                &bitem.key,
                Some(bitem),
                &p[pi],
                &g[gi],
                &skip_attr,
            ),
            (Some(pi), None) => {
                // Google lost it (removed, or its value changed so it no longer pairs). Both sides
                // may have changed it to the same new value: that is an agreement.
                if let Some(at) = g_new.iter().position(|gi| g[*gi].key == p[pi].key) {
                    let gi = g_new.remove(at);
                    merge_attrs(
                        ctx,
                        kind,
                        &bitem.key,
                        Some(bitem),
                        &p[pi],
                        &g[gi],
                        &skip_attr,
                    );
                } else if !edited(kind, &p[pi], bitem) {
                    emit_remove_apply(ctx, kind, &p[pi]);
                } else {
                    // PM edited what Google removed or replaced: ask. Google's replacement is
                    // offered only when it is plainly one: it is the only new Google item left
                    // that PM has no match for, and it is the lost item with its value edited.
                    // Anything else stays in `g_new` and reaches PM as an ordinary addition, so an
                    // unrelated new Google item is never held in a conflict (and, after "Keep
                    // PM's", never read as something PM removed and sent back as a deletion).
                    let unmatched: Vec<usize> = g_new
                        .iter()
                        .copied()
                        .filter(|gi| !p.iter().any(|x| x.key == g[*gi].key))
                        .collect();
                    // Any other base item Google lost while PM still has it could be the one the
                    // new Google item replaces, edited or not, so the new item is not offered.
                    let rivals = (0..b.len())
                        .filter(|j| *j != bi && g_of_base[*j].is_none() && p_of_base[*j].is_some())
                        .count();
                    let replacement = match unmatched.as_slice() {
                        [only] if rivals == 0 && value_edit_of(kind, &bitem.d, &g[*only].d) => {
                            g_new.retain(|gi| gi != only);
                            Some(*only)
                        }
                        _ => None,
                    };
                    ctx.frozen_rows.push(p[pi].row);
                    ctx.new.push(Conflict {
                        field: kind.to_string(),
                        item_key: Some(bitem.key.clone()),
                        attribute: None,
                        pm: item_json(&p[pi].d),
                        google: replacement.map_or(Value::Null, |gi| item_json(&g[gi].d)),
                        seen_at: ctx.now.to_string(),
                    });
                }
            }
            (None, Some(gi)) => {
                // PM lost it.
                if !edited(kind, &g[gi], bitem) {
                    emit_remove_send(ctx, kind, &g[gi]);
                } else {
                    ctx.new.push(Conflict {
                        field: kind.to_string(),
                        item_key: Some(bitem.key.clone()),
                        attribute: None,
                        pm: Value::Null,
                        google: item_json(&g[gi].d),
                        seen_at: ctx.now.to_string(),
                    });
                }
            }
            (None, None) => {}
        }
    }

    // 5. An open conflict whose Google item went away looks for what Google put there instead.
    // Same rule as step 4: only a lone new item that is the old Google item with its value edited.
    // Rivals: the other open conflicts looking for a replacement, and any base item Google lost
    // that PM still holds (it may be the one the new item replaces).
    let rivals = repoint.len()
        + (0..b.len())
            .filter(|j| g_of_base[*j].is_none() && p_of_base[*j].is_some())
            .count();
    for ci in repoint {
        let old: Option<CardDetail> = serde_json::from_value(ctx.carried[ci].google.clone()).ok();
        let unmatched: Vec<usize> = g_new
            .iter()
            .copied()
            .filter(|gi| !p.iter().any(|x| x.key == g[*gi].key))
            .collect();
        match (unmatched.as_slice(), old) {
            ([only], Some(old)) if rivals == 1 && value_edit_of(kind, &old, &g[*only].d) => {
                let only = *only;
                g_new.retain(|gi| *gi != only);
                ctx.carried[ci].google = item_json(&g[only].d);
            }
            _ => ctx.carried[ci].google = Value::Null,
        }
    }

    // 6. The same new item on both sides: agree, attribute by attribute against an empty base.
    for (pi, gi) in agreed {
        merge_attrs(ctx, kind, &p[pi].key, None, &p[pi], &g[gi], &skip_attr);
    }
    // 7. What only PM added is sent; what only Google added is applied.
    for pi in p_lonely {
        emit_add_send(ctx, kind, &p[pi]);
    }
    for gi in g_new {
        emit_add_apply(ctx, kind, &g[gi]);
    }

    // 8. An open attribute conflict follows Google's current value.
    for ci in open_here {
        let (key, attr) = {
            let c = &ctx.carried[ci];
            match (&c.item_key, &c.attribute) {
                (Some(k), Some(a)) => (k.clone(), a.clone()),
                _ => continue,
            }
        };
        if let Some(it) = g.iter().find(|it| it.key == key) {
            let m = attrs_of(kind, it);
            ctx.carried[ci].google = json!(attr_text(&m, &attr));
        }
    }
}

fn merge_attrs(
    ctx: &mut Ctx<'_>,
    kind: &str,
    key: &str,
    b: Option<&Item>,
    p: &Item,
    g: &Item,
    skip_attr: &HashSet<(String, String)>,
) {
    let bm = b.map(|b| attrs_of(kind, b)).unwrap_or_default();
    let pm = attrs_of(kind, p);
    let gm = attrs_of(kind, g);
    let names: BTreeSet<&String> = pm.keys().chain(gm.keys()).chain(bm.keys()).collect();
    let mut apply_attrs: Vec<String> = Vec::new();
    let mut send_attrs: Vec<String> = Vec::new();
    for a in names {
        // A pair neither side had before has no base to say who moved the primary flag, and a kind
        // with a lone item on some side has no choice to make: the item is the primary by being
        // alone, so a removal that leaves one must not read as the survivor being promoted.
        if a == "primary" && (b.is_none() || !ctx.primary_ok) {
            continue;
        }
        if skip_attr.contains(&(key.to_string(), a.clone())) {
            continue;
        }
        let (bv, pv, gv) = (attr_text(&bm, a), attr_text(&pm, a), attr_text(&gm, a));
        if pv == gv {
            continue;
        }
        if gv == bv {
            send_attrs.push(a.clone());
        } else if pv == bv {
            apply_attrs.push(a.clone());
        } else if a == "value" && p.key == g.key {
            // The same item written two ways ("(555) 0101" and "555-0101", or an e-mail in another
            // case): the key already says they agree, so there is nothing to ask.
            continue;
        } else {
            ctx.new.push(Conflict {
                field: kind.to_string(),
                item_key: Some(key.to_string()),
                attribute: Some(a.clone()),
                pm: json!(pv),
                google: json!(gv),
                seen_at: ctx.now.to_string(),
            });
        }
    }
    if !apply_attrs.is_empty() {
        let mut to = p.d.clone();
        for a in &apply_attrs {
            copy_attr(&mut to, &g.d, a);
        }
        ctx.apply.push(Apply::EditItem {
            row: p.row,
            attrs: apply_attrs,
            to,
            primary: g.primary,
        });
    }
    if !send_attrs.is_empty() {
        ctx.send.push(Send::EditItem {
            row: p.row,
            kind: kind.to_string(),
            google_key: g.key.clone(),
            attrs: send_attrs,
            to: p.d.clone(),
            primary: p.primary,
        });
    }
}

fn emit_add_apply(ctx: &mut Ctx<'_>, kind: &str, g: &Item) {
    if kind == LABEL_KIND {
        ctx.apply.push(Apply::AddLabel(g.d.value.clone()));
    } else {
        ctx.apply.push(Apply::AddItem {
            detail: g.d.clone(),
            primary: g.primary,
        });
    }
}

fn emit_remove_apply(ctx: &mut Ctx<'_>, kind: &str, p: &Item) {
    if kind == LABEL_KIND {
        ctx.apply.push(Apply::RemoveLabel(p.d.value.clone()));
    } else {
        ctx.apply.push(Apply::RemoveItem { row: p.row });
    }
}

fn emit_add_send(ctx: &mut Ctx<'_>, kind: &str, p: &Item) {
    if kind == LABEL_KIND {
        ctx.send.push(Send::AddLabel(p.d.value.clone()));
    } else {
        ctx.send.push(Send::AddItem {
            row: p.row,
            detail: p.d.clone(),
            primary: p.primary,
        });
    }
}

fn emit_remove_send(ctx: &mut Ctx<'_>, kind: &str, g: &Item) {
    if kind == LABEL_KIND {
        ctx.send.push(Send::RemoveLabel(g.d.value.clone()));
    } else {
        ctx.send.push(Send::RemoveItem {
            kind: kind.to_string(),
            google_key: g.key.clone(),
        });
    }
}

#[cfg(test)]
#[path = "merge_tests.rs"]
mod tests;
