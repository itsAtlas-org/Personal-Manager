// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The stable `MOD-####` codes a mod call is refused with (Mods spec §1.5), and the one error type
//! that carries them back to the frame as `{code, message}`. ERROR_CODES.md lists every code, and a
//! test here holds the two together.
//!
//! Ranges: `1xxx` the call's shape, `2xxx` permission, `3xxx` limits, `4xxx` the data, `5xxx` the
//! state of PM or the mod, `9xxx` PM's own failure. **Never renumber or reuse a shipped code:** a mod
//! branches on them, and they travel in bug reports.
//!
//! A message is PM's own short sentence. Nothing from a database error, a path or another mod ever
//! reaches one: a failure inside PM is logged in full and answered with [`Code::Internal`]'s fixed
//! text ([`ModError::internal`]).

use std::fmt;

use serde::{Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Code {
    /// No such method or event in this PM's API.
    UnknownMethod,
    /// The method was removed in an API version this PM serves.
    RemovedMethod,
    /// The arguments aren't the shape the method takes.
    BadArgs,
    /// The mod doesn't hold the permission, and asking won't change that (not declared, denied,
    /// never consented to, or refused by a rule such as D3's).
    NotGranted,
    /// The permission is asked for at first use and hasn't been asked yet; PM draws the prompt.
    NeedsPrompt,
    /// Too many calls, or too many at once.
    RateLimited,
    /// The arguments or the answer are past their cap.
    TooLarge,
    /// The mod's private storage is full.
    QuotaExceeded,
    /// A write based on a stale copy; the answer carries the current revision.
    StaleRevision,
    /// Nothing there, or nothing this mod may see (the two are never told apart).
    #[cfg_attr(not(test), allow(dead_code))] // part 3's document calls
    NotFound,
    /// PM is locked or its data is closed.
    Locked,
    /// The mod is paused, parked or not turned on yet.
    Paused,
    /// The call's session ended (the frame closed, the mod changed, PM locked) before it finished.
    SessionEnded,
    /// PM couldn't complete the call.
    Internal,
}

// `ALL` and `name` are the tests' (and the Developer tab's, slice 6).
#[cfg_attr(not(test), allow(dead_code))]
impl Code {
    pub const ALL: [Code; 14] = [
        Self::UnknownMethod,
        Self::RemovedMethod,
        Self::BadArgs,
        Self::NotGranted,
        Self::NeedsPrompt,
        Self::RateLimited,
        Self::TooLarge,
        Self::QuotaExceeded,
        Self::StaleRevision,
        Self::NotFound,
        Self::Locked,
        Self::Paused,
        Self::SessionEnded,
        Self::Internal,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnknownMethod => "MOD-1001",
            Self::RemovedMethod => "MOD-1002",
            Self::BadArgs => "MOD-1003",
            Self::NotGranted => "MOD-2001",
            Self::NeedsPrompt => "MOD-2002",
            Self::RateLimited => "MOD-3001",
            Self::TooLarge => "MOD-3002",
            Self::QuotaExceeded => "MOD-3003",
            Self::StaleRevision => "MOD-4001",
            Self::NotFound => "MOD-4002",
            Self::Locked => "MOD-5001",
            Self::Paused => "MOD-5002",
            Self::SessionEnded => "MOD-5003",
            Self::Internal => "MOD-9001",
        }
    }

    /// The code's name in the page's table (`modBridge.ts`'s `MOD_ERRORS`).
    pub fn name(self) -> &'static str {
        match self {
            Self::UnknownMethod => "unknownMethod",
            Self::RemovedMethod => "removedMethod",
            Self::BadArgs => "badArgs",
            Self::NotGranted => "notGranted",
            Self::NeedsPrompt => "needsPrompt",
            Self::RateLimited => "rateLimited",
            Self::TooLarge => "tooLarge",
            Self::QuotaExceeded => "quotaExceeded",
            Self::StaleRevision => "staleRevision",
            Self::NotFound => "notFound",
            Self::Locked => "locked",
            Self::Paused => "paused",
            Self::SessionEnded => "sessionEnded",
            Self::Internal => "internal",
        }
    }
}

impl Serialize for Code {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// A refused or failed call, as the frame gets it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModError {
    pub code: Code,
    pub message: String,
    /// The permission a [`Code::NeedsPrompt`] or [`Code::NotGranted`] is about, so the page can draw
    /// its prompt (slice 6).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission: Option<&'static str>,
    /// The current revision, with [`Code::StaleRevision`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<i64>,
}

impl ModError {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            permission: None,
            revision: None,
        }
    }

    /// PM failed: `what` is logged for the developer, and the frame gets only the fixed sentence.
    pub fn internal(what: impl fmt::Display) -> Self {
        eprintln!("mods: a call failed inside PM: {what}");
        Self::new(Code::Internal, INTERNAL_MESSAGE)
    }

    pub fn with_permission(mut self, permission: &'static str) -> Self {
        self.permission = Some(permission);
        self
    }

    pub fn with_revision(mut self, revision: i64) -> Self {
        self.revision = Some(revision);
        self
    }

    pub fn bad_args(why: impl Into<String>) -> Self {
        Self::new(Code::BadArgs, why)
    }

    pub fn session_ended() -> Self {
        Self::new(Code::SessionEnded, "This mod's session has ended.")
    }

    pub fn locked() -> Self {
        Self::new(Code::Locked, "PM is locked, or its data is closed.")
    }
}

/// [`Code::Internal`]'s only message.
pub const INTERNAL_MESSAGE: &str = "PM couldn't complete that call.";

impl fmt::Display for ModError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for ModError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// ERROR_CODES.md's MOD table, as `(code, what it means)`.
    fn documented() -> Vec<(String, String)> {
        let doc = include_str!("../../../../ERROR_CODES.md");
        let section = doc
            .split("## `MOD-####`")
            .nth(1)
            .expect("ERROR_CODES.md has a MOD-#### section");
        let section = section.split("\n## ").next().unwrap();
        section
            .lines()
            .filter_map(|line| {
                let cells: Vec<&str> = line.split('|').map(str::trim).collect();
                let code = cells.get(1)?.trim_matches('`');
                code.starts_with("MOD-")
                    .then(|| (code.to_owned(), cells.get(2).unwrap_or(&"").to_string()))
            })
            .collect()
    }

    #[test]
    fn every_code_is_unique_well_formed_and_documented_and_nothing_else_is() {
        let mut codes = BTreeSet::new();
        let mut names = BTreeSet::new();
        for code in Code::ALL {
            let s = code.as_str();
            assert!(
                s.len() == 8 && s.starts_with("MOD-") && s[4..].bytes().all(|b| b.is_ascii_digit()),
                "{s}"
            );
            assert!(codes.insert(s), "{s} is used twice");
            assert!(names.insert(code.name()), "{} is used twice", code.name());
        }
        let documented = documented();
        let in_doc: BTreeSet<&str> = documented.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(
            in_doc.len(),
            documented.len(),
            "ERROR_CODES.md repeats a code"
        );
        assert_eq!(in_doc, codes, "ERROR_CODES.md and Code::ALL disagree");
        for (code, meaning) in &documented {
            assert!(
                !meaning.is_empty(),
                "{code} has no meaning in ERROR_CODES.md"
            );
        }
    }

    /// The codes the page answers with itself (`modBridge.ts`'s `MOD_ERRORS`) are this table's, by
    /// name and number.
    #[test]
    fn the_pages_own_codes_are_these() {
        let page = include_str!("../../../../src/components/mods/modBridge.ts");
        let block = page
            .split("export const MOD_ERRORS = {")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .expect("modBridge.ts has MOD_ERRORS");
        let mut seen = 0;
        for line in block.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let (name, code) = line
                .trim_end_matches(',')
                .split_once(':')
                .unwrap_or_else(|| panic!("MOD_ERRORS line {line:?}"));
            let code = code.trim().trim_matches('"');
            let ours = Code::ALL
                .into_iter()
                .find(|c| c.name() == name.trim())
                .unwrap_or_else(|| panic!("the page's {name} isn't a code here"));
            assert_eq!(ours.as_str(), code, "{name}");
            seen += 1;
        }
        assert!(seen >= 4, "{block}");
    }

    #[test]
    fn an_error_reaches_the_frame_as_code_and_message_and_nothing_internal() {
        let e = ModError::new(Code::StaleRevision, "stale").with_revision(4);
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({"code": "MOD-4001", "message": "stale", "revision": 4})
        );
        let e = ModError::new(Code::NeedsPrompt, "ask").with_permission("documents.text");
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({"code": "MOD-2002", "message": "ask", "permission": "documents.text"})
        );
        // What went wrong inside PM is logged, never sent: a path or SQL in it stays out.
        let e = ModError::internal("no such table: mod_kv at /home/someone/pm.sqlite");
        assert_eq!(e.code, Code::Internal);
        assert_eq!(e.message, INTERNAL_MESSAGE);
        let sent = serde_json::to_string(&e).unwrap();
        assert!(
            !sent.contains("mod_kv") && !sent.contains("/home"),
            "{sent}"
        );
    }
}
