// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

use std::path::Path;

use serde::{Serialize, Serializer};

/// Machine-branchable classification of a vault-path failure, so the UI can pick the
/// right recovery action (Repair access / rejoin / passphrase prompt) instead of
/// string-matching. Kebab-case on the wire to match the TS union in `src/lib/types.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VaultFaultCode {
    /// The OS refused access (Windows `ERROR_ACCESS_DENIED` / POSIX `EACCES`) — the
    /// folder is there but this account can't open it. Repairable; never a brick.
    Denied,
    /// The file/folder is gone (deleted folder, unplugged drive, renamed path).
    NotFound,
    /// The folder answers but holds no PM vault (no `vault-meta.json`).
    NoVault,
    /// The vault is here — settings, Markdown, or both — but `pm.sqlite` is gone.
    /// Distinct from [`Self::NoVault`] because the recovery is the opposite one:
    /// nothing may be deleted, and the store must NOT be silently re-created.
    /// `rusqlite::Connection::open` CREATES a missing file, so without this code an
    /// erased store comes back as a fresh, fully-migrated, empty library that looks
    /// healthy — the "all my documents vanished" report this classification exists to
    /// stop. See [`crate::vault::store_presence`].
    StoreMissing,
    /// The passphrase verifier rejected the derived key — the folder and metadata are fine.
    WrongPassphrase,
    /// The verifier accepted the key but the store itself won't open — damaged files.
    Corrupt,
    /// The store opened, but a newer PM wrote it: its schema is past this build's migrations
    /// (`db::NEWER_STORE_MSG`). Healthy data, so like [`Self::Denied`] and [`Self::StoreMissing`]
    /// it never arms a destructive recovery; the only way forward is a newer PM (INVARIANTS.md I-28).
    NewerStore,
    /// Everything else (transient file locks, disk I/O, parse errors).
    Other,
}

/// A vault failure the UI can branch on AND display verbatim: the classification, a
/// user-voiced operation ("read the vault's settings"), the path involved, and a
/// ready-to-show sentence. This is what `Error::Vault` serializes to the webview —
/// the one structured rejection shape (every other variant stays a bare string).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VaultFault {
    pub code: VaultFaultCode,
    pub op: String,
    pub path: Option<String>,
    pub message: String,
}

impl VaultFault {
    /// Fold any backend error into a fault for the state slots that must always hold one
    /// (boot, status, engage): a `Vault` error passes its fault through untouched; every
    /// other variant becomes `Other` carrying its display string, attributed to `op`.
    pub fn from_error(op: &str, e: &Error) -> Self {
        match e {
            Error::Vault(f) => f.clone(),
            other => VaultFault {
                code: VaultFaultCode::Other,
                op: op.to_string(),
                path: None,
                message: other.to_string(),
            },
        }
    }
}

/// The one way a vault-path `std::io::Error` becomes a structured `Error::Vault`:
/// `.map_err(io_at("read the vault's settings", &path))`. Classifies by `ErrorKind`
/// (PermissionDenied → Denied, NotFound → NotFound, else Other) and composes the
/// fallback sentence — screens with better copy branch on the code instead.
pub fn io_at(op: &'static str, path: &Path) -> impl FnOnce(std::io::Error) -> Error {
    let path = path.to_path_buf();
    move |e: std::io::Error| {
        let (code, clause) = match e.kind() {
            std::io::ErrorKind::PermissionDenied => (
                VaultFaultCode::Denied,
                "the system refused this account access".to_string(),
            ),
            std::io::ErrorKind::NotFound => {
                (VaultFaultCode::NotFound, "it doesn't exist".to_string())
            }
            _ => (VaultFaultCode::Other, e.to_string()),
        };
        let display = path.display();
        Error::Vault(VaultFault {
            code,
            op: op.to_string(),
            path: Some(display.to_string()),
            message: format!("PM couldn't {op} at {display}: {clause}"),
        })
    }
}

/// One error type for the whole backend. Implements `Serialize` so it can be
/// returned straight out of `#[tauri::command]` functions to the frontend.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("keychain error: {0}")]
    Keyring(#[from] keyring::Error),

    /// A transport failure. Deliberately **not** `#[from]`: the hand-written
    /// `From<reqwest::Error>` below is the one place a request URL is redacted, and no
    /// call site may format a raw `reqwest::Error` instead (see [`redact_url`]).
    #[error("network error: {0}")]
    Http(#[source] reqwest::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("archive error: {0}")]
    Zip(#[from] zip::result::ZipError),

    /// A classified vault-path failure (see [`VaultFault`]). Serializes as a flat JSON
    /// object so the frontend can branch on `code`; its Display stays the friendly
    /// sentence so a generic `String(e)` still renders cleanly.
    #[error("{}", .0.message)]
    Vault(VaultFault),

    /// A provider sign-in that only signing in again can fix: a refresh token Google refused, no
    /// token (or an unreadable one), or a token whose Cloud project PM no longer holds. Its own
    /// variant so a calendar save can say "reconnect" rather than "failed";
    /// it Displays and serialises exactly as the `Other` string it replaced.
    #[error("{0}")]
    Reauth(String),

    /// A cloud provider's call that failed in a way the caller can tell from the rest (see
    /// [`ProviderFailure`]). Boxed so the common `Result` stays small. It Displays and serialises as
    /// its sentence alone, exactly as the `Other(String)` it stands in for did, so the ~200
    /// `String(e)` sites and the webview's error shape do not change.
    #[error("{}", .0.message)]
    Provider(Box<ProviderFailure>),

    /// A list of models that ran out: every row was skipped or failed without billing, so nothing
    /// is left to try. `attempts` is what happened to each row in order and `last` is the error the
    /// last row that was tried ended on. It Displays as one plain sentence a background job can log
    /// and serialises as that sentence, like every other variant but `Vault`, so no webview shape
    /// changes.
    #[error("{}", exhausted_sentence(attempts))]
    Exhausted {
        attempts: Vec<Attempt>,
        last: Box<Error>,
    },

    #[error("{0}")]
    Other(String),
}

/// Why a provider's call failed, as a class the code can branch on (the sentence is `message`, for
/// the person). The class is WORDING and what a one-click offer is worth; whether anything else may
/// be tried after it is [`Billing`]'s call, never the class's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// Overloaded, a 5xx, "high demand".
    Busy,
    /// A per-minute (or daily allowance) rate limit on the key.
    RateLimited,
    /// The model is no longer offered to this key.
    ModelGone,
    /// The conversation is longer than the model can take.
    ContextTooLong,
    /// The provider does not accept this key (rejected, revoked, held, leaked, restricted).
    KeyBad,
    /// No credit left, or a spend cap or limit set at the provider.
    NoCredit,
    /// Anthropic's Fable on a workspace without 30-day retention.
    NeedsRetention,
    /// PM's own setup for the provider is out of date (an effort or output limit it refuses).
    SetupStale,
    /// PM's own brake: the person's daily limit for this provider has been reached. Nothing was sent.
    LimitReached,
    /// Paused where the person is (a free Gemini key in the UK, EEA or Switzerland; OpenAI's own
    /// region refusal).
    Paused,
    /// The model refused or withheld the reply (billed, and the same prompt would refuse again).
    Refused,
    /// A failure after the request left that PM cannot class further: a cut stream, a timeout after
    /// send, a mid-stream error with no code PM reads.
    StoppedPartway,
    /// Everything else; the sentence is the provider-mapped one.
    Other,
}

impl FailureClass {
    /// Every class, for the reverse lookup of a logged token ([`list_reason_words`]).
    pub const ALL: [FailureClass; 13] = [
        FailureClass::Busy,
        FailureClass::RateLimited,
        FailureClass::ModelGone,
        FailureClass::ContextTooLong,
        FailureClass::KeyBad,
        FailureClass::NoCredit,
        FailureClass::NeedsRetention,
        FailureClass::SetupStale,
        FailureClass::LimitReached,
        FailureClass::Paused,
        FailureClass::Refused,
        FailureClass::StoppedPartway,
        FailureClass::Other,
    ];

    /// The stable token the webview's copy keys on and a log line carries.
    pub fn as_str(self) -> &'static str {
        match self {
            FailureClass::Busy => "busy",
            FailureClass::RateLimited => "rate_limited",
            FailureClass::ModelGone => "model_gone",
            FailureClass::ContextTooLong => "context_too_long",
            FailureClass::KeyBad => "key_bad",
            FailureClass::NoCredit => "no_credit",
            FailureClass::NeedsRetention => "needs_retention",
            FailureClass::SetupStale => "setup_stale",
            FailureClass::LimitReached => "limit_reached",
            FailureClass::Paused => "paused",
            FailureClass::Refused => "refused",
            FailureClass::StoppedPartway => "stopped_partway",
            FailureClass::Other => "other",
        }
    }

    /// A failure about the KEY or the account as a whole, so another model on the same key meets
    /// the same wall. A model-level class (a model gone, a retention need, a stale setup) leaves the
    /// provider's other models worth offering.
    pub fn is_key_level(self) -> bool {
        matches!(
            self,
            FailureClass::KeyBad
                | FailureClass::NoCredit
                | FailureClass::LimitReached
                | FailureClass::Paused
        )
    }
}

impl FailureClass {
    /// The class as the end of "{Who} didn't answer ({why})": a fragment, not a sentence. The full
    /// sentences a card shows live in `offers::reason_line`; this is the one short form the list's
    /// own notices (the walk strip, `Error::Exhausted`, the log) share, so each class is worded once
    /// for them.
    pub fn why(self) -> &'static str {
        match self {
            FailureClass::Busy => "busy",
            FailureClass::RateLimited => "rate limiting this key",
            FailureClass::ModelGone => "no longer offers this model",
            FailureClass::ContextTooLong => "can't take this much text",
            FailureClass::KeyBad => "doesn't accept this key",
            FailureClass::NoCredit => "no credit left or at a spend limit",
            FailureClass::NeedsRetention => "needs data retention turned on for this model",
            FailureClass::SetupStale => "PM's settings for it are out of date",
            FailureClass::LimitReached => "paused by your daily limit",
            FailureClass::Paused => "paused where you are",
            FailureClass::Refused => "stopped this reply for safety reasons",
            FailureClass::StoppedPartway => "stopped partway",
            FailureClass::Other => "it failed",
        }
    }
}

/// Why a row of a person's list was passed over without a request being sent to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The provider's key is not on this device.
    NoKey,
    /// The key could not be read (the keychain is locked or broken).
    KeyUnreadable,
    /// The provider is not turned on in Your own keys.
    NotOnForYou,
    /// This computer has no model chosen for the role, or no server set.
    NoLocalModel,
    /// Paused where the person is (a free Gemini key in the UK, EEA or Switzerland).
    Paused,
    /// The person's daily limit has paused the provider.
    LimitReached,
    /// An earlier request failed and PM remembers it for a few minutes ([`crate::entry_health`]).
    Down(FailureClass),
    /// The conversation is longer than the model's window by more than the margin.
    TooLong,
    /// A this-computer row the walk reached while Local AI is off: the list stops here, so nothing
    /// after it is tried (Local AI off is never a back door to a cloud bill).
    LocalAiOff,
    /// A this-computer row the walk reached while the role waits for its model to be picked: the
    /// list stops here for the same reason.
    PickAModel,
}

impl SkipReason {
    /// A stable token for `usage_log.fallback_reason` (`list_skip_<token>`).
    pub fn as_str(self) -> &'static str {
        match self {
            SkipReason::NoKey => "no_key",
            SkipReason::KeyUnreadable => "key_unreadable",
            SkipReason::NotOnForYou => "not_on",
            SkipReason::NoLocalModel => "no_local_model",
            SkipReason::Paused => "paused",
            SkipReason::LimitReached => "limit_reached",
            SkipReason::Down(_) => "down",
            SkipReason::TooLong => "too_long",
            SkipReason::LocalAiOff => "local_ai_off",
            SkipReason::PickAModel => "pick_a_model",
        }
    }

    /// The reason as the end of "{Who} was skipped ({why})".
    pub fn why(self) -> &'static str {
        match self {
            SkipReason::NoKey => "no key saved",
            SkipReason::KeyUnreadable => "PM can't read the key",
            SkipReason::NotOnForYou => "turned off in Your own keys",
            SkipReason::NoLocalModel => "no model chosen on this computer",
            SkipReason::Paused => "paused where you are",
            SkipReason::LimitReached => "paused by your daily limit",
            SkipReason::Down(c) => c.why(),
            SkipReason::TooLong => "too long for its window",
            SkipReason::LocalAiOff => "Local AI is off",
            SkipReason::PickAModel => "no model is picked for Local AI yet",
        }
    }

    /// The sentence that ends a list the walk STOPPED at this row, when this reason stops the list
    /// (a this-computer row that cannot run for a reason PM will not route round). `None` for every
    /// reason that only passes a row over.
    pub fn stop_sentence(self) -> Option<&'static str> {
        match self {
            SkipReason::LocalAiOff => Some(
                "Local AI is off, so PM stopped there and didn't try the rest of your list.",
            ),
            SkipReason::PickAModel => Some(
                "No model is picked for Local AI, so PM stopped there and didn't try the rest of your list.",
            ),
            _ => None,
        }
    }
}

/// What happened to one row of a list during one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptOutcome {
    Skipped(SkipReason),
    Failed(FailureClass),
    /// This computer's own model failed (before the walk began, as a list's head), for a reason that
    /// has its own words ("your local model timed out"): the same words the local strip uses, so a
    /// Local row's step never reads as a company's class line ("it failed"). Counted as a failure
    /// everywhere a failed row is.
    LocalFailed(&'static str),
    Answered,
}

/// The row an [`Attempt`] was about: a company's model, or this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptEntry {
    /// `None` is this computer's model.
    pub provider: Option<crate::llm_gateway::CloudProvider>,
    pub model: Option<String>,
    /// This computer's row, when Local AI points at a server that is NOT this computer (the same
    /// loopback test the 4c card makes, `offers::local_company`). Always false for a company's row.
    /// The strip, the log sentences and the list chip then say "your Local AI server", never "this
    /// computer", because the text left the machine.
    pub local_elsewhere: bool,
}

impl AttemptEntry {
    /// Who a person calls it: "Anthropic", "Gemini", "this computer" ("your Local AI server" when
    /// Local AI is on another machine).
    pub fn who(&self) -> &'static str {
        match self.provider {
            Some(p) => crate::offers::short_name(p),
            None => crate::offers::local_company(!self.local_elsewhere),
        }
    }

    /// Who gets the request, as the card's data line says it ("Google", not "Gemini"): the company
    /// the strip names when a reply went somewhere else.
    pub fn company(&self) -> &'static str {
        match self.provider {
            Some(p) => crate::offers::company(p),
            None => crate::offers::local_company(!self.local_elsewhere),
        }
    }

    /// The model as a person reads it: a catalogue name for a direct model, otherwise the id the
    /// row holds (OpenRouter's and this computer's own), which the page trims to its last part.
    pub fn label(&self) -> String {
        match (&self.model, self.provider) {
            (Some(m), Some(p)) if p.is_direct() => crate::direct_models::find(m)
                .map(|d| d.name.clone())
                .unwrap_or_else(|| m.clone()),
            (Some(m), _) => m.clone(),
            (None, _) => capitalise(self.who()),
        }
    }
}

/// The words for a `usage_log.fallback_reason` that came from a list: `list_<class>` for a row that
/// failed and `list_skip_<reason>` for one passed over. `None` for any other value (a local
/// failure, a power-policy route, or nothing), so the Usage tab counts only a list's own walks. The
/// log keeps no class for a row PM passed over because it remembered it as down, so that one reads
/// as the remembered failure.
pub fn list_reason_words(slug: &str) -> Option<&'static str> {
    if let Some(token) = slug.strip_prefix("list_skip_") {
        return Some(match token {
            "no_key" => SkipReason::NoKey.why(),
            "key_unreadable" => SkipReason::KeyUnreadable.why(),
            "not_on" => SkipReason::NotOnForYou.why(),
            "no_local_model" => SkipReason::NoLocalModel.why(),
            "paused" => SkipReason::Paused.why(),
            "limit_reached" => SkipReason::LimitReached.why(),
            "too_long" => SkipReason::TooLong.why(),
            "local_ai_off" => SkipReason::LocalAiOff.why(),
            "pick_a_model" => SkipReason::PickAModel.why(),
            "down" => "an earlier request to it had failed",
            _ => return None,
        });
    }
    let token = slug.strip_prefix("list_")?;
    FailureClass::ALL
        .into_iter()
        .find(|c| c.as_str() == token)
        .map(FailureClass::why)
}

/// One row of the person's list and what became of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub entry: AttemptEntry,
    pub outcome: AttemptOutcome,
}

impl Attempt {
    /// "Anthropic didn't answer (no credit left or at a spend limit)." or "Gemini was skipped (paused
    /// where you are)." An answered row reads "Gemini answered.".
    pub fn sentence(&self) -> String {
        let who = capitalise(self.entry.who());
        match self.outcome {
            AttemptOutcome::Answered => format!("{who} answered."),
            AttemptOutcome::Failed(c) => format!("{who} didn't answer ({}).", c.why()),
            AttemptOutcome::LocalFailed(why) => format!("{who} didn't answer ({why})."),
            AttemptOutcome::Skipped(r) => format!("{who} was skipped ({}).", r.why()),
        }
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The sentence for a list that ran out, from what happened to each row in order. A list the walk
/// stopped at a this-computer row (Local AI off, or no model picked) ends by saying that, not that
/// nothing else is in the list: there is more, and PM did not try it.
pub fn exhausted_sentence(attempts: &[Attempt]) -> String {
    let mut out = attempts
        .iter()
        .map(Attempt::sentence)
        .collect::<Vec<_>>()
        .join(" ");
    if !out.is_empty() {
        out.push(' ');
    }
    out.push_str(stop_note(attempts).unwrap_or("Nothing else is in your list."));
    out
}

/// The sentence for the row a walk stopped at, when its last note is a skip that stops the list.
pub fn stop_note(attempts: &[Attempt]) -> Option<&'static str> {
    match attempts.last()?.outcome {
        AttemptOutcome::Skipped(r) => r.stop_sentence(),
        _ => None,
    }
}

/// Whether the failed call can have cost money. This is the gate on trying anything else after it
/// (a request bills at most one attempt), so a failure PM cannot prove unbilled never reads as one.
/// Only `NeverLeft` and `Rejected` may be followed by another attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Billing {
    /// PM refused before anything left this computer, or a connect failure proves nothing was sent.
    NeverLeft,
    /// An HTTP error RESPONSE received before any body streamed, with a status the vendors document
    /// as unbilled (see [`billing_for_status`]).
    Rejected,
    /// Anything else: a timeout after send, a stream that began, a mid-stream `error` event, a cut
    /// stream, a failure PM cannot classify. It may have billed, so nothing is tried after it.
    MaybeBilled,
}

impl Billing {
    /// Whether the call may have cost money.
    pub fn may_have_billed(self) -> bool {
        self == Billing::MaybeBilled
    }
}

/// The billing of a failed HTTP response, by status. PURE. Only the statuses the vendors document as
/// unbilled are `Rejected`: 400, 401, 402, 403, 404, 413, 422, 429, 503 and 529. 500, 502, 504, 408,
/// 524 and every other status are `MaybeBilled`, because a router or CDN timeout can arrive after the
/// upstream finished the work. Called ONLY at the sites that see the status of a response before any
/// body byte was read; a mid-stream error never reaches it.
pub fn billing_for_status(status: u16) -> Billing {
    match status {
        400 | 401 | 402 | 403 | 404 | 413 | 422 | 429 | 503 | 529 => Billing::Rejected,
        _ => Billing::MaybeBilled,
    }
}

/// A failed provider call: what kind, whether it could have billed, whose it was and what to say.
/// The webview never sees these fields; `Display` and `Serialize` are the message alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFailure {
    pub class: FailureClass,
    pub billing: Billing,
    pub provider: crate::llm_gateway::CloudProvider,
    /// The model the call was for, when the failure is about one.
    pub model: Option<String>,
    /// The sentence a person reads. Never carries a key or a provider's own wording of the failure.
    pub message: String,
}

impl Error {
    /// A typed provider failure. Billing starts at `MaybeBilled`, the safe default: it is lowered
    /// only by the one place that can prove otherwise (an HTTP error response before any body byte,
    /// or the attempt's own wire mark), never by the adapter that merely classifies the words.
    pub fn provider(
        provider: crate::llm_gateway::CloudProvider,
        class: FailureClass,
        message: String,
    ) -> Error {
        Error::Provider(Box::new(ProviderFailure {
            class,
            billing: Billing::MaybeBilled,
            provider,
            model: None,
            message,
        }))
    }

    /// A typed failure PM itself decided before anything was sent (a paused region, a free allowance
    /// it knows is spent): `NeverLeft`, which is true by construction.
    pub fn refused_before_send(
        provider: crate::llm_gateway::CloudProvider,
        class: FailureClass,
        message: String,
    ) -> Error {
        Error::Provider(Box::new(ProviderFailure {
            class,
            billing: Billing::NeverLeft,
            provider,
            model: None,
            message,
        }))
    }

    /// This error, told it came back as an HTTP error response with `status` before any body was
    /// read. A typed failure still at the `MaybeBilled` default takes the status's billing; anything
    /// else is returned as it was.
    pub fn with_response_status(self, status: u16) -> Error {
        match self {
            Error::Provider(mut f) if f.billing == Billing::MaybeBilled => {
                f.billing = billing_for_status(status);
                Error::Provider(f)
            }
            other => other,
        }
    }

    /// The typed provider failure inside this error, if it is one.
    pub fn provider_failure(&self) -> Option<&ProviderFailure> {
        match self {
            Error::Provider(f) => Some(f),
            _ => None,
        }
    }
}

impl Error {
    /// Whether this error is an OS access-denial — a raw io `PermissionDenied` or an
    /// already-classified Denied fault. The lock watcher uses it to tell a broken-ACL
    /// vault folder (persistent, worth a fault banner) from transient tick noise.
    pub fn is_denied(&self) -> bool {
        match self {
            Error::Io(e) => e.kind() == std::io::ErrorKind::PermissionDenied,
            Error::Vault(f) => f.code == VaultFaultCode::Denied,
            _ => false,
        }
    }
}

/// Reduce a URL to the only part that is safe to show or log: scheme + host (+ a
/// non-default port). A URL can BE the credential — a private iCal feed link
/// (`/published/2/<token>`), a Drive resumable-upload session URI (`upload_id=…`), a Graph
/// delta cursor (`token=…`) — with the secret in the path or the query, and `url::Url`'s
/// Display prints userinfo verbatim on top of that. Everything after the authority is
/// dropped. Mirrors `calendar::default_label`, which already treats the host as a feed
/// URL's public projection.
pub fn redact_url(url: &reqwest::Url) -> String {
    // `Url::port()` is `None` for a scheme's default port, so `:443` never shows up and a
    // deliberate `:8443` does.
    match url.port() {
        Some(p) => format!("{}://{}:{p}", url.scheme(), url.host_str().unwrap_or("?")),
        None => format!("{}://{}", url.scheme(), url.host_str().unwrap_or("?")),
    }
}

impl From<reqwest::Error> for Error {
    /// The ONE conversion every `?` on a `reqwest::Error` in the backend passes through.
    /// reqwest appends `" for url ({url})"` to its own Display verbatim, so redacting the
    /// attached URL here covers every current *and* future network call site at a stroke —
    /// a per-call-site fix would leave the class open.
    fn from(e: reqwest::Error) -> Self {
        let Some(safe) = e.url().map(redact_url) else {
            return Error::Http(e);
        };
        Error::Http(match reqwest::Url::parse(&safe) {
            Ok(u) => e.without_url().with_url(u),
            // Never panic inside an error path: an unparseable authority just loses the host.
            Err(_) => e.without_url(),
        })
    }
}

impl Serialize for Error {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            // The single structured rejection: {"code","op","path","message"}. Everything
            // else keeps the historical bare-string shape (~200 String(e) call sites).
            Error::Vault(fault) => fault.serialize(serializer),
            other => serializer.serialize_str(&other.to_string()),
        }
    }
}

/// Trim an upstream/provider response body before folding it into an error that
/// reaches the webview or logs. Provider error bodies can be long and carry
/// incidental detail (absolute paths, request echoes); keep just enough to
/// diagnose. No key/token is ever interpolated into these (invariant #5 holds).
pub fn truncate_detail(detail: &str) -> String {
    const MAX: usize = 500;
    let detail = detail.trim();
    if detail.chars().count() <= MAX {
        return detail.to_string();
    }
    let head: String = detail.chars().take(MAX).collect();
    format!("{head}… (truncated)")
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every token a list writes to `usage_log.fallback_reason` reads back as the SAME words the
    /// strip and `Error::Exhausted` use, and nothing else is taken for one.
    #[test]
    fn a_logged_list_reason_reads_back_as_its_class_words() {
        for c in FailureClass::ALL {
            assert_eq!(
                list_reason_words(&format!("list_{}", c.as_str())),
                Some(c.why())
            );
        }
        for (token, r) in [
            ("no_key", SkipReason::NoKey),
            ("key_unreadable", SkipReason::KeyUnreadable),
            ("not_on", SkipReason::NotOnForYou),
            ("no_local_model", SkipReason::NoLocalModel),
            ("paused", SkipReason::Paused),
            ("limit_reached", SkipReason::LimitReached),
            ("too_long", SkipReason::TooLong),
        ] {
            assert_eq!(r.as_str(), token);
            assert_eq!(
                list_reason_words(&format!("list_skip_{token}")),
                Some(r.why())
            );
        }
        assert!(list_reason_words("list_skip_down").is_some());
        for other in [
            "cooldown",
            "power_policy",
            "hard_failure:timeout",
            "list_",
            "list_nonsense",
            "list_skip_nonsense",
            "",
        ] {
            assert_eq!(list_reason_words(other), None, "{other}");
        }
    }

    #[test]
    fn truncate_detail_keeps_short_and_cuts_long() {
        assert_eq!(truncate_detail("  short body  "), "short body");
        let long = "y".repeat(2000);
        let out = truncate_detail(&long);
        assert!(out.ends_with("… (truncated)"));
        assert_eq!(out.chars().filter(|c| *c == 'y').count(), 500);
    }

    fn fault_of(e: Error) -> VaultFault {
        match e {
            Error::Vault(f) => f,
            other => panic!("expected Error::Vault, got: {other}"),
        }
    }

    #[test]
    fn io_at_classifies_by_error_kind() {
        let path = Path::new("C:/shared/vault-meta.json");
        let denied = fault_of(io_at("read the vault's settings", path)(
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        ));
        assert_eq!(denied.code, VaultFaultCode::Denied);
        assert!(denied.message.contains("read the vault's settings"));
        assert!(denied.message.contains("vault-meta.json"));

        let missing = fault_of(io_at("read the vault's settings", path)(
            std::io::Error::from(std::io::ErrorKind::NotFound),
        ));
        assert_eq!(missing.code, VaultFaultCode::NotFound);

        let other = fault_of(io_at("read the vault's settings", path)(
            std::io::Error::other("disk exploded"),
        ));
        assert_eq!(other.code, VaultFaultCode::Other);
        assert!(other.message.contains("disk exploded"));
    }

    #[test]
    fn vault_error_serializes_as_flat_object_others_as_strings() {
        let fault = VaultFault {
            code: VaultFaultCode::Denied,
            op: "read the vault's settings".into(),
            path: Some("C:/shared".into()),
            message: "PM couldn't read the vault's settings".into(),
        };
        let v = serde_json::to_value(Error::Vault(fault)).unwrap();
        assert_eq!(v["code"], "denied");
        assert_eq!(v["path"], "C:/shared");
        assert!(v["message"].as_str().unwrap().contains("couldn't"));
        // Every other variant keeps the historical bare-string rejection shape.
        let s = serde_json::to_value(Error::Other("plain".into())).unwrap();
        assert_eq!(s, serde_json::json!("plain"));
        // The wire name the open-error screen branches on to withhold "Start fresh" (types.ts).
        assert_eq!(
            serde_json::to_value(VaultFaultCode::NewerStore).unwrap(),
            "newer-store"
        );
    }

    /// The typed provider failure reads and travels as its sentence alone: the ~200 `String(e)` sites
    /// and the webview's rejection shape (a bare string) must not notice the new variant.
    #[test]
    fn a_provider_failure_displays_and_serialises_as_its_sentence() {
        let e = Error::Provider(Box::new(ProviderFailure {
            class: FailureClass::LimitReached,
            billing: Billing::NeverLeft,
            provider: crate::llm_gateway::CloudProvider::Anthropic,
            model: Some("anthropic/claude-sonnet-5-5".into()),
            message: "Anthropic is paused.".into(),
        }));
        assert_eq!(e.to_string(), "Anthropic is paused.");
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!("Anthropic is paused.")
        );
        let f = e.provider_failure().expect("typed");
        assert_eq!(f.class, FailureClass::LimitReached);
        assert_eq!(f.billing, Billing::NeverLeft);
        assert_eq!(f.model.as_deref(), Some("anthropic/claude-sonnet-5-5"));
        // Anything else is not one.
        assert!(Error::Other("x".into()).provider_failure().is_none());
    }

    /// Which statuses are proven unbilled: exactly the vendors' documented ones. A router or CDN
    /// timeout (504, 524, 408) and a plain 500 or 502 can arrive after the upstream finished, so they
    /// stay `MaybeBilled`. This is the table the billing property rests on.
    #[test]
    fn only_the_documented_unbilled_statuses_are_rejected() {
        for s in [400u16, 401, 402, 403, 404, 413, 422, 429, 503, 529] {
            assert_eq!(billing_for_status(s), Billing::Rejected, "{s}");
        }
        for s in [
            200u16, 201, 300, 405, 408, 409, 418, 500, 501, 502, 504, 520, 524, 599,
        ] {
            assert_eq!(billing_for_status(s), Billing::MaybeBilled, "{s}");
        }
    }

    /// A status only ever LOWERS a typed failure from its safe default, and only once: a failure
    /// that was already proven (PM's own refusal) or is not typed is left as it was.
    #[test]
    fn a_response_status_lowers_only_the_default() {
        let typed = |billing| {
            Error::Provider(Box::new(ProviderFailure {
                class: FailureClass::Busy,
                billing,
                provider: crate::llm_gateway::CloudProvider::OpenAi,
                model: None,
                message: "x".into(),
            }))
        };
        let b = |e: Error| e.provider_failure().unwrap().billing;
        assert_eq!(
            b(typed(Billing::MaybeBilled).with_response_status(429)),
            Billing::Rejected
        );
        assert_eq!(
            b(typed(Billing::MaybeBilled).with_response_status(502)),
            Billing::MaybeBilled
        );
        // A refusal PM made itself keeps its proof; a status never raises it.
        assert_eq!(
            b(typed(Billing::NeverLeft).with_response_status(502)),
            Billing::NeverLeft
        );
        assert!(Error::Other("x".into())
            .with_response_status(429)
            .provider_failure()
            .is_none());
    }

    /// The constructor defaults to the safe value: nothing is "unbilled" until something proves it.
    #[test]
    fn a_new_typed_failure_may_have_billed() {
        let e = Error::provider(
            crate::llm_gateway::CloudProvider::Gemini,
            FailureClass::Busy,
            "busy".into(),
        );
        let f = e.provider_failure().unwrap();
        assert_eq!(f.billing, Billing::MaybeBilled);
        assert!(f.billing.may_have_billed());
        assert!(!Billing::NeverLeft.may_have_billed());
        assert!(!Billing::Rejected.may_have_billed());
    }

    #[test]
    fn redact_url_keeps_only_scheme_host_and_port() {
        let url = reqwest::Url::parse(
            "https://user:pw@p61-caldav.icloud.com:8443/published/2/SECRET-TOKEN?upload_id=ALSO-SECRET#frag",
        )
        .unwrap();
        let out = redact_url(&url);
        assert_eq!(out, "https://p61-caldav.icloud.com:8443");
        // The whole point: none of the bearer material survives.
        for leaked in [
            "SECRET-TOKEN",
            "ALSO-SECRET",
            "upload_id",
            "pw",
            "published",
        ] {
            assert!(!out.contains(leaked), "redact_url leaked {leaked}: {out}");
        }
        // A default port is not noise the user needs; a deliberate one is.
        let plain = reqwest::Url::parse("https://graph.microsoft.com:443/v1.0/me?token=X").unwrap();
        assert_eq!(redact_url(&plain), "https://graph.microsoft.com");
    }

    /// A deterministic offline transport failure: nothing listens on loopback port 1, so this
    /// needs no DNS and no network. Exercises the CONVERSION, not just the helper — an
    /// `error.rs` change that fixes only `redact_url` would still leak through `?`.
    fn offline_send_error() -> reqwest::Error {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_millis(200))
            .build()
            .expect("blocking client builds")
            .get("https://127.0.0.1:1/published/2/SECRET-TOKEN?upload_id=ALSO-SECRET")
            .send()
            .expect_err("nothing listens on loopback port 1")
    }

    #[test]
    fn http_error_display_keeps_the_host_and_drops_the_secret() {
        let raw = offline_send_error();
        // Guard the fixture: if reqwest ever stops attaching the URL, the test would pass
        // vacuously and the redaction would go unexercised.
        assert!(
            raw.to_string().contains("SECRET-TOKEN"),
            "fixture no longer carries a URL, so it proves nothing: {raw}"
        );
        let msg = Error::from(raw).to_string();
        assert!(msg.contains("127.0.0.1"), "host should survive: {msg}");
        assert!(!msg.contains("SECRET-TOKEN"), "path leaked: {msg}");
        assert!(!msg.contains("upload_id"), "query leaked: {msg}");
    }

    #[test]
    fn http_error_serializes_to_the_webview_without_the_url_secret() {
        // The literal rejection payload the webview receives for a network failure.
        let v = serde_json::to_value(Error::from(offline_send_error())).unwrap();
        let s = v.as_str().expect("non-Vault variants stay bare strings");
        assert!(s.contains("127.0.0.1"), "host should survive: {s}");
        assert!(!s.contains("SECRET-TOKEN"), "path leaked: {s}");
        assert!(!s.contains("upload_id"), "query leaked: {s}");
    }

    #[test]
    fn from_error_passes_faults_through_and_folds_the_rest() {
        let fault = VaultFault {
            code: VaultFaultCode::WrongPassphrase,
            op: "unlock the vault".into(),
            path: None,
            message: "That passphrase doesn't match this vault.".into(),
        };
        let through = VaultFault::from_error("open the vault", &Error::Vault(fault.clone()));
        assert_eq!(through, fault);
        let folded = VaultFault::from_error("open the vault", &Error::Other("boom".into()));
        assert_eq!(folded.code, VaultFaultCode::Other);
        assert_eq!(folded.op, "open the vault");
        assert_eq!(folded.message, "boom");
    }

    fn attempt(
        provider: Option<crate::llm_gateway::CloudProvider>,
        outcome: AttemptOutcome,
    ) -> Attempt {
        Attempt {
            entry: AttemptEntry {
                provider,
                model: None,
                local_elsewhere: false,
            },
            outcome,
        }
    }

    /// A Local row on another machine is "your Local AI server" in every sentence (the log, the
    /// strip, the card share `offers::local_company`); a loopback one stays "this computer".
    #[test]
    fn a_local_row_on_another_machine_is_named_for_what_it_is() {
        let here = attempt(None, AttemptOutcome::Failed(FailureClass::Other));
        assert_eq!(here.entry.who(), "this computer");
        assert_eq!(here.entry.company(), "this computer");
        assert_eq!(here.sentence(), "This computer didn't answer (it failed).");
        let mut away = attempt(None, AttemptOutcome::Failed(FailureClass::Other));
        away.entry.local_elsewhere = true;
        assert_eq!(away.entry.who(), "your Local AI server");
        assert_eq!(away.entry.company(), "your Local AI server");
        assert_eq!(
            away.sentence(),
            "Your Local AI server didn't answer (it failed)."
        );
        // The flag means nothing for a company's row.
        let mut cloud = attempt(
            Some(crate::llm_gateway::CloudProvider::Gemini),
            AttemptOutcome::Answered,
        );
        cloud.entry.local_elsewhere = true;
        assert_eq!(cloud.entry.who(), "Gemini");
    }

    /// A list that ran out reads as one plain sentence a background job can log, row by row, ending
    /// "Nothing else is in your list.", and serialises as that sentence like every other variant.
    #[test]
    fn an_exhausted_list_is_one_plain_sentence_and_serialises_as_it() {
        use crate::llm_gateway::CloudProvider::{Anthropic, Gemini};
        let e = Error::Exhausted {
            attempts: vec![
                attempt(
                    Some(Anthropic),
                    AttemptOutcome::Failed(FailureClass::NoCredit),
                ),
                attempt(Some(Gemini), AttemptOutcome::Skipped(SkipReason::Paused)),
                attempt(None, AttemptOutcome::Failed(FailureClass::Busy)),
            ],
            last: Box::new(Error::Other("busy".into())),
        };
        let want = "Anthropic didn't answer (no credit left or at a spend limit). \
                    Gemini was skipped (paused where you are). \
                    This computer didn't answer (busy). Nothing else is in your list.";
        assert_eq!(e.to_string(), want);
        assert_eq!(serde_json::to_value(&e).unwrap(), serde_json::json!(want));
        // An empty list of attempts still says something true.
        assert_eq!(exhausted_sentence(&[]), "Nothing else is in your list.");
        assert_eq!(
            attempt(Some(Gemini), AttemptOutcome::Answered).sentence(),
            "Gemini answered."
        );
    }

    /// Every class and every skip reason has a fragment, and the tokens the log keeps are the
    /// stable ones the usage column reads.
    #[test]
    fn every_class_and_skip_reason_has_words_and_a_stable_token() {
        for class in [
            FailureClass::Busy,
            FailureClass::RateLimited,
            FailureClass::ModelGone,
            FailureClass::ContextTooLong,
            FailureClass::KeyBad,
            FailureClass::NoCredit,
            FailureClass::NeedsRetention,
            FailureClass::SetupStale,
            FailureClass::LimitReached,
            FailureClass::Paused,
            FailureClass::Refused,
            FailureClass::StoppedPartway,
            FailureClass::Other,
        ] {
            assert!(!class.why().is_empty(), "{class:?}");
            assert!(!class.as_str().is_empty());
        }
        for (why, token) in [
            (SkipReason::NoKey, "no_key"),
            (SkipReason::KeyUnreadable, "key_unreadable"),
            (SkipReason::NotOnForYou, "not_on"),
            (SkipReason::NoLocalModel, "no_local_model"),
            (SkipReason::Paused, "paused"),
            (SkipReason::LimitReached, "limit_reached"),
            (SkipReason::Down(FailureClass::Busy), "down"),
            (SkipReason::TooLong, "too_long"),
            (SkipReason::LocalAiOff, "local_ai_off"),
            (SkipReason::PickAModel, "pick_a_model"),
        ] {
            assert_eq!(why.as_str(), token);
            assert!(!why.why().is_empty());
            // The log's words for a token read back the same, so the Usage tab can say them.
            let expected = if token == "down" {
                "an earlier request to it had failed"
            } else {
                why.why()
            };
            assert_eq!(
                list_reason_words(&format!("list_skip_{token}")),
                Some(expected),
                "{token}"
            );
        }
        // Only a this-computer row that cannot run ends the list, and says so; a row merely passed
        // over never does.
        assert!(SkipReason::LocalAiOff
            .stop_sentence()
            .unwrap()
            .starts_with("Local AI is off"));
        assert!(SkipReason::PickAModel.stop_sentence().is_some());
        for passed_over in [
            SkipReason::NoKey,
            SkipReason::NoLocalModel,
            SkipReason::Paused,
        ] {
            assert_eq!(passed_over.stop_sentence(), None);
        }
        let stopped = [Attempt {
            entry: AttemptEntry {
                provider: None,
                model: None,
                local_elsewhere: false,
            },
            outcome: AttemptOutcome::Skipped(SkipReason::LocalAiOff),
        }];
        assert!(exhausted_sentence(&stopped).ends_with("didn't try the rest of your list."));
        assert!(!exhausted_sentence(&stopped).contains("Nothing else is in your list"));
        // A remembered-down row says why it is down.
        assert_eq!(
            SkipReason::Down(FailureClass::NoCredit).why(),
            FailureClass::NoCredit.why()
        );
    }
}
