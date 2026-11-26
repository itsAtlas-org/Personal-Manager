// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! "Who did you meet?" (spec 6.2 and 6.3): the meetings still owed an answer, and answering them.
//!
//! Nothing in PM fires when an event ends and PM sends no notifications, so "owed" is a question
//! PM asks whenever it looks, answered from the calendar mirror and the answers on file
//! ([`owed_meetings`]). An occurrence is owed when it ended within the last 14 days and before now,
//! has an iCal id, sits on no quiet calendar, is an ordinary event (not a birthday, focus time, out
//! of office or working location), was not declined by the user, has at least one other human
//! guest, and has no answer, no skipped series and no entry already linked to it.
//!
//! - **Ended** is judged on the user's clock: a timed event by its instant, an all-day event at the
//!   end of its last civil day in the user's zone (`julianday('now')` would never end one).
//! - **Who the user is** comes from the account addresses: every connected Google and Microsoft
//!   account's address ("My addresses"), Google's own `self` guest, and, for a calendar that has an
//!   account (Outlook, whose guests carry no `self`), the guest with that account's address. A feed
//!   with no account at all (ICS) can't tell: the meeting counts as not declined and Skip answers it.
//! - **One entry per occurrence.** The same meeting on two calendars is one card, keyed on the iCal
//!   id and the occurrence's start (every occurrence of a series shares an id). It counts as declined
//!   only when every copy that names the user says so, as the Calendar tab strikes a merged row.
//! - **Guest lists decode in Rust through `text_at`**, never SQL `LIKE` or `json_each`: rows over a
//!   year old keep them as compressed blobs. A guest with no email can't be matched or added and is
//!   left out.
//!
//! Answering ([`answer_meeting`]) writes in one transaction: the people ticked (a guest PM doesn't
//! know becomes a person with no Google link; someone in Trash comes back), one `met` entry naming
//! them, and the answer, an upsert so answering again replaces an earlier skip.
//!
//! Not model-path code (E13). A guest's name or email never reaches a log or an error string.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, SecondsFormat, TimeZone, Utc};
use chrono_tz::Tz;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::fold::{clean_text, email_key};
use super::timeline::{
    clean_entry, clip, insert_entry, instant, EventLink, InteractionDraft, ENTRY_PEOPLE_MAX,
    START_MAX,
};
use super::{
    create_person_in, in_tx, no_such_person, now, refuse, require_transaction, restore_people_in,
    DetailDraft, PersonDraft, PROMPT_ANSWERS, VALUE_MAX,
};
use crate::calendar::{text_at, Attendee};
use crate::error::Result;

/// How far back a meeting is still asked about.
pub const OWED_DAYS: i64 = 14;
/// A room's address ends like this; the guest list doesn't flag rooms (Google's `resource` isn't
/// parsed), so the domain says it.
const ROOM_DOMAIN: &str = "@resource.calendar.google.com";
/// The `event_type`s of an ordinary event (a missing one is ordinary too). Birthday, focus time,
/// out of office, working location and mail-made events are not meetings.
const ORDINARY: &str = "default";

// --- types ---------------------------------------------------------------------------------------

/// A guest PM knows: someone in People (not in Trash) whose email matches the invite. Pre-ticked.
#[derive(Clone, Serialize)]
pub struct KnownGuest {
    pub id: i64,
    pub display_name: String,
    /// The address on the invite.
    pub email: String,
}

/// A guest PM doesn't know. Listed unticked, so one big meeting never creates a crowd by accident.
#[derive(Clone, Serialize)]
pub struct UnknownGuest {
    /// The name on the invite (empty when it has none).
    pub name: String,
    pub email: String,
    /// Set when this address belongs only to someone in Trash: the card says "In Trash", and ticking
    /// them brings that person back (send `{ id }`) instead of making a second one.
    pub trashed_id: Option<i64>,
}

/// One meeting owed an answer.
#[derive(Clone, Serialize)]
pub struct OwedMeeting {
    /// The event's iCal id.
    #[serde(rename = "uid")]
    pub event_ical: String,
    /// The occurrence's start, as the calendar stored it (an instant, or a date for all-day).
    pub start: String,
    pub end: Option<String>,
    pub title: String,
    pub all_day: bool,
    /// Part of a series, so "Don't ask about this series" applies.
    pub recurs: bool,
    pub known: Vec<KnownGuest>,
    pub unknown: Vec<UnknownGuest>,
}

impl std::fmt::Debug for OwedMeeting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwedMeeting")
            .field("known", &self.known.len())
            .field("unknown", &self.unknown.len())
            .finish_non_exhaustive()
    }
}

/// A guest to add to People as the meeting is saved.
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct NewGuest {
    pub name: String,
    pub email: String,
}

/// Who a card ticked: someone already in People (`{ id }`, also someone in Trash, who is brought
/// back) or a guest PM doesn't know yet (`{ new: { name, email } }`).
#[derive(Clone, Deserialize)]
#[serde(untagged)]
pub enum PersonRef {
    Existing { id: i64 },
    Guest { new: NewGuest },
}

impl std::fmt::Debug for PersonRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PersonRef::Existing { .. } => "PersonRef::Existing",
            PersonRef::Guest { .. } => "PersonRef::Guest",
        })
    }
}

/// What the card answered. The words on the wire are [`PROMPT_ANSWERS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingAnswer {
    Logged,
    Skipped,
    SeriesSkipped,
}

impl MeetingAnswer {
    /// The stored word.
    pub fn word(self) -> &'static str {
        match self {
            MeetingAnswer::Logged => PROMPT_ANSWERS[0],
            MeetingAnswer::Skipped => PROMPT_ANSWERS[1],
            MeetingAnswer::SeriesSkipped => PROMPT_ANSWERS[2],
        }
    }
}

/// What answering did, so the card can say so ("2 people came back from Trash").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct MeetingAnswered {
    /// The `met` entry written, when the answer was "logged".
    pub interaction: Option<i64>,
    /// People made from guests PM didn't know.
    pub created: usize,
    /// People brought back from Trash because they were ticked.
    pub restored: usize,
}

// --- the guest list ---------------------------------------------------------------------------------

/// The addresses that are the user's own: every connected Google and Microsoft account's.
fn my_addresses(conn: &Connection) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT account_email FROM connector_sources \
          WHERE provider IN ('google', 'microsoft') AND account_email IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut mine = HashSet::new();
    for address in rows {
        let key = email_key(&address?);
        if !key.is_empty() {
            mine.insert(key);
        }
    }
    Ok(mine)
}

/// A guest's address, trimmed, when it is one.
fn address_of(a: &Attendee) -> Option<&str> {
    a.email
        .as_deref()
        .map(str::trim)
        .filter(|e| e.contains('@'))
}

/// The guest that is the user, on one copy of an event: the one Google marks `self`, or the one
/// whose address is the calendar's own account. With both, the account's address wins (a shared
/// calendar can mark several).
fn the_user<'a>(list: &'a [Attendee], acct: Option<&str>) -> Option<&'a Attendee> {
    let by_acct = acct.and_then(|acct| {
        list.iter()
            .find(|a| address_of(a).is_some_and(|e| email_key(e) == acct))
    });
    by_acct.or_else(|| list.iter().find(|a| a.is_self))
}

/// Whether a guest is someone to ask about: a human other than the user, with an address.
fn is_other_human(a: &Attendee, mine: &HashSet<String>) -> bool {
    let Some(address) = address_of(a) else {
        return false;
    };
    let key = email_key(address);
    !a.is_self && !mine.contains(&key) && !key.ends_with(ROOM_DOMAIN)
}

/// One guest as the invite names them, cleaned and clipped (invite text is other people's).
struct Guest {
    name: String,
    email: String,
}

/// What PM knows of an address: the live person it belongs to, or the one in Trash.
struct Match {
    live: Option<(i64, String)>,
    trashed: Option<i64>,
}

fn match_address(conn: &Connection, key: &str) -> Result<Match> {
    let find = |trashed: bool| -> Result<Option<(i64, String)>> {
        Ok(conn
            .query_row(
                "SELECT p.id, p.display_name FROM person_details d JOIN people p ON p.id = d.person_id \
                  WHERE d.kind = 'email' AND d.value_key = ?1 AND (p.trashed_at IS NOT NULL) = ?2 \
                  ORDER BY p.id LIMIT 1",
                params![key, trashed],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    };
    Ok(Match {
        live: find(false)?,
        trashed: find(true)?.map(|(id, _)| id),
    })
}

/// Split guests into the people PM knows and the ones it doesn't.
fn split_guests(
    conn: &Connection,
    guests: &[Guest],
) -> Result<(Vec<KnownGuest>, Vec<UnknownGuest>)> {
    let mut known: Vec<KnownGuest> = Vec::new();
    let mut unknown = Vec::new();
    let mut seen = HashSet::new();
    for g in guests {
        let hit = match_address(conn, &email_key(&g.email))?;
        match hit.live {
            Some((id, display_name)) => {
                // A person with two invited addresses is one tick.
                if seen.insert(id) {
                    known.push(KnownGuest {
                        id,
                        display_name,
                        email: g.email.clone(),
                    });
                }
            }
            None => unknown.push(UnknownGuest {
                name: g.name.clone(),
                email: g.email.clone(),
                trashed_id: hit.trashed,
            }),
        }
    }
    Ok((known, unknown))
}

// --- what is owed -----------------------------------------------------------------------------------

/// One occurrence, folded across the calendars it is on.
struct Occurrence {
    event_ical: String,
    start: String,
    end: Option<String>,
    title: String,
    all_day: bool,
    recurs: bool,
    guests: Vec<Guest>,
    guest_keys: HashSet<String>,
    /// Some copy names the user, and every copy that does says they declined.
    named: bool,
    all_declined: bool,
}

/// The meetings owed an answer now, in the user's zone, newest first.
pub fn owed_meetings(conn: &Connection) -> Result<Vec<OwedMeeting>> {
    owed_meetings_at(conn, Utc::now(), crate::commands::resolve_zone(conn))
}

/// [`owed_meetings`] at an injected instant and zone (the test seam, as `clock`'s `*_at` twins).
pub fn owed_meetings_at(
    conn: &Connection,
    now: DateTime<Utc>,
    zone: Tz,
) -> Result<Vec<OwedMeeting>> {
    let today = crate::clock::today_sql_in_at(zone, now);
    let rfc = |t: DateTime<Utc>| t.to_rfc3339_opts(SecondsFormat::Secs, true);
    let (now_text, since_text) = (rfc(now), rfc(now - Duration::days(OWED_DAYS)));
    let mine = my_addresses(conn)?;

    // Candidates by id, calendar, kind and the clock; guests, answers and links are judged in Rust
    // below. An all-day event's `end` is the day after its last day, so it has ended when that date
    // is today or before, and is still asked about while the last day is within the window.
    let mut stmt = conn.prepare(
        "SELECT e.summary, e.start, e.end, e.all_day, e.uid, e.recurring, e.series_id, e.attendees, \
                (SELECT s.account_email FROM calendars c JOIN connector_sources s ON s.id = c.source_id \
                  WHERE c.id = e.calendar_id) \
           FROM calendar_events e \
          WHERE e.uid IS NOT NULL AND e.uid <> '' \
            AND e.calendar_id NOT IN (SELECT id FROM calendars WHERE quiet = 1) \
            AND COALESCE(NULLIF(e.event_type, ''), ?1) = ?1 \
            AND (CASE WHEN e.all_day = 1 \
                      THEN date(COALESCE(e.end, date(e.start, '+1 day'))) > date(?2, '-14 days') \
                       AND date(COALESCE(e.end, date(e.start, '+1 day'))) <= ?2 \
                      ELSE julianday(COALESCE(e.end, e.start)) >= julianday(?3) \
                       AND julianday(COALESCE(e.end, e.start)) < julianday(?4) END) \
          ORDER BY e.start, e.calendar_id, e.id",
    )?;
    let mut at: HashMap<(String, String), usize> = HashMap::new();
    let mut found: Vec<Occurrence> = Vec::new();
    let mut rows = stmt.query(params![ORDINARY, today, since_text, now_text])?;
    while let Some(r) = rows.next()? {
        let (event_ical, start): (String, String) = (r.get(4)?, r.get(1)?);
        let attendees = text_at(r, 7)?;
        let acct: Option<String> = r.get(8)?;
        let acct = acct.map(|a| email_key(&a)).filter(|a| !a.is_empty());
        let list: Vec<Attendee> = attendees
            .as_deref()
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or_default();
        let slot = *at
            .entry((event_ical.clone(), start.clone()))
            .or_insert_with(|| {
                found.push(Occurrence {
                    event_ical: event_ical.clone(),
                    start: start.clone(),
                    end: None,
                    title: String::new(),
                    all_day: false,
                    recurs: false,
                    guests: Vec::new(),
                    guest_keys: HashSet::new(),
                    named: false,
                    all_declined: true,
                });
                found.len() - 1
            });
        let o = &mut found[slot];
        if o.title.is_empty() {
            let title: String = r.get(0)?;
            o.title = clip(&clean_text(&title), VALUE_MAX);
            o.end = r.get(2)?;
            o.all_day = r.get::<_, i64>(3)? != 0;
        }
        let series: Option<String> = r.get(6)?;
        o.recurs |= r.get::<_, i64>(5)? != 0 || series.is_some_and(|s| !s.is_empty());
        if let Some(me) = the_user(&list, acct.as_deref()) {
            o.named = true;
            o.all_declined &= me.response.as_deref() == Some("declined");
        }
        for a in list.iter().filter(|a| is_other_human(a, &mine)) {
            let email = clip(&clean_text(address_of(a).unwrap_or_default()), VALUE_MAX);
            if o.guest_keys.insert(email_key(&email)) {
                o.guests.push(Guest {
                    name: clip(
                        &clean_text(a.name.as_deref().unwrap_or_default()),
                        VALUE_MAX,
                    ),
                    email,
                });
            }
        }
    }
    drop(rows);
    drop(stmt);

    let mut answered = conn.prepare(
        "SELECT EXISTS(SELECT 1 FROM meet_prompt_answers WHERE event_uid = ?1 AND event_start = ?2) \
             OR EXISTS(SELECT 1 FROM meet_prompt_answers WHERE event_uid = ?1 AND answer = ?3) \
             OR EXISTS(SELECT 1 FROM interactions WHERE event_uid = ?1 AND event_start = ?2)",
    )?;
    let mut owed: Vec<(DateTime<Utc>, OwedMeeting)> = Vec::new();
    for o in found {
        if o.guests.is_empty() || (o.named && o.all_declined) {
            continue;
        }
        let done: bool = answered.query_row(
            params![o.event_ical, o.start, super::PROMPT_ANSWERS[2]],
            |r| r.get(0),
        )?;
        if done {
            continue;
        }
        let (known, unknown) = split_guests(conn, &o.guests)?;
        let ended = ended_at(&o, zone);
        owed.push((
            ended,
            OwedMeeting {
                event_ical: o.event_ical,
                start: o.start,
                end: o.end,
                title: o.title,
                all_day: o.all_day,
                recurs: o.recurs,
                known,
                unknown,
            },
        ));
    }
    // Newest first; ties keep a stable order.
    owed.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.start.cmp(&a.1.start))
            .then_with(|| a.1.event_ical.cmp(&b.1.event_ical))
    });
    Ok(owed.into_iter().map(|(_, m)| m).collect())
}

/// When the occurrence ended, for ordering: a timed one by its end, an all-day one at the user's
/// midnight that closes its last day.
fn ended_at(o: &Occurrence, zone: Tz) -> DateTime<Utc> {
    let timed = |text: &str| {
        DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|d| d.with_timezone(&Utc))
    };
    if !o.all_day {
        return timed(o.end.as_deref().unwrap_or(&o.start)).unwrap_or_default();
    }
    let day = |text: &str| {
        chrono::NaiveDate::parse_from_str(text.get(0..10).unwrap_or_default(), "%Y-%m-%d").ok()
    };
    let closes = o
        .end
        .as_deref()
        .and_then(day)
        .or_else(|| day(&o.start).map(|d| d + Duration::days(1)));
    closes
        .and_then(|d| {
            // A zone whose clocks skip midnight (a DST gap there, e.g. Havana) has no 00:00 that
            // day: the first local time that exists is the day's start, so ordering stays right.
            (0..24).find_map(|h| {
                zone.from_local_datetime(&d.and_hms_opt(h, 0, 0)?)
                    .earliest()
            })
        })
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_default()
}

// --- answering --------------------------------------------------------------------------------------

/// Write the answer row, replacing an earlier one for the same occurrence (logging a skipped
/// meeting replaces the skip). A skip never overwrites a "logged": the entry is on file.
fn put_answer(tx: &Connection, event_ical: &str, start: &str, answer: MeetingAnswer) -> Result<()> {
    let stamp = now(tx)?;
    tx.execute(
        "INSERT INTO meet_prompt_answers(event_uid, event_start, answer, answered_at) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(event_uid, event_start) DO UPDATE \
            SET answer = excluded.answer, answered_at = excluded.answered_at \
          WHERE excluded.answer = ?5 OR meet_prompt_answers.answer <> ?5",
        params![
            event_ical,
            start,
            answer.word(),
            stamp,
            MeetingAnswer::Logged.word()
        ],
    )?;
    Ok(())
}

/// One ticked person as an id, creating or restoring them as the reference needs.
fn resolve(tx: &Connection, who: &PersonRef, out: &mut MeetingAnswered) -> Result<i64> {
    match who {
        PersonRef::Existing { id } => {
            let trashed: Option<bool> = tx
                .query_row(
                    "SELECT trashed_at IS NOT NULL FROM people WHERE id = ?1",
                    params![id],
                    |r| r.get(0),
                )
                .optional()?;
            match trashed {
                None => Err(no_such_person()),
                Some(false) => Ok(*id),
                Some(true) => {
                    out.restored += restore_people_in(tx, &[*id])?;
                    Ok(*id)
                }
            }
        }
        PersonRef::Guest { new } => {
            let email = clean_text(&new.email);
            let name = clean_text(&new.name);
            if email.is_empty() && name.is_empty() {
                return Err(refuse("A guest needs a name or an email."));
            }
            // An address PM already holds is that person, never a second one (two ticks of one
            // address, or a guest who has since been added by hand, or one in Trash).
            if !email.is_empty() {
                let hit = match_address(tx, &email_key(&email))?;
                if let Some((id, _)) = hit.live {
                    return Ok(id);
                }
                if let Some(id) = hit.trashed {
                    out.restored += restore_people_in(tx, &[id])?;
                    return Ok(id);
                }
            }
            let mut draft = PersonDraft {
                display_name: name,
                ..Default::default()
            };
            if !email.is_empty() {
                draft.details.push(DetailDraft {
                    kind: "email".into(),
                    value: email,
                    ..Default::default()
                });
            }
            let id = create_person_in(tx, &draft)?;
            out.created += 1;
            Ok(id)
        }
    }
}

/// Answer one meeting in the caller's transaction (see the module docs). `event_ical` is empty for
/// an event with no iCal id: it gets the entry, with its title and no link, and no answer row (so
/// it can only be logged). `start` is the occurrence's start as the calendar stores it. `title`
/// defaults to the mirrored event's.
pub fn answer_meeting_in(
    tx: &Connection,
    event_ical: &str,
    start: &str,
    people: &[PersonRef],
    answer: MeetingAnswer,
    note: Option<&str>,
    title: Option<&str>,
) -> Result<MeetingAnswered> {
    require_transaction(tx)?;
    let event_ical = clean_text(event_ical);
    let start = clean_text(start);
    let mut out = MeetingAnswered::default();
    match answer {
        MeetingAnswer::Skipped | MeetingAnswer::SeriesSkipped => {
            if event_ical.is_empty() {
                return Err(refuse(
                    "That meeting has no id PM can remember an answer against.",
                ));
            }
            if event_ical.chars().count() > VALUE_MAX {
                return Err(refuse("The meeting's id is too long."));
            }
            if answer == MeetingAnswer::Skipped {
                if start.is_empty() {
                    return Err(refuse("That meeting needs its start time."));
                }
                if start.chars().count() > START_MAX {
                    return Err(refuse("The meeting's start time is too long."));
                }
                put_answer(tx, &event_ical, &start, answer)?;
            } else {
                // A series answer covers every occurrence: its start is empty.
                put_answer(tx, &event_ical, "", answer)?;
            }
            Ok(out)
        }
        MeetingAnswer::Logged => {
            if people.is_empty() {
                return Err(refuse("Pick at least one person, or skip the meeting."));
            }
            if people.len() > ENTRY_PEOPLE_MAX {
                return Err(refuse(format!(
                    "An entry can name up to {ENTRY_PEOPLE_MAX} people ({} given).",
                    people.len()
                )));
            }
            let linked = !event_ical.is_empty();
            if linked && start.is_empty() {
                return Err(refuse("That meeting needs its start time."));
            }
            let zone = crate::commands::resolve_zone(tx);
            let on_date = if start.is_empty() {
                crate::clock::today_sql_in_at(zone, Utc::now())
            } else {
                crate::clock::zone_date_of(&start, zone)
            };
            // A blank title counts as none: the mirror's summary is the better label.
            let title = match title.map(str::trim).filter(|t| !t.is_empty()) {
                Some(t) => t.to_string(),
                None if linked => tx
                    .query_row(
                        "SELECT summary FROM calendar_events WHERE uid = ?1 AND start = ?2 \
                          ORDER BY id LIMIT 1",
                        params![event_ical, start],
                        |r| r.get(0),
                    )
                    .optional()?
                    .unwrap_or_default(),
                None => String::new(),
            };
            let draft = InteractionDraft {
                kind: super::timeline::KIND_MET.into(),
                on_date,
                at: instant(&start),
                note: note.unwrap_or_default().to_string(),
                people: Vec::new(),
                event: linked.then(|| EventLink {
                    ical: event_ical.clone(),
                    start: start.clone(),
                    title: title.clone(),
                }),
            };
            let mut entry = clean_entry(&draft, true)?;
            if !linked {
                // No id to link, but the title is still worth keeping.
                let t = clip(&clean_text(&title), VALUE_MAX);
                entry.title = (!t.is_empty()).then_some(t);
            }
            let mut ids: Vec<i64> = Vec::new();
            for who in people {
                let id = resolve(tx, who, &mut out)?;
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            out.interaction = Some(insert_entry(tx, &entry, &ids)?);
            if linked {
                put_answer(tx, &event_ical, &start, answer)?;
            }
            Ok(out)
        }
    }
}

/// Answer one meeting. See [`answer_meeting_in`].
pub fn answer_meeting(
    conn: &Connection,
    event_ical: &str,
    start: &str,
    people: &[PersonRef],
    answer: MeetingAnswer,
    note: Option<&str>,
    title: Option<&str>,
) -> Result<MeetingAnswered> {
    in_tx(conn, |tx| {
        answer_meeting_in(tx, event_ical, start, people, answer, note, title)
    })
}
