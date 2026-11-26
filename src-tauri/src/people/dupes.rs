// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Possible duplicates: the finder behind "Possible duplicates (N)" (spec 4.1 and 4.2).
//!
//! **Pure after one read.** [`load`] reads everyone outside Trash and the "not the same person"
//! pairs while the caller holds the DB guard; the caller drops the guard and [`scan`] does the
//! comparing, so a big address book never holds the one connection while it thinks.
//!
//! **Four reasons, kept apart (I-07).** A pair is reported with every reason that applies and
//! never scored into one number:
//!
//! | reason | rule |
//! |---|---|
//! | [`Reason::Email`] | an equal `email_key` on any two details |
//! | [`Reason::Phone`] | an equal `digits_key`, or one a suffix of the other with 9 or more digits (a national number against its international form, trunk zero ignored) |
//! | [`Reason::Name`] | an equal folded display name, or an equal folded given + family |
//! | [`Reason::Similar`] | the same folded family name and given names where one is a prefix of the other, or the same words in another order |
//!
//! People are grouped by key (email, the last 9 digits of a phone, family name, display name) and
//! compared only within a group, so the work stays small. **A group over [`GROUP_MAX`] people is
//! reported as skipped, never as clean**, and so is a scan that stopped at [`PAIRS_MAX`] pairs: the
//! report says what it did not check.
//!
//! Each pair also carries what differs between the two ([`Differs`]) and whether merging them
//! would lose nothing at all (the "Nothing differs" group), judged by the merge's own rule, so the
//! group's promise is the merge's behaviour. "Similar name" pairs never join that group.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::fold::fold_key;
use super::merge::{differences, load_live, nothing_differs, Differs, Facts};
use super::{in_tx, no_such_person, refuse};
use crate::error::Result;

/// The most people one group may hold before the scan skips it and says so.
pub const GROUP_MAX: usize = 200;
/// The most pairs one report holds; past that the report says it was cut short.
pub const PAIRS_MAX: usize = 1_000;
/// Details a card shows (the card is for recognising a person, not for editing).
const CARD_DETAILS_MAX: usize = 20;
/// Characters of a note a card shows.
const CARD_NOTE_CHARS: usize = 160;

/// Why two people look like one. Declared strongest first: the order is the order a pair's reasons
/// are listed and pairs are sorted in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum Reason {
    Email,
    Phone,
    Name,
    Similar,
}

/// What [`load`] reads: everyone outside Trash, and the pairs marked "not the same person".
pub struct DupeInput {
    people: Vec<Facts>,
    /// `(low, high)` uids.
    distinct: HashSet<(String, String)>,
}

/// Read what the scan needs. Hold the DB guard for this only.
pub fn load(conn: &Connection) -> Result<DupeInput> {
    let people = load_live(conn)?;
    let mut stmt = conn.prepare("SELECT low_uid, high_uid FROM person_distinct_pairs")?;
    let distinct = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<HashSet<(String, String)>, _>>()?;
    Ok(DupeInput { people, distinct })
}

/// One detail on a card.
#[derive(Clone, Serialize)]
pub struct CardDetail {
    pub kind: String,
    pub label: String,
    pub value: String,
}

/// What a duplicate pair shows of one person: enough to tell the two apart without opening either.
/// Holds contact values: no `Debug`.
#[derive(Clone, Serialize)]
pub struct DupeCard {
    pub id: i64,
    #[serde(rename = "uid")]
    pub ident: String,
    pub display_name: String,
    #[serde(rename = "birthday")]
    pub born_on: Option<String>,
    pub met_where: String,
    pub met_on: Option<String>,
    /// The start of the note, or a word for a long one.
    pub note: String,
    pub details: Vec<CardDetail>,
}

/// Two people who may be one. `a` is the older record (the lower id): the one that stays in a
/// merge nobody chose the direction of.
#[derive(Clone, Debug, Serialize)]
pub struct DupePair {
    pub a: i64,
    pub b: i64,
    /// Every reason that applies, strongest first.
    pub reasons: Vec<Reason>,
    pub differs: Differs,
    /// A merge of these two loses nothing (the "Nothing differs" group).
    pub nothing_differs: bool,
}

/// A group the scan did not compare because it was too big to be a duplicate list.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct SkippedGroup {
    pub reason: Reason,
    pub people: usize,
}

/// The finder's snapshot (spec 4.1).
#[derive(Clone, Serialize)]
pub struct DupesReport {
    /// The pairs to review, strongest reason first.
    pub pairs: Vec<DupePair>,
    /// Everyone in `pairs`, once.
    pub cards: Vec<DupeCard>,
    /// Pairs the scan found that you said aren't the same person.
    pub hidden: usize,
    /// Groups too big to compare. A report with any is never "clean".
    pub skipped: Vec<SkippedGroup>,
    /// The scan stopped at [`PAIRS_MAX`] pairs: there may be more.
    pub cut_short: bool,
    /// How many people it looked at.
    pub scanned: usize,
}

impl std::fmt::Debug for DupesReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DupesReport")
            .field("pairs", &self.pairs.len())
            .field("skipped", &self.skipped.len())
            .finish_non_exhaustive()
    }
}

impl DupesReport {
    /// The scan looked at everything: no group skipped, nothing cut short. Only then is "no pairs"
    /// the same as "no duplicates".
    pub fn complete(&self) -> bool {
        self.skipped.is_empty() && !self.cut_short
    }
}

// --- the keys --------------------------------------------------------------------------------------------

/// A phone key's comparable core: no `+`, no leading zeros (the trunk prefix of a national number).
fn phone_core(key: &str) -> &str {
    key.trim_start_matches('+').trim_start_matches('0')
}

/// The group a phone key belongs to: the last nine digits of a number long enough to be matched by
/// suffix, else the key itself (a short number matches only an equal one).
pub(super) fn phone_group(key: &str) -> String {
    let core = phone_core(key);
    if core.len() >= 9 {
        format!("t{}", &core[core.len() - 9..])
    } else {
        format!("k{key}")
    }
}

pub(super) fn phones_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (x, y) = (phone_core(a), phone_core(b));
    x.len() >= 9 && y.len() >= 9 && (x.ends_with(y) || y.ends_with(x))
}

fn words(folded: &str) -> Vec<&str> {
    let mut w: Vec<&str> = folded.split(' ').filter(|s| !s.is_empty()).collect();
    w.sort_unstable();
    w
}

/// Two different folded given names that are "Sam" and "Samuel" (one a prefix of the other, at
/// least two characters, so an initial never flags everyone), or the same words in another order.
fn similar_given(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() || a == b {
        return false;
    }
    let (short, long) = if a.chars().count() <= b.chars().count() {
        (a, b)
    } else {
        (b, a)
    };
    (short.chars().count() >= 2 && long.starts_with(short)) || words(a) == words(b)
}

/// One member of a group: the person (an index into the input) and the key that put them there.
type Member = (usize, String);

fn grouped(
    people: &[Facts],
    keys: impl Fn(&Facts) -> Vec<(String, String)>,
) -> HashMap<String, Vec<Member>> {
    let mut out: HashMap<String, Vec<Member>> = HashMap::new();
    for (i, p) in people.iter().enumerate() {
        let mut seen: HashSet<String> = HashSet::new();
        for (group, payload) in keys(p) {
            if !group.is_empty() && seen.insert(group.clone()) {
                out.entry(group).or_default().push((i, payload));
            }
        }
    }
    out
}

/// Compare within each group and note a pair that matches; a group past [`GROUP_MAX`] is recorded
/// as skipped instead.
fn compare(
    groups: HashMap<String, Vec<Member>>,
    reason: Reason,
    matches: impl Fn(&str, &str) -> bool,
    found: &mut HashMap<(usize, usize), Vec<Reason>>,
    skipped: &mut Vec<SkippedGroup>,
) {
    for members in groups.into_values() {
        if members.len() < 2 {
            continue;
        }
        if members.len() > GROUP_MAX {
            skipped.push(SkippedGroup {
                reason,
                people: members.len(),
            });
            continue;
        }
        for (x, (i, ki)) in members.iter().enumerate() {
            for (j, kj) in &members[x + 1..] {
                if i != j && matches(ki, kj) {
                    let key = if i < j { (*i, *j) } else { (*j, *i) };
                    let why = found.entry(key).or_default();
                    if !why.contains(&reason) {
                        why.push(reason);
                    }
                }
            }
        }
    }
}

fn card_of(p: &Facts) -> DupeCard {
    // Email and phone first: they are what flags a pair, so the cap must never cut them (the stable
    // sort keeps each kind's own order).
    let mut details: Vec<_> = p.details.iter().collect();
    details.sort_by_key(|d| d.kind != "email" && d.kind != "phone");
    DupeCard {
        id: p.id,
        ident: p.ident.clone(),
        display_name: p.display_name.clone(),
        born_on: p.born_on.clone(),
        met_where: p.met_where.clone(),
        met_on: p.met_on.clone(),
        note: if p.notes_are_fingerprint() {
            "A long note".to_string()
        } else {
            p.notes.chars().take(CARD_NOTE_CHARS).collect()
        },
        details: details
            .into_iter()
            .take(CARD_DETAILS_MAX)
            .map(|d| CardDetail {
                kind: d.kind.clone(),
                label: d.label.clone(),
                value: d.value.clone(),
            })
            .collect(),
    }
}

/// Find the possible duplicates among `input`. Pure: no database, no clock.
pub fn scan(input: &DupeInput) -> DupesReport {
    let people = &input.people;
    let mut found: HashMap<(usize, usize), Vec<Reason>> = HashMap::new();
    let mut skipped: Vec<SkippedGroup> = Vec::new();

    let of_kind = |p: &Facts, kind: &'static str| -> Vec<String> {
        p.details
            .iter()
            .filter(|d| d.kind == kind && !d.value_key.is_empty())
            .map(|d| d.value_key.clone())
            .collect()
    };

    compare(
        grouped(people, |p| {
            of_kind(p, "email")
                .into_iter()
                .map(|k| (k, String::new()))
                .collect()
        }),
        Reason::Email,
        |_, _| true,
        &mut found,
        &mut skipped,
    );
    compare(
        grouped(people, |p| {
            of_kind(p, "phone")
                .into_iter()
                .map(|k| (phone_group(&k), k))
                .collect()
        }),
        Reason::Phone,
        phones_match,
        &mut found,
        &mut skipped,
    );
    compare(
        grouped(people, |p| {
            let both = fold_key(&format!("{} {}", p.given, p.family));
            vec![
                (fold_key(&p.display_name), String::new()),
                (both, String::new()),
            ]
        }),
        Reason::Name,
        |_, _| true,
        &mut found,
        &mut skipped,
    );
    compare(
        grouped(people, |p| vec![(fold_key(&p.family), fold_key(&p.given))]),
        Reason::Similar,
        similar_given,
        &mut found,
        &mut skipped,
    );

    let mut hidden = 0;
    let mut pairs: Vec<DupePair> = Vec::new();
    for ((i, j), mut why) in found {
        let (x, y) = if people[i].id < people[j].id {
            (&people[i], &people[j])
        } else {
            (&people[j], &people[i])
        };
        let ordered = if x.ident < y.ident {
            (x.ident.clone(), y.ident.clone())
        } else {
            (y.ident.clone(), x.ident.clone())
        };
        if input.distinct.contains(&ordered) {
            hidden += 1;
            continue;
        }
        // The same name and a similar one is one finding: the stronger.
        if why.contains(&Reason::Name) {
            why.retain(|r| *r != Reason::Similar);
        }
        why.sort_unstable();
        let differs = differences(x, y);
        pairs.push(DupePair {
            a: x.id,
            b: y.id,
            nothing_differs: !why.contains(&Reason::Similar) && nothing_differs(x, y),
            differs,
            reasons: why,
        });
    }
    let by_id: HashMap<i64, &Facts> = people.iter().map(|p| (p.id, p)).collect();
    pairs.sort_by(|p, q| {
        let sort_name = |id: i64| by_id.get(&id).map(|f| fold_key(&f.display_name));
        (p.reasons.first(), sort_name(p.a), p.a, p.b).cmp(&(
            q.reasons.first(),
            sort_name(q.a),
            q.a,
            q.b,
        ))
    });
    let cut_short = pairs.len() > PAIRS_MAX;
    pairs.truncate(PAIRS_MAX);

    let mut shown: Vec<i64> = pairs.iter().flat_map(|p| [p.a, p.b]).collect();
    shown.sort_unstable();
    shown.dedup();
    let cards = shown
        .iter()
        .filter_map(|id| by_id.get(id).map(|p| card_of(p)))
        .collect();
    skipped.sort_by_key(|s| (s.reason, std::cmp::Reverse(s.people)));

    DupesReport {
        pairs,
        cards,
        hidden,
        skipped,
        cut_short,
        scanned: people.len(),
    }
}

// --- "not the same person" -----------------------------------------------------------------------------

/// Say two people are not the same: the finder never offers the pair again. Stored by their `uid`s,
/// ordered low to high, so it survives a restore and a merge rewrites it by rule. Marking a pair
/// twice is fine. Returns whether a new row was written.
pub fn mark_distinct(conn: &Connection, a: i64, b: i64) -> Result<bool> {
    if a == b {
        return Err(refuse("Pick two different people."));
    }
    in_tx(conn, |tx| {
        let ident_of = |id: i64| -> Result<String> {
            tx.query_row("SELECT uid FROM people WHERE id = ?1", params![id], |r| {
                r.get(0)
            })
            .optional()?
            .ok_or_else(no_such_person)
        };
        let (x, y) = (ident_of(a)?, ident_of(b)?);
        let (low, high) = if x < y { (x, y) } else { (y, x) };
        let n = tx.execute(
            "INSERT OR IGNORE INTO person_distinct_pairs(low_uid, high_uid) VALUES (?1, ?2)",
            params![low, high],
        )?;
        Ok(n > 0)
    })
}

/// Forget every "not the same person" answer ("Show them again"). Returns how many were forgotten.
pub fn restore_distinct(conn: &Connection) -> Result<usize> {
    Ok(conn.execute("DELETE FROM person_distinct_pairs", [])?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_national_number_matches_its_international_form() {
        assert!(phones_match("+442079460000", "02079460000"));
        assert!(phones_match("02079460000", "+442079460000"));
        assert_eq!(phone_group("+442079460000"), phone_group("02079460000"));
    }

    #[test]
    fn short_numbers_match_only_when_equal() {
        assert!(phones_match("1234", "1234"));
        assert!(!phones_match("1234", "01234"));
        assert!(!phones_match("5550100", "905550100"));
    }

    #[test]
    fn similar_given_names_are_prefixes_or_reorderings() {
        assert!(similar_given("sam", "samuel"));
        assert!(similar_given("mary ann", "ann mary"));
        assert!(!similar_given("sam", "sam"));
        assert!(!similar_given("s", "samuel"));
        assert!(!similar_given("", "sam"));
        assert!(!similar_given("sam", "tom"));
    }
}
