// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The stable `MOD-####` codes a mod call is refused with (Mods spec §1.5), and the one error type
//! that carries them back to the frame as `{code, message}`. ERROR_CODES.md lists every code, and a
//! test here holds the two together.
//!
//! Ranges: `1xxx` the call's shape, `2xxx` permission, `3xxx` limits, `4xxx` the data, `5xxx` the
//! state of PM or the mod, `6xxx` the network (`net.fetch`), `9xxx` PM's own failure. **Never
//! renumber or reuse a shipped code:** a mod branches on them, and they travel in bug reports. The
//! codes of the experimental calls (PR B: `ai.complete`, `net.fetch`, `secrets.set`, `notify`) are
//! appended below `Internal` in `Code::ALL`; their numbers freeze at the first release after PR C,
//! not at merge (D13).
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
    NotFound,
    /// PM is locked or its data is closed.
    Locked,
    /// The mod is paused, parked or not turned on yet.
    Paused,
    /// The call's session ended (the frame closed, the mod changed, PM locked) before it finished.
    SessionEnded,
    /// PM couldn't complete the call.
    Internal,
    // The experimental calls' codes (PR B). Appended, never reordered.
    /// The mod's monthly AI limit is reached (or it has none set on a paid route).
    AiCapReached,
    /// The call waited longer than `limits::CALL_TIMEOUT` and was stopped.
    CallTimedOut,
    /// PM has no AI model to answer with (no provider is set up).
    NoModel,
    /// The model's price is unknown, so its worst-case cost can't be reserved: fails closed.
    PriceUnknown,
    /// The model didn't answer (every failure of the route, whatever the provider said).
    ModelCallFailed,
    /// The website isn't one the mod declared (or the URL isn't one a mod may fetch).
    HostNotAllowed,
    /// The website's address isn't on the public internet.
    HostNotPublic,
    /// The website's name couldn't be resolved.
    HostUnresolved,
    /// The website couldn't be reached in time (offline, refused, or too slow).
    NetworkUnavailable,
    /// The website redirected more times than PM follows.
    TooManyRedirects,
    /// The website's answer is past `limits::MAX_FETCH_RESPONSE_BYTES`.
    ResponseTooLarge,
    /// The mod's key for the website has not been typed in.
    SecretNotSet,
    // PR C's codes (K1). Appended, never reordered.
    /// The document engine isn't set up, and PM provisions nothing for a mod (the page shows a PM
    /// "Set it up" button).
    EngineNotReady,
    /// The call needs the person's yes first: the error carries the token PM's own question is
    /// answered with, and the question's text ([`Confirmation`]).
    NeedsConfirmation,
    /// The person said no to PM's question.
    Declined,
}

// `ALL` and `name` are the tests' (and the Developer tab's, slice 6).
#[cfg_attr(not(test), allow(dead_code))]
impl Code {
    pub const ALL: [Code; 29] = [
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
        Self::AiCapReached,
        Self::CallTimedOut,
        Self::NoModel,
        Self::PriceUnknown,
        Self::ModelCallFailed,
        Self::HostNotAllowed,
        Self::HostNotPublic,
        Self::HostUnresolved,
        Self::NetworkUnavailable,
        Self::TooManyRedirects,
        Self::ResponseTooLarge,
        Self::SecretNotSet,
        Self::EngineNotReady,
        Self::NeedsConfirmation,
        Self::Declined,
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
            Self::AiCapReached => "MOD-3004",
            Self::CallTimedOut => "MOD-3005",
            Self::NoModel => "MOD-5004",
            Self::PriceUnknown => "MOD-5005",
            Self::ModelCallFailed => "MOD-5006",
            Self::HostNotAllowed => "MOD-6001",
            Self::HostNotPublic => "MOD-6002",
            Self::HostUnresolved => "MOD-6003",
            Self::NetworkUnavailable => "MOD-6004",
            Self::TooManyRedirects => "MOD-6005",
            Self::ResponseTooLarge => "MOD-6006",
            Self::SecretNotSet => "MOD-6007",
            Self::EngineNotReady => "MOD-5007",
            Self::NeedsConfirmation => "MOD-2003",
            Self::Declined => "MOD-2004",
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
            Self::AiCapReached => "aiCapReached",
            Self::CallTimedOut => "callTimedOut",
            Self::NoModel => "noModel",
            Self::PriceUnknown => "priceUnknown",
            Self::ModelCallFailed => "modelCallFailed",
            Self::HostNotAllowed => "hostNotAllowed",
            Self::HostNotPublic => "hostNotPublic",
            Self::HostUnresolved => "hostUnresolved",
            Self::NetworkUnavailable => "networkUnavailable",
            Self::TooManyRedirects => "tooManyRedirects",
            Self::ResponseTooLarge => "responseTooLarge",
            Self::SecretNotSet => "secretNotSet",
            Self::EngineNotReady => "engineNotReady",
            Self::NeedsConfirmation => "needsConfirmation",
            Self::Declined => "declined",
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
    /// The question PM's page must ask, with [`Code::NeedsConfirmation`] (and only then).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm: Option<Confirmation>,
}

/// What PM's page needs to ask the person one yes/no question for a call: the single-use token its
/// answer is sent back with (`mod_confirm`), and the text of the question. The text is built in Rust
/// from PM's own data, never from what the mod said (`mods::confirm`). It is for PM's page: the page
/// never hands the token to the frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Confirmation {
    pub token: String,
    pub summary: String,
}

impl ModError {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            permission: None,
            revision: None,
            confirm: None,
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

// The experimental calls' errors (PR B). Each is one fixed PM sentence: nothing a provider said,
// no URL, host, header, path or number from the request ever reaches one (D-25), so a mod can't
// use the text to learn what PM saw, and a malicious website or model can't write into it. The
// handlers use these and no other sentence for these codes.
impl ModError {
    pub fn ai_cap_reached() -> Self {
        Self::new(
            Code::AiCapReached,
            "This mod has reached its monthly AI limit.",
        )
    }

    pub fn call_timed_out() -> Self {
        Self::new(
            Code::CallTimedOut,
            "That call took too long and was stopped.",
        )
    }

    pub fn no_model() -> Self {
        Self::new(Code::NoModel, NO_MODEL_MESSAGE)
    }

    pub fn price_unknown() -> Self {
        Self::new(
            Code::PriceUnknown,
            "PM can't tell what that AI model costs, so it won't run it for a mod.",
        )
    }

    pub fn model_call_failed() -> Self {
        Self::new(Code::ModelCallFailed, "The AI model didn't answer.")
    }

    /// The person's own daily spending limit has paused the provider the call would use. The money
    /// code a mod already handles for its own cap ("no AI spend right now"), with a sentence of its
    /// own: no provider and no figure, because the limit's own sentence carries today's spend and
    /// the limit, which a mod must not learn.
    pub fn ai_provider_paused() -> Self {
        Self::new(
            Code::AiCapReached,
            "PM's daily AI spending limit has paused AI calls for now.",
        )
    }

    pub fn host_not_allowed() -> Self {
        Self::new(
            Code::HostNotAllowed,
            "That isn't a web address this mod may use.",
        )
    }

    pub fn host_not_public() -> Self {
        Self::new(
            Code::HostNotPublic,
            "That website isn't on the public internet.",
        )
    }

    pub fn host_unresolved() -> Self {
        Self::new(Code::HostUnresolved, "PM couldn't find that website.")
    }

    pub fn network_unavailable() -> Self {
        Self::new(
            Code::NetworkUnavailable,
            "PM couldn't reach that website in time.",
        )
    }

    pub fn too_many_redirects() -> Self {
        Self::new(
            Code::TooManyRedirects,
            "That website sent PM around too many times.",
        )
    }

    pub fn response_too_large() -> Self {
        Self::new(
            Code::ResponseTooLarge,
            "That website's answer is larger than PM allows.",
        )
    }

    pub fn key_not_stored() -> Self {
        Self::new(
            Code::SecretNotSet,
            "This mod's key for that website hasn't been set.",
        )
    }
}

// PR C's errors (K1), each one fixed PM sentence like the experimental calls' above.
impl ModError {
    pub fn engine_not_ready() -> Self {
        Self::new(
            Code::EngineNotReady,
            "PM's document engine isn't set up yet.",
        )
    }

    /// The call waits for the person's yes. `summary` is PM's own question, built from PM's data.
    pub fn needs_confirmation(token: String, summary: String) -> Self {
        Self {
            confirm: Some(Confirmation { token, summary }),
            ..Self::new(
                Code::NeedsConfirmation,
                "PM needs the person's OK before this can go ahead.",
            )
        }
    }

    pub fn declined() -> Self {
        Self::new(Code::Declined, "The person chose not to allow that.")
    }

    /// A confirmation PM no longer holds: answered already, run out, revoked or never asked. It is
    /// answered `MOD-2004` like a no, so a token that isn't live does nothing and tells a caller
    /// nothing more than that the person didn't agree to it.
    pub fn confirmation_gone() -> Self {
        Self::new(
            Code::Declined,
            "That question has already been answered or has run out, so nothing was done.",
        )
    }

    /// What the person was asked about changed while the question was open.
    pub fn changed_while_asking() -> Self {
        Self::new(
            Code::StaleRevision,
            "That changed while the question was open, so nothing was done.",
        )
    }
}

/// [`Code::Internal`]'s only message.
pub const INTERNAL_MESSAGE: &str = "PM couldn't complete that call.";

/// [`Code::NoModel`]'s message. It also covers a Background role on a person's own provider key,
/// which a mod can never use (that route isn't zero data retention), so the person who reads it
/// isn't sent looking for a model they already set up. Short on purpose: every experimental code's
/// sentence is held to 80 characters with nothing from a request in it, so ERROR_CODES.md carries
/// the full explanation ("...which mods can't use").
const NO_MODEL_MESSAGE: &str =
    "PM has no AI model to answer with, or Background uses your own key.";

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

    /// The sentence a mod sees for MOD-5004 and the one ERROR_CODES.md gives it are the same words:
    /// a Background role on your own key refuses a mod's AI call with this code, and the
    /// explanation has to be where the person looks.
    #[test]
    fn the_no_model_sentence_and_its_documented_meaning_agree() {
        let (_, meaning) = documented()
            .into_iter()
            .find(|(c, _)| c == "MOD-5004")
            .expect("MOD-5004 is documented");
        // The code's own sentence is short (the 80-character rule), so it says "Background uses your
        // own key" and the table says why that is no answer: mods can't use it.
        assert!(
            meaning.contains("Background is on your own key, which mods can't use"),
            "{meaning}"
        );
        let message = ModError::no_model().to_string();
        assert!(
            message.contains("Background uses your own key"),
            "{message}"
        );
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
        // The experimental calls' codes are all in the page's table (the page's `MOD_ERRORS` lists
        // them so a page-side answer names them by word).
        for code in &Code::ALL[SHIPPED_BY_PR_A.len()..] {
            assert!(
                block.contains(&format!("{}: \"{}\"", code.name(), code.as_str())),
                "the page doesn't list {}",
                code.name()
            );
        }
        assert_eq!(seen, 7 + Code::ALL.len() - SHIPPED_BY_PR_A.len(), "{block}");
    }

    /// The codes PR A shipped, which a mod may already branch on. A test pins them so a later edit
    /// can't renumber one (the codes in the ranges below are free until the first release after C).
    const SHIPPED_BY_PR_A: [(&str, &str); 14] = [
        ("unknownMethod", "MOD-1001"),
        ("removedMethod", "MOD-1002"),
        ("badArgs", "MOD-1003"),
        ("notGranted", "MOD-2001"),
        ("needsPrompt", "MOD-2002"),
        ("rateLimited", "MOD-3001"),
        ("tooLarge", "MOD-3002"),
        ("quotaExceeded", "MOD-3003"),
        ("staleRevision", "MOD-4001"),
        ("notFound", "MOD-4002"),
        ("locked", "MOD-5001"),
        ("paused", "MOD-5002"),
        ("sessionEnded", "MOD-5003"),
        ("internal", "MOD-9001"),
    ];

    #[test]
    fn the_codes_pr_a_shipped_keep_their_numbers_and_stay_first() {
        for (i, (name, number)) in SHIPPED_BY_PR_A.iter().enumerate() {
            assert_eq!(
                (Code::ALL[i].name(), Code::ALL[i].as_str()),
                (*name, *number),
                "slot {i}"
            );
        }
    }

    /// Every code of the experimental calls has its one fixed sentence, and the sentence carries
    /// nothing from a request (a digit, a double quote, a colon or a slash would be a leak waiting
    /// to happen): D-25.
    #[test]
    fn every_experimental_code_has_one_fixed_sentence_with_nothing_from_a_request() {
        let made = [
            ModError::ai_cap_reached(),
            ModError::call_timed_out(),
            ModError::no_model(),
            ModError::price_unknown(),
            ModError::model_call_failed(),
            ModError::host_not_allowed(),
            ModError::host_not_public(),
            ModError::host_unresolved(),
            ModError::network_unavailable(),
            ModError::too_many_redirects(),
            ModError::response_too_large(),
            ModError::key_not_stored(),
            // PR C's (K1), in `Code::ALL`'s order.
            ModError::engine_not_ready(),
            ModError::needs_confirmation(String::new(), String::new()),
            ModError::declined(),
        ];
        let new_codes: Vec<Code> = Code::ALL[SHIPPED_BY_PR_A.len()..].to_vec();
        assert_eq!(
            made.iter().map(|e| e.code).collect::<Vec<_>>(),
            new_codes,
            "a code of the experimental calls has no constructor, or they are out of order"
        );
        let mut sentences = BTreeSet::new();
        for e in &made {
            assert!(sentences.insert(e.message.clone()), "{} repeats", e.message);
            assert!(e.message.ends_with('.') && e.message.len() <= 80, "{e}");
            assert!(
                !e.message
                    .chars()
                    .any(|c| c.is_ascii_digit() || "\"/:\\@".contains(c)),
                "{e}"
            );
            assert!(e.permission.is_none() && e.revision.is_none());
        }
        // The network range is 6xxx, and the money and timeout codes are with their kind.
        for code in [
            Code::HostNotAllowed,
            Code::HostNotPublic,
            Code::HostUnresolved,
            Code::NetworkUnavailable,
            Code::TooManyRedirects,
            Code::ResponseTooLarge,
            Code::SecretNotSet,
        ] {
            assert!(code.as_str().starts_with("MOD-6"), "{}", code.as_str());
        }
        assert_eq!(Code::AiCapReached.as_str(), "MOD-3004");
        assert_eq!(Code::CallTimedOut.as_str(), "MOD-3005");
    }

    /// PR C's three codes (K1) keep the numbers the plan gave them, sit last in `Code::ALL` after
    /// `SecretNotSet`, and the confirmation travels with its token and its question; no other error
    /// carries one.
    #[test]
    fn prs_c_codes_are_pinned_and_a_confirmation_carries_its_token_and_question() {
        let tail: Vec<(&str, &str)> = Code::ALL[Code::ALL.len() - 4..]
            .iter()
            .map(|c| (c.name(), c.as_str()))
            .collect();
        assert_eq!(
            tail,
            [
                ("secretNotSet", "MOD-6007"),
                ("engineNotReady", "MOD-5007"),
                ("needsConfirmation", "MOD-2003"),
                ("declined", "MOD-2004"),
            ]
        );
        let e = ModError::needs_confirmation("ab".into(), "Delete it?".into());
        assert_eq!(e.code, Code::NeedsConfirmation);
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({
                "code": "MOD-2003",
                "message": e.message,
                "confirm": {"token": "ab", "summary": "Delete it?"}
            })
        );
        for plain in [
            ModError::declined(),
            ModError::engine_not_ready(),
            ModError::confirmation_gone(),
            ModError::changed_while_asking(),
        ] {
            assert!(plain.confirm.is_none(), "{plain}");
            assert!(serde_json::to_value(&plain)
                .unwrap()
                .get("confirm")
                .is_none());
        }
        assert_eq!(ModError::declined().code, Code::Declined);
        assert_eq!(ModError::engine_not_ready().code, Code::EngineNotReady);
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
