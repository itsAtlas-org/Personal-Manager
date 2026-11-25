// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `calendar.agenda` (experimental, PR C3): what is coming up on the user's calendar, for a mod
//! holding `calendar.read`.
//!
//! **The same agenda the assistant paths read** ([`crate::calendar::agenda_now`]): events not yet
//! ended, soonest first, one row per occurrence, quiet calendars left out. Unlike them it also leaves
//! out an event the user declined, which the calendar view strikes through but a mod has no use for.
//! The query's LIMIT runs before its dedup and that filter, so it is asked for [`MAX_LIMIT`] rows and
//! the page is cut to `limit` after both: a declined series ahead of a real event doesn't push the
//! event off the page. (Past [`MAX_LIMIT`] rows in the window a page can still come up short.) It
//! never reads the whole mirror ([`crate::calendar::list_events_between`] is deliberately unfiltered
//! by quiet). A title or a location is cut at [`TEXT_MAX_CHARS`] characters, and a uid longer than
//! [`UID_MAX_CHARS`] is left out (`null`); a start or an end is cut at [`TIME_MAX_CHARS`].
//!
//! **What an event tells.** Exactly `{uid, title, start, end, allDay, location, recurring}`, built
//! field by field. `uid` is the provider's iCal UID, or `null` when the event has none: never PM's row
//! id, which embeds the calendar and account ids (often an email address). No guest list, organiser,
//! description, join link or provider link, and no calendar or account id: those are
//! `calendar.details`' to give, which nothing here reads. An event marked private or confidential is
//! returned as PM's own views return it, because the mod holds `calendar.read`.
//!
//! The pieces take a [`Projection`], not a mod, so the MCP server answers the same question from the
//! scopes its client was given.

use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};

use super::calls::{self, internal};
use super::handlers::parse_object;
use super::projection::{Dto, Needs, Projection};
use super::{Authorised, Host, ModError, Reply};
use crate::calendar::{AgendaEvent, AGENDA_DAYS};

/// The most days ahead a call may look, and the most events it may ask for.
pub(crate) const MAX_DAYS: i64 = 60;
pub(crate) const MAX_LIMIT: usize = 250;
/// How many events a page has when `limit` is left out.
pub(crate) const DEFAULT_LIMIT: usize = 50;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgendaArgs {
    #[serde(default)]
    days: Option<i64>,
    #[serde(default)]
    limit: Option<usize>,
}

/// An agenda question, checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Query {
    pub days: i64,
    pub limit: usize,
}

impl Query {
    /// The arguments as a question: `days` an integer 1 to [`MAX_DAYS`] (three weeks when left out),
    /// `limit` an integer 1 to [`MAX_LIMIT`] ([`DEFAULT_LIMIT`] when left out).
    pub(crate) fn parse(args: &Value) -> Result<Self, ModError> {
        // Nothing at all is `{}`: every argument is optional.
        let args = if args.is_null() { &json!({}) } else { args };
        let AgendaArgs { days, limit } = parse_object(args)?;
        let days = days.unwrap_or(AGENDA_DAYS);
        if !(1..=MAX_DAYS).contains(&days) {
            return Err(ModError::bad_args(format!("days is 1 to {MAX_DAYS}.")));
        }
        let limit = limit.unwrap_or(DEFAULT_LIMIT);
        if limit == 0 || limit > MAX_LIMIT {
            return Err(ModError::bad_args(format!("limit is 1 to {MAX_LIMIT}.")));
        }
        Ok(Self { days, limit })
    }
}

/// The most characters of a title or a location an event shows. A subscribed feed (or a shared-vault
/// writer) controls both columns, so without a bound one hostile row could push a page past the
/// result cap and make the whole call fail.
pub(crate) const TEXT_MAX_CHARS: usize = 200;

/// The most characters of an iCal UID a mod is given. Real UIDs are far shorter (RFC 5545 asks for a
/// globally unique id, in practice a GUID or `id@host`), and the mirror stores the feed's UID
/// uncut. A longer one is sent as no uid rather than a cut one: two long UIDs that share a prefix
/// would otherwise look like the same event to a mod correlating by it.
pub(crate) const UID_MAX_CHARS: usize = 512;

/// The most characters of a start or an end an event shows. The columns are text a writer of the
/// store controls, like the title; a real one ("2026-12-05T10:00:00+01:00") is far shorter, so a cut
/// only ever touches a hostile row, which would otherwise sink a whole page.
pub(crate) const TIME_MAX_CHARS: usize = 64;

/// `text` cut to at most `max` characters, at a character boundary.
fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// An event as a caller with this `projection` gets it, field by field from the event's own columns
/// (never the struct as it is, so a column added to an event can never leak by being added).
pub(crate) fn event_dto(e: &AgendaEvent, projection: &Projection) -> Value {
    let event = &e.event;
    Dto::new(*projection)
        // An empty uid is no uid: a feed that left it blank can't be correlated either. An absurdly
        // long one is none too (see [`UID_MAX_CHARS`]); the char count stops at the cap, so a huge
        // value costs no more than the cap to check.
        .field(
            Needs::Nothing,
            "uid",
            event
                .uid
                .clone()
                .filter(|u| !u.is_empty() && u.chars().nth(UID_MAX_CHARS).is_none()),
        )
        .field(
            Needs::Nothing,
            "title",
            clip(&event.summary, TEXT_MAX_CHARS),
        )
        .field(Needs::Nothing, "start", clip(&event.start, TIME_MAX_CHARS))
        .field(
            Needs::Nothing,
            "end",
            event.end.as_deref().map(|t| clip(t, TIME_MAX_CHARS)),
        )
        .field(Needs::Nothing, "allDay", event.all_day)
        .field(
            Needs::Nothing,
            "location",
            event.location.as_deref().map(|l| clip(l, TEXT_MAX_CHARS)),
        )
        .field(Needs::Nothing, "recurring", event.recurring)
        .build()
}

/// The agenda for `query`, as the DTOs a caller with `projection` gets: not ended, soonest first,
/// without quiet calendars or declined events.
pub(crate) fn agenda(
    conn: &Connection,
    projection: &Projection,
    query: &Query,
) -> Result<Vec<Value>, ModError> {
    let today = crate::clock::today_sql_in(crate::commands::resolve_zone(conn));
    // Asked for the most a page may be, whatever `limit` is: the query's LIMIT runs before the
    // declined filter, so asking for just `limit` rows would spend the page on declined occurrences
    // (a declined daily series) and lose the real events behind them with no cursor to find them by.
    let events =
        crate::calendar::agenda_now(conn, query.days, MAX_LIMIT, &today).map_err(internal)?;
    Ok(events
        .iter()
        .filter(|e| !e.declined)
        .take(query.limit)
        .map(|e| event_dto(e, projection))
        .collect())
}

/// `calendar.agenda({days?, limit?})` → `{events: [{uid, title, start, end, allDay, location,
/// recurring}]}`.
pub(super) async fn agenda_call<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let query = Query::parse(args)?;
    let events = calls::read(host, auth, move |cx| {
        let projection = Projection::of_grants(cx.conn, cx.facts, cx.host.engine_sealed())?;
        agenda(cx.conn, &projection, &query)
    })
    .await?;
    Ok(Reply::value(json!({ "events": events })))
}
