// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The registered handlers: one [`Method`] per live call row in the table (a test holds the two
//! together), and the step that runs the one a call names. A handler gets a call that passed every
//! check, and the arguments already parsed as JSON; it validates their shape itself, and touches
//! data only through [`super::recheck`] under the guard it uses.

use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{storage, Authorised, Host, ModError, PageAction, Reply};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    ThemeGet,
    StorageGet,
    StorageSet,
}

impl Method {
    pub const ALL: [Method; 3] = [Self::ThemeGet, Self::StorageGet, Self::StorageSet];

    pub fn name(self) -> &'static str {
        match self {
            Self::ThemeGet => "theme.get",
            Self::StorageGet => "storage.get",
            Self::StorageSet => "storage.set",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == name)
    }
}

/// Run the handler of `auth`'s row.
pub(super) async fn run<H: Host>(
    host: &H,
    auth: &Authorised,
    args: &Value,
) -> Result<Reply, ModError> {
    let Some(method) = Method::from_name(auth.row.name) else {
        // A live row without a handler: the coverage test forbids it.
        return Err(ModError::internal(format!(
            "no handler for {}",
            auth.row.name
        )));
    };
    match method {
        Method::ThemeGet => theme_get(args),
        Method::StorageGet => storage::get(host, auth, args).await,
        Method::StorageSet => storage::set(host, auth, args).await,
    }
}

/// `theme.get()`: PM's live theme. Rust has authorised it; the page answers from its theme
/// (`ModFrame.tsx`, `modTheme.ts`), which holds the CSS custom properties and the axes.
fn theme_get(args: &Value) -> Result<Reply, ModError> {
    no_args(args)?;
    Ok(Reply::Page {
        action: PageAction::ThemeGet,
        args: Value::Null,
    })
}

/// A call that takes nothing takes `null` (or nothing) or `{}`.
fn no_args(args: &Value) -> Result<(), ModError> {
    match args {
        Value::Null => Ok(()),
        Value::Object(map) if map.is_empty() => Ok(()),
        _ => Err(ModError::bad_args("This call takes no arguments.")),
    }
}

/// The arguments as `T`, whose shape is strict (`deny_unknown_fields`): a field PM doesn't know is
/// refused rather than ignored, so a typo never silently means something else.
pub(super) fn parse<T: DeserializeOwned>(args: &Value) -> Result<T, ModError> {
    T::deserialize(args).map_err(|e| ModError::bad_args(format!("Bad arguments: {e}.")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::api::Code;
    use serde_json::json;

    #[test]
    fn names_round_trip_and_nothing_else_resolves() {
        for m in Method::ALL {
            assert_eq!(Method::from_name(m.name()), Some(m));
        }
        for name in ["", "theme", "Theme.get", "theme.get ", "storage.delete"] {
            assert_eq!(Method::from_name(name), None, "{name:?}");
        }
    }

    #[test]
    fn theme_get_takes_nothing_and_hands_the_answer_to_the_page() {
        for args in [json!(null), json!({})] {
            assert_eq!(
                theme_get(&args),
                Ok(Reply::Page {
                    action: PageAction::ThemeGet,
                    args: Value::Null
                })
            );
        }
        for args in [json!({"x": 1}), json!([]), json!("dark"), json!(0)] {
            assert_eq!(theme_get(&args).unwrap_err().code, Code::BadArgs, "{args}");
        }
    }

    /// The page performs exactly the actions it knows (`modBridge.ts`'s `PAGE_ACTIONS`), and an
    /// action reaches the page by that name.
    #[test]
    fn every_page_action_is_one_the_page_performs() {
        let page = include_str!("../../../../src/components/mods/modBridge.ts");
        let line = page
            .lines()
            .find(|l| l.starts_with("export const PAGE_ACTIONS = "))
            .expect("modBridge.ts has PAGE_ACTIONS");
        for action in PageAction::ALL {
            assert_eq!(
                serde_json::to_value(action).unwrap(),
                json!(action.name()),
                "{action:?}"
            );
            assert!(
                line.contains(&format!("\"{}\"", action.name())),
                "the page doesn't perform {}",
                action.name()
            );
        }
        let listed = line.matches('"').count() / 2;
        assert_eq!(
            listed,
            PageAction::ALL.len(),
            "the page performs more: {line}"
        );
    }
}
