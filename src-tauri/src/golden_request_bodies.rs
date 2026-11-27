// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The golden request bodies the weekly provider canary sends (test-only).
//!
//! `tests/golden/request_bodies.json` holds, per provider, the exact bodies PM's REAL pure builders
//! make for one fixed tiny prompt. Each provider's own test module builds its bodies and hands them to
//! [`assert_matches`]; the canary (`scripts/provider-checks`) reads the same file and swaps in its own
//! prompt where the file says `<<SYSTEM>>` and `<<USER>>`. So the canary can never test a request PM
//! no longer sends, and a change to a body PM sends fails the PR gate here until the file (and with it
//! the canary) is updated: the failure prints the new body.
//!
//! The comparison is on each body's canonical serialisation (`serde_json` sorts the keys), not on the
//! file's layout, so Prettier may reflow the file freely.

use serde_json::Value;

/// The one file, read at compile time like `direct_models.json`.
const GOLDEN: &str = include_str!("../tests/golden/request_bodies.json");

/// Where the canary's prompt goes. Mirrored in `scripts/provider-checks/lib/data.mjs`.
pub(crate) const SYSTEM: &str = "<<SYSTEM>>";
pub(crate) const USER: &str = "<<USER>>";

/// Every provider and the case names its test must build, in the file's order of appearance.
const EXPECTED: [(&str, &[&str]); 3] = [
    ("openai", &["stream", "plain", "key_check"]),
    ("gemini", &["stream", "plain", "tier_probe"]),
    ("anthropic", &["stream", "plain", "key_check"]),
];

fn golden() -> Value {
    serde_json::from_str(GOLDEN).expect("tests/golden/request_bodies.json is not valid JSON")
}

/// The provider's golden `cases` object.
fn cases_of(provider: &str) -> serde_json::Map<String, Value> {
    golden()["providers"][provider]["cases"]
        .as_object()
        .unwrap_or_else(|| panic!("the golden file has no cases for {provider}"))
        .clone()
}

/// Compare what a provider's builders made with what the file holds, case by case and byte for byte
/// (canonical form). `built` is every case the provider's test made, by name.
pub(crate) fn assert_matches(provider: &str, built: &[(&str, Value)]) {
    let golden = cases_of(provider);
    let mut want: Vec<&str> = golden.keys().map(String::as_str).collect();
    let mut have: Vec<&str> = built.iter().map(|(n, _)| *n).collect();
    want.sort_unstable();
    have.sort_unstable();
    assert_eq!(
        have, want,
        "{provider}: the cases the test builds and the cases the golden file holds differ"
    );
    for (name, body) in built {
        let file = &golden[*name]["body"];
        let (a, b) = (
            serde_json::to_string(body).unwrap(),
            serde_json::to_string(file).unwrap(),
        );
        assert_eq!(
            a,
            b,
            "{provider}/{name}: PM's request body no longer matches tests/golden/request_bodies.json, \
             which the weekly canary sends. Update the file's `body` to:\n{}",
            serde_json::to_string_pretty(body).unwrap()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_golden_file_holds_exactly_the_three_providers_and_their_cases() {
        let g = golden();
        assert_eq!(g["schema_version"], 1);
        let providers = g["providers"].as_object().unwrap();
        assert_eq!(
            providers.len(),
            EXPECTED.len(),
            "a provider was added or dropped"
        );
        for (provider, names) in EXPECTED {
            let cases = cases_of(provider);
            let mut have: Vec<&str> = cases.keys().map(String::as_str).collect();
            let mut want: Vec<&str> = names.to_vec();
            have.sort_unstable();
            want.sort_unstable();
            assert_eq!(have, want, "{provider}");
            for (name, case) in &cases {
                assert!(case["body"].is_object(), "{provider}/{name} has no body");
            }
        }
    }

    #[test]
    fn the_canary_prompt_markers_appear_in_every_chat_body_and_no_key_does() {
        // The canary swaps its own prompt in at these two markers, so a body without them would send
        // the markers' absence as a different request than PM's.
        for (provider, names) in EXPECTED {
            let cases = cases_of(provider);
            for name in names
                .iter()
                .filter(|n| !n.ends_with("probe") && **n != "key_check")
            {
                let text = serde_json::to_string(&cases[*name]["body"]).unwrap();
                assert!(
                    text.contains(SYSTEM) && text.contains(USER),
                    "{provider}/{name}"
                );
            }
        }
        let raw = GOLDEN.to_lowercase();
        for needle in ["sk-", "aiza", "bearer", "api_key", "apikey"] {
            assert!(!raw.contains(needle), "the golden file mentions {needle}");
        }
    }
}
