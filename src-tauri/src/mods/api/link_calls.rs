// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `shell.openLink` (PR C, K2): a web address in the person's browser, from a mod (permission
//! `links`, class egress: D3 judges it like `internet`, so an unsigned mod may read your data or
//! open links, never both).
//!
//! **An address can carry data out**, so the rules are strict, and they are the same at the moment of
//! asking and at the moment of the person's yes ([`check_link`] runs at both):
//! - `https` only: no `http`, no `mailto` (which would hand a message to the mail app), no `file`;
//! - no user name or password (`https://trusted.com@evil.com` says one thing and opens another);
//! - a real website NAME: not an IP address in any spelling the parser reads as one (`2130706433`,
//!   `0x7f.1`, `[::1]`), not `localhost` or a `.localhost` name, not a name with a single label
//!   (`intranet`), the root dot ignored;
//! - at most [`limits::LINK_MAX_BYTES`] bytes, with no control character and no space around it.
//!
//! **Parsed ONCE.** The text a mod sends is parsed here, and the PARSED address ([`Url::as_str`], the
//! normal form: lower-cased host, punycode for an international name, default port dropped) is the
//! only thing PM ever shows or opens, so what the person agreed to is what opens, and a trick in the
//! spelling (a `\` read as `/`, a tab inside the address) shows as what it really is.
//!
//! **Who is asked.** A first-party mod's `links` grant at install is the whole of it, and the link
//! opens at once. A second-party or unsigned mod's goes through PM's own question every time
//! (`mods::confirm`, [`CONFIRM_EACH_TIME`](super::permissions::CONFIRM_EACH_TIME)), which shows the
//! FULL address (clamped at [`limits::LINK_SHOWN_CHARS`] with how many characters more there are),
//! not only the host: a path or a query is the channel. Either way it is opened with
//! [`Host::open_link`], which never waits for the launcher, and never on the main thread.

use reqwest::Url;
use serde::Deserialize;
use serde_json::{json, Value};

use super::calls;
use super::confirm::{self, Target};
use super::handlers::parse;
use super::permissions::{confirm_each_time, ConfirmKind};
use super::{limits, still_current, Authorised, Host, ModError, Reply};
use crate::mods::confirm::Action;
use crate::net_screen;

/// The one refusal, in PM's words: it never repeats the address.
fn refused() -> ModError {
    ModError::bad_args("A mod may open only an https:// address to a website.")
}

/// The address a mod may open, parsed, or the refusal (see the module doc for the rules).
pub(super) fn check_link(raw: &str) -> Result<Url, ModError> {
    if raw.len() > limits::LINK_MAX_BYTES || raw.trim() != raw || raw.chars().any(char::is_control)
    {
        return Err(refused());
    }
    let url = Url::parse(raw).map_err(|_| refused())?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err(refused());
    }
    let host = url.host_str().ok_or_else(refused)?;
    if net_screen::literal_ip(host).is_some() {
        return Err(refused());
    }
    let name = host.trim_end_matches('.');
    if name.is_empty() || !name.contains('.') || name == "localhost" || name.ends_with(".localhost")
    {
        return Err(refused());
    }
    Ok(url)
}

/// What PM asks about an address: its question, then the whole address as the page draws it (in a
/// monospace line of its own), clamped, and how much more there is.
fn question(address: &str) -> String {
    let total = address.chars().count();
    let shown: String = address.chars().take(limits::LINK_SHOWN_CHARS).collect();
    if total > limits::LINK_SHOWN_CHARS {
        format!(
            "Open this address in your browser?\n{shown}\n...and {} more characters",
            total - limits::LINK_SHOWN_CHARS
        )
    } else {
        format!("Open this address in your browser?\n{shown}")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkArgs {
    url: String,
}

/// `shell.openLink({url})` → `{opened}`; or, for a second-party or unsigned mod, `MOD-2003` carrying
/// PM's question.
pub(super) async fn open_link<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let LinkArgs { url } = parse(args)?;
    // Parsed once: from here on, only this is shown or opened.
    let address = check_link(&url)?.to_string();
    let value = calls::read(host, auth, move |cx| {
        if confirm_each_time(cx.facts.tier, ConfirmKind::Link) {
            return Err(confirm::request(
                cx.host,
                cx.conn,
                cx.auth,
                Action::OpenLink { url: address },
            ));
        }
        // The last check before something leaves PM: under the guard an uninstall needs, so none can
        // land between this and the launcher (which only starts a process and waits for nothing).
        still_current(cx.host, cx.auth)?;
        Ok(json!({"opened": cx.host.open_link(&address)}))
    })
    .await?;
    Ok(Reply::value(value))
}

// --- the person's yes ---------------------------------------------------------------------------

/// The address a question is about, as PM reads it now (the rules run again, on the stored text).
pub(super) fn read(stored: &str) -> Result<Target, ModError> {
    let address = check_link(stored)?.to_string();
    Ok(Target {
        summary: question(&address),
        facts: address,
    })
}

/// Open the address the person agreed to, the guard held and the call just re-checked.
pub(super) fn perform<H: Host>(
    host: &H,
    auth: &Authorised,
    stored: &str,
) -> Result<Value, ModError> {
    let address = check_link(stored)?.to_string();
    still_current(host, auth)?;
    Ok(json!({"opened": host.open_link(&address)}))
}

#[cfg(test)]
pub(super) fn question_for_test(address: &str) -> String {
    question(address)
}
