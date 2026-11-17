// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The two experimental calls that put something on PM's own surface instead of answering from
//! data: `secrets.set` (PM's key field) and `notify` (an in-app strip). Rust authorises and counts;
//! PM's page draws (`ModFrame.tsx`), because only the page owns the screen. Both answer with a
//! [`PageAction`], which the page performs synchronously from arguments Rust checked and never from
//! anything the frame said.
//!
//! **`secrets.set({name})`** asks PM to show the field where the person types the key for the
//! manifest's slot `name` (the value goes from that field to Rust's `mod_secret_save`, never to
//! the frame, spec §1.7). The mod learns only `{set}`: whether PM holds a key for the slot as of
//! the call. The person may be typing one now; the mod asks again to find out. Rust refuses an
//! undeclared slot, and counts the asks of a frame session (`SECRET_ASKS_PER_SESSION`) under the
//! runtime's lock, so a mod cannot make the field open over and over. Showing one field at a time
//! per mod is the page's to keep (only it knows when a field is still open).
//!
//! **`notify({text, tone?})`** shows a strip with the mod's name and tier, in PM's band above the
//! page, text only (the page draws it as React text: no link, no button of its own). Rust holds the
//! text to one plain line of at most [`limits::NOTIFY_MAX_CHARS`] characters (a refusal, never a
//! repair, and the same rule the manifest's own names are held to), the tone to `info`, `warn` or
//! `good`, and the mod to [`limits::NOTIFY_PER_DAY`] notices a day by the user's local date, counted
//! in `mods.notify_day`/`notify_count` by ONE statement inside the transaction that re-checks the
//! mod. A notice the page then cannot show still counts: the limit bounds what a mod may ask.

use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};

use super::handlers::parse;
use super::{
    blocking, limits, recheck, still_current, Authorised, Code, Host, ModError, PageAction, Reply,
};
use crate::mods::package::plain_text_fault;
use crate::mods::secrets;

// --- secrets.set --------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretsArgs {
    name: String,
}

pub(super) async fn secrets_set<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let SecretsArgs { name } = parse(args)?;
    let page_args = {
        let (host, auth) = (host.clone(), auth.clone());
        blocking(move || ask_for_key(&host, &auth, &name)).await?
    };
    still_current(host, auth)?;
    Ok(Reply::Page {
        action: PageAction::SecretsSet,
        args: page_args,
    })
}

/// The blocking step: the mod as it is now, the slot it named, and the ask counted. What the page
/// is told is the slot's name, host and header (from the INSTALLED manifest, so the field can say
/// where the key will go) and whether a key is held now. Never the key.
fn ask_for_key<H: Host>(host: &H, auth: &Authorised, name: &str) -> Result<Value, ModError> {
    let conn = host.store().conn().map_err(|_| ModError::locked())?;
    recheck(host, &conn, auth)?;
    let slots = secrets::status(&conn, auth.ticket.mod_id.as_str()).map_err(|e| match e {
        secrets::Refused::NotInstalled => ModError::session_ended(),
        other => ModError::internal(format!("{other:?}")),
    })?;
    let Some(slot) = slots.into_iter().find(|s| s.name == name) else {
        return Err(ModError::bad_args("This mod has no key by that name."));
    };
    // Counted only for a slot that exists, under the registry's lock: the last ask is taken once.
    let counted = host
        .runtime()
        .note_secret_ask(&auth.ticket, limits::SECRET_ASKS_PER_SESSION)
        .map_err(|_| ModError::session_ended())?;
    if !counted {
        return Err(ModError::new(
            Code::RateLimited,
            "This mod has asked for a key too many times for now.",
        ));
    }
    Ok(json!({
        "name": slot.name,
        "host": slot.host,
        "header": slot.header,
        "set": slot.set,
    }))
}

// --- notify -------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NotifyArgs {
    text: String,
    #[serde(default)]
    tone: Option<String>,
}

/// How a notice is drawn: PM's own word for it, never the mod's styling.
pub(super) const TONES: [&str; 3] = ["info", "warn", "good"];

/// The notice's text: the spaces around it dropped, then held to one plain line of at most
/// [`limits::NOTIFY_MAX_CHARS`] characters. The sentences name the rule, never the text.
fn checked_text(raw: &str) -> Result<String, ModError> {
    let text = raw.trim_matches(' ');
    plain_text_fault(text, limits::NOTIFY_MAX_CHARS, false, false).map_err(|fault| {
        // The fault's own words ("is longer than 140 characters", "has a hidden or control
        // character") are PM's fixed vocabulary, never the mod's text.
        ModError::bad_args(format!(
            "A notice must be one plain line of at most {} characters; this one {fault}.",
            limits::NOTIFY_MAX_CHARS
        ))
    })?;
    Ok(text.to_owned())
}

fn checked_tone(tone: Option<&str>) -> Result<&'static str, ModError> {
    match tone {
        None => Ok(TONES[0]),
        Some(t) => TONES
            .into_iter()
            .find(|known| *known == t)
            .ok_or_else(|| ModError::bad_args("A notice's tone is info, warn or good.")),
    }
}

/// Count one notice for `mod_id` on `today` (the user's local date, `YYYY-MM-DD`) if it has fewer
/// than `per_day` already: one statement, so two notices cannot both take the last one. A different
/// stored day starts the count again at one. `Ok(false)` when the day's notices are used up.
pub(super) fn take_notice_slot(
    conn: &Connection,
    mod_id: &str,
    today: &str,
    per_day: u32,
) -> rusqlite::Result<bool> {
    let changed = conn.execute(
        "UPDATE mods SET \
            notify_count = CASE WHEN notify_day IS ?2 THEN notify_count + 1 ELSE 1 END, \
            notify_day = ?2 \
         WHERE id = ?1 AND (notify_day IS NOT ?2 OR notify_count < ?3)",
        params![mod_id, today, per_day],
    )?;
    Ok(changed == 1)
}

pub(super) async fn notify<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let NotifyArgs { text, tone } = parse(args)?;
    let text = checked_text(&text)?;
    let tone = checked_tone(tone.as_deref())?;
    {
        let (host, auth) = (host.clone(), auth.clone());
        blocking(move || {
            #[cfg(test)]
            host.at(super::Point::BeforeData);
            let mut conn = host.store().conn().map_err(|_| ModError::locked())?;
            let tx = conn.transaction().map_err(ModError::internal)?;
            recheck(&host, &tx, &auth)?;
            let today = crate::clock::today_sql_in(crate::commands::resolve_zone(&tx));
            let taken = take_notice_slot(
                &tx,
                auth.ticket.mod_id.as_str(),
                &today,
                limits::NOTIFY_PER_DAY,
            )
            .map_err(ModError::internal)?;
            if !taken {
                return Err(ModError::new(
                    Code::RateLimited,
                    "This mod has shown all the notices it may today.",
                ));
            }
            #[cfg(test)]
            host.at(super::Point::BeforeCommit);
            // Immediately before the commit, as every writing handler does: a call that stopped
            // being current rolls its count back with the transaction.
            still_current(&host, &auth)?;
            tx.commit().map_err(ModError::internal)
        })
        .await?;
    }
    still_current(host, auth)?;
    Ok(Reply::Page {
        action: PageAction::Notify,
        args: json!({ "text": text, "tone": tone }),
    })
}
