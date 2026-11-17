// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a Mods tab command answers when it refuses: a stable `code` the page can branch on (to draw
//! a button, a retry, or nothing) and a `message` in words a person reads as it is. The wire shape
//! is `{code, message}` (kebab-case codes), deliberately without the `op` field `VaultFault` has, so
//! `ipc.ts` passes it through untouched and `modsErrorOf` reads it.
//!
//! Every refusal of the ledger, the verifier, the fetcher and the grant rules maps to one code here
//! ([`ModsError::from`]); nothing reaches the page as a bare database or file error.

use serde::Serialize;

use crate::mods::api::grants::GrantError;
use crate::mods::fetch::FetchError;
use crate::mods::ledger::{ChangeRefusal, LedgerError};
use crate::mods::package::{PackageError, Refusal as PackageRefusal};
use crate::mods::policy::Refusal;
use crate::mods::secrets;

/// The closed set of reasons a command refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Code {
    /// The command came from a window other than the main one.
    NotMainWindow,
    /// No store is open: the vault is locked, or another PM window holds it as the writer.
    NoStore,
    /// PM's app lock is on and not yet unlocked.
    Locked,
    NotInstalled,
    /// The catalogue doesn't list the mod, or the list can't be used (none yet, or out of date).
    NotInCatalogue,
    /// The request itself is wrong: a permission the mod didn't declare, an answer left out.
    BadRequest,
    /// The consent screen's token is unknown, used or too old: start the install again.
    ConsentExpired,
    FileTooLarge,
    FileUnreadable,
    /// The file isn't a `.pmmod` PM trusts (the verifier's refusal).
    NotAMod,
    /// A `.pmmod` with no signature at all: only an unsigned install (developer mode) takes it.
    Unsigned,
    /// The signed safety list, or the catalogue it carries, says no.
    SafetyList,
    NeedsNewerPm,
    ApiRetired,
    /// Another install rule (publisher, version, a removed mod) says no.
    Refused,
    /// A repair of a mod that doesn't come from the catalogue: install its file again.
    NotFromCatalogue,
    /// The store or the row changed while the command ran. Nothing changed.
    Changed,
    /// A rule that holds whatever the user says (unsigned mods read XOR internet, the sealed
    /// engine) refuses the grant.
    GrantRefused,
    /// A pause, a resume or a "Turn on" that isn't allowed now.
    NotAllowed,
    /// No answer from the network, or the server had no such file.
    Offline,
    /// A download PM won't accept (wrong file, too large, a redirect it won't follow).
    DownloadRefused,
    /// An unsigned file while developer mode is off for this device. Read by PM itself, not
    /// something the page can claim.
    DevModeOff,
    /// The stark screen's "type the mod's id" didn't match.
    ConfirmMismatch,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModsError {
    pub code: Code,
    pub message: String,
}

pub type Result<T> = std::result::Result<T, ModsError>;

impl ModsError {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(Code::BadRequest, message)
    }

    /// A failure PM didn't expect: logged in full, told in one fixed sentence (a database or file
    /// error's text is for the log, not for the page).
    pub fn internal(e: impl std::fmt::Display) -> Self {
        eprintln!("mods: a command failed ({e})");
        Self::new(
            Code::Internal,
            "Something went wrong inside PM. Nothing changed.",
        )
    }

    pub fn not_installed() -> Self {
        Self::new(Code::NotInstalled, "That mod isn't installed.")
    }

    pub fn consent_expired() -> Self {
        Self::new(
            Code::ConsentExpired,
            "That install screen is out of date. Start the install again.",
        )
    }

    pub fn no_store() -> Self {
        Self::new(
            Code::NoStore,
            "PM's data isn't open on this computer right now: it is locked, or another PM window \
             on it is in charge.",
        )
    }
}

impl std::fmt::Display for ModsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ModsError {}

impl From<Refusal> for ModsError {
    fn from(r: Refusal) -> Self {
        let message = format!("This mod wasn't installed: {r}.");
        let code = match &r {
            Refusal::NeedsNewerPm { .. } => Code::NeedsNewerPm,
            Refusal::ApiRetired => Code::ApiRetired,
            Refusal::Catalogue(_) => Code::SafetyList,
            Refusal::Changed => Code::Changed,
            Refusal::DevModeOff => Code::DevModeOff,
            _ => Code::Refused,
        };
        // The catalogue's own words already read as a sentence of their own.
        match r {
            Refusal::Catalogue(why) => {
                Self::new(code, format!("This mod wasn't installed: {why}."))
            }
            _ => Self::new(code, message),
        }
    }
}

impl From<PackageError> for ModsError {
    fn from(e: PackageError) -> Self {
        let code = match e.refusal {
            PackageRefusal::TooLarge => Code::FileTooLarge,
            PackageRefusal::Unsigned => Code::Unsigned,
            _ => Code::NotAMod,
        };
        Self::new(code, e.to_string())
    }
}

impl From<ChangeRefusal> for ModsError {
    fn from(r: ChangeRefusal) -> Self {
        let code = match r {
            ChangeRefusal::Changed => Code::Changed,
            _ => Code::NotAllowed,
        };
        Self::new(code, r.to_string())
    }
}

impl From<GrantError> for ModsError {
    fn from(e: GrantError) -> Self {
        match e {
            GrantError::NotInstalled => Self::not_installed(),
            GrantError::Unknown(_) | GrantError::Undeclared(_) => Self::bad_request(e.to_string()),
            GrantError::Refused(..) => Self::new(Code::GrantRefused, e.to_string()),
            GrantError::Db(_) => Self::internal(e),
        }
    }
}

impl From<LedgerError> for ModsError {
    fn from(e: LedgerError) -> Self {
        match e {
            LedgerError::Package(p) => p.into(),
            LedgerError::Refused(r) => r.into(),
            LedgerError::Store(_) => Self::no_store(),
            LedgerError::VaultChanged => Self::new(Code::Changed, e.to_string()),
            LedgerError::NotInstalled => Self::not_installed(),
            LedgerError::Change(c) => c.into(),
            LedgerError::Grants(g) => g.into(),
            LedgerError::Disk(_) | LedgerError::Db(_) => Self::internal(&e),
        }
    }
}

/// A refused key command, in fixed sentences. The typed text is never in one, and neither is a
/// database error's (`internal` logs that, and a store error does not carry parameter values).
impl From<secrets::Refused> for ModsError {
    fn from(r: secrets::Refused) -> Self {
        match r {
            secrets::Refused::NotInstalled => Self::not_installed(),
            secrets::Refused::NoSuchSlot => Self::bad_request("That mod has no key by that name."),
            secrets::Refused::Unusable => Self::bad_request(
                "PM can't send that key. It may be too long, or hold a line break or a character \
                 a request can't carry.",
            ),
            secrets::Refused::Db(e) => Self::internal(e),
        }
    }
}

impl From<FetchError> for ModsError {
    fn from(e: FetchError) -> Self {
        match e {
            FetchError::Offline(_) | FetchError::Status(_) => Self::new(
                Code::Offline,
                "PM couldn't reach the mod's download. Check your connection and try again.",
            ),
            FetchError::TooLarge
            | FetchError::Redirect(_)
            | FetchError::NotPinned(_)
            | FetchError::ShaMismatch { .. } => Self::new(
                Code::DownloadRefused,
                format!("PM didn't accept that download: {e}."),
            ),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::mods::disk::PathRefusal;

    fn code_of(e: impl Into<ModsError>) -> Code {
        e.into().code
    }

    #[test]
    fn every_ledger_refusal_has_a_code_the_page_can_branch_on() {
        assert_eq!(code_of(LedgerError::NotInstalled), Code::NotInstalled);
        assert_eq!(code_of(LedgerError::Store("locked".into())), Code::NoStore);
        assert_eq!(code_of(LedgerError::VaultChanged), Code::Changed);
        assert_eq!(
            code_of(LedgerError::Refused(Refusal::NeedsNewerPm { min: (2, 0) })),
            Code::NeedsNewerPm
        );
        assert_eq!(
            code_of(LedgerError::Refused(Refusal::ApiRetired)),
            Code::ApiRetired
        );
        assert_eq!(
            code_of(LedgerError::Refused(Refusal::Catalogue(
                "on the list".into()
            ))),
            Code::SafetyList
        );
        assert_eq!(
            code_of(LedgerError::Refused(Refusal::Changed)),
            Code::Changed
        );
        assert_eq!(
            code_of(LedgerError::Refused(Refusal::DevModeOff)),
            Code::DevModeOff
        );
        for r in [
            Refusal::Publisher,
            Refusal::Downgrade {
                installed: "2.0.0".into(),
            },
            Refusal::SameVersionDifferentFile,
            Refusal::UnreadableRecord,
            Refusal::RemovedPending,
            Refusal::NothingToUpdate,
            Refusal::OtherInstallHoldsId,
            Refusal::MakerChanged,
            Refusal::Tier("nope".into()),
            Refusal::Path(PathRefusal::Version("x".into())),
        ] {
            assert_eq!(code_of(LedgerError::Refused(r)), Code::Refused);
        }
        assert_eq!(
            code_of(LedgerError::Change(ChangeRefusal::Changed)),
            Code::Changed
        );
        assert_eq!(
            code_of(LedgerError::Change(ChangeRefusal::AlreadyOn)),
            Code::NotAllowed
        );
    }

    #[test]
    fn a_database_or_file_failure_is_one_fixed_sentence() {
        let e: ModsError = LedgerError::Disk(std::io::Error::other("/home/x/secret path")).into();
        assert_eq!(e.code, Code::Internal);
        assert!(!e.message.contains("secret"), "{}", e.message);
        let e: ModsError = GrantError::Db("table mod_grants has no column".into()).into();
        assert_eq!(e.code, Code::Internal);
        assert!(!e.message.contains("column"), "{}", e.message);
    }

    #[test]
    fn a_refused_key_command_is_a_fixed_sentence_that_quotes_nothing() {
        assert_eq!(code_of(secrets::Refused::NotInstalled), Code::NotInstalled);
        for r in [secrets::Refused::NoSuchSlot, secrets::Refused::Unusable] {
            assert_eq!(code_of(r), Code::BadRequest);
        }
        // A store error is told in the one fixed sentence, whatever it said.
        let e: ModsError = secrets::Refused::Db(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(19),
            Some("UNIQUE constraint failed: mod_secrets.value = typed-text".into()),
        ))
        .into();
        assert_eq!(e.code, Code::Internal);
        assert!(!e.message.contains("typed-text"), "{}", e.message);
    }

    #[test]
    fn grants_and_downloads_and_the_verifier_are_told_apart() {
        assert_eq!(
            code_of(GrantError::Refused(
                "internet".into(),
                crate::mods::api::grants::Refused::Exclusion
            )),
            Code::GrantRefused
        );
        assert_eq!(
            code_of(GrantError::Undeclared("ai".into())),
            Code::BadRequest
        );
        assert_eq!(code_of(FetchError::Offline("dns".into())), Code::Offline);
        assert_eq!(code_of(FetchError::Status(404)), Code::Offline);
        assert_eq!(code_of(FetchError::TooLarge), Code::DownloadRefused);
        assert_eq!(
            code_of(FetchError::ShaMismatch {
                want: "a".into(),
                got: "b".into()
            }),
            Code::DownloadRefused
        );
        assert_eq!(
            code_of(PackageError {
                refusal: PackageRefusal::TooLarge,
                detail: "the file is too large".into()
            }),
            Code::FileTooLarge
        );
        assert_eq!(
            code_of(PackageError {
                refusal: PackageRefusal::Unsigned,
                detail: "it isn't signed".into()
            }),
            Code::Unsigned
        );
        assert_eq!(
            code_of(PackageError {
                refusal: PackageRefusal::Missing,
                detail: "it has no checksums".into()
            }),
            Code::NotAMod
        );
    }

    /// Every code, with a match that stops compiling when one is added without being listed here.
    fn every_code() -> Vec<Code> {
        let all = vec![
            Code::NotMainWindow,
            Code::NoStore,
            Code::Locked,
            Code::NotInstalled,
            Code::NotInCatalogue,
            Code::BadRequest,
            Code::ConsentExpired,
            Code::FileTooLarge,
            Code::FileUnreadable,
            Code::NotAMod,
            Code::Unsigned,
            Code::SafetyList,
            Code::NeedsNewerPm,
            Code::ApiRetired,
            Code::Refused,
            Code::NotFromCatalogue,
            Code::Changed,
            Code::GrantRefused,
            Code::NotAllowed,
            Code::Offline,
            Code::DownloadRefused,
            Code::DevModeOff,
            Code::ConfirmMismatch,
            Code::Internal,
        ];
        for c in &all {
            match c {
                Code::NotMainWindow
                | Code::NoStore
                | Code::Locked
                | Code::NotInstalled
                | Code::NotInCatalogue
                | Code::BadRequest
                | Code::ConsentExpired
                | Code::FileTooLarge
                | Code::FileUnreadable
                | Code::NotAMod
                | Code::Unsigned
                | Code::SafetyList
                | Code::NeedsNewerPm
                | Code::ApiRetired
                | Code::Refused
                | Code::NotFromCatalogue
                | Code::Changed
                | Code::GrantRefused
                | Code::NotAllowed
                | Code::Offline
                | Code::DownloadRefused
                | Code::DevModeOff
                | Code::ConfirmMismatch
                | Code::Internal => {}
            }
        }
        all
    }

    /// The quoted words of the TS union `name` in `src/lib/modsTypes.ts`.
    pub(crate) fn ts_union(name: &str) -> Vec<String> {
        let source = include_str!("../../../../src/lib/modsTypes.ts");
        let start = source
            .find(&format!("export type {name} ="))
            .unwrap_or_else(|| panic!("{name} isn't in modsTypes.ts"));
        let body = &source[start..];
        let body = &body[..body.find(";\n").unwrap()];
        body.split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn the_pages_error_codes_are_the_backends_exactly() {
        let mut rust: Vec<String> = every_code()
            .into_iter()
            .map(|c| {
                serde_json::to_value(c)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        let mut ts = ts_union("ModsErrorCode");
        rust.sort();
        ts.sort();
        assert_eq!(
            rust, ts,
            "src/lib/modsTypes.ts's ModsErrorCode and Code differ"
        );
        assert!(!rust.is_empty() && rust.windows(2).all(|w| w[0] != w[1]));
    }

    #[test]
    fn the_wire_shape_is_a_kebab_code_and_a_message_only() {
        let e = ModsError::new(Code::NotMainWindow, "x");
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({"code": "not-main-window", "message": "x"})
        );
    }
}
