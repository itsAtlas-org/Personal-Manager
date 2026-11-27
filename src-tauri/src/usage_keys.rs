// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Which numeric keys a provider's `usage` object may carry, and what PM does about each.
//!
//! The list is `provider_usage_keys.json`, ONE file the weekly provider canary reads too, so the app
//! and the canary can't disagree about what counts as news. A reply's usage is walked once: a number
//! above zero under a key PM prices (`known_priced`) or knowingly ignores (`known_info`) changes
//! nothing; one under `known_unpriced` names a [`CostNote`] (PM can't price it, so the row is "at
//! least"); one under no key at all is a new kind of token the provider started reporting
//! (`NewTokenType`), the quiet way a bill can grow. The cost PM worked out is never touched: PM prices
//! what it can, and the note says the rest is missing.
//!
//! Only the NOTE is kept. A key's name is the vendor's text, so it is never logged, stored or shown;
//! a hostile or buggy response can at most set one of a fixed list of slugs.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::Value;

use crate::llm_gateway::CloudProvider;
use crate::openrouter::CostNote;

static KEYS_JSON: &str = include_str!("../provider_usage_keys.json");
static KEYS: OnceLock<Keys> = OnceLock::new();

/// A usage object nests a few levels at most (`cache_creation.ephemeral_1h_input_tokens`); anything
/// deeper is not a count PM knows and is not walked, so a runaway response can't make this recurse.
const MAX_DEPTH: usize = 4;

#[derive(Deserialize)]
struct Keys {
    providers: HashMap<String, ProviderKeys>,
}

#[derive(Deserialize)]
struct ProviderKeys {
    known_priced: Vec<String>,
    known_info: Vec<String>,
    /// Dotted path to the [`CostNote`] slug it stands for.
    known_unpriced: HashMap<String, String>,
}

/// The parsed file (parsed once). Panics only if the COMMITTED JSON is malformed, which the
/// parse-guard test below prevents from ever landing.
fn keys() -> &'static Keys {
    KEYS.get_or_init(|| {
        serde_json::from_str(KEYS_JSON)
            .expect("committed provider_usage_keys.json must be valid (see parse-guard test)")
    })
}

/// What a reply's usage object says PM can't price, if anything. `usage` is the provider's own
/// `usage` object. OpenRouter reports its own cost and has no list here, so it answers `None`; so
/// does a provider the file doesn't know.
pub(crate) fn note_for(provider: CloudProvider, usage: &Value) -> Option<CostNote> {
    if provider == CloudProvider::OpenRouter {
        return None;
    }
    let section = keys().providers.get(provider.as_str())?;
    let mut found: Option<CostNote> = None;
    walk(usage, "", 0, &mut |path, count| {
        if count <= 0.0 {
            // A zero count cannot change a price. The canary still sees a new zero key; a person's
            // Usage tab should not call every reply "at least" over one.
            return;
        }
        if section.known_priced.iter().any(|k| k == path)
            || section.known_info.iter().any(|k| k == path)
        {
            return;
        }
        let note = match section.known_unpriced.get(path) {
            Some(slug) => CostNote::from_slug(slug).unwrap_or(CostNote::NewTokenType),
            None => CostNote::NewTokenType,
        };
        found = worse(found, Some(note));
    });
    // Gemini's native shape lists the prompt by modality in an array, which the walk skips: anything
    // that isn't text is billed at a rate of its own.
    if provider == CloudProvider::Gemini && has_non_text_prompt(usage) {
        found = worse(found, Some(CostNote::NonTextPrompt));
    }
    found
}

/// The note that matters more when a reply (or a stream's several usage objects) names two: the
/// earlier in [`CostNote::ALL`], so a known token type outranks "a new kind". Deterministic, so a row's
/// note doesn't depend on the order the provider listed its keys.
pub(crate) fn worse(a: Option<CostNote>, b: Option<CostNote>) -> Option<CostNote> {
    let rank = |n: CostNote| {
        CostNote::ALL
            .iter()
            .position(|x| *x == n)
            .unwrap_or(usize::MAX)
    };
    match (a, b) {
        (Some(a), Some(b)) => Some(if rank(b) < rank(a) { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// Call `visit(dotted_path, value)` for every numeric leaf of a usage object. Arrays and anything
/// that isn't a number are skipped (a `service_tier` string, a `null`).
fn walk(value: &Value, path: &str, depth: usize, visit: &mut impl FnMut(&str, f64)) {
    match value {
        Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                visit(path, f);
            }
        }
        Value::Object(map) if depth < MAX_DEPTH => {
            for (key, child) in map {
                let next = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                walk(child, &next, depth + 1, visit);
            }
        }
        _ => {}
    }
}

/// Whether a Gemini usage object lists prompt tokens of a modality other than text.
fn has_non_text_prompt(usage: &Value) -> bool {
    usage["promptTokensDetails"].as_array().is_some_and(|rows| {
        rows.iter().any(|row| {
            row["tokenCount"].as_f64().is_some_and(|n| n > 0.0)
                && row["modality"]
                    .as_str()
                    .is_some_and(|m| !m.eq_ignore_ascii_case("TEXT"))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn note(p: CloudProvider, usage: Value) -> Option<CostNote> {
        note_for(p, &usage)
    }

    /// The committed file parses, covers the three direct providers, and every `known_unpriced`
    /// slug is one PM writes (so a typo in the shared file fails here, not silently as a new type).
    /// A key never sits in two lists, which would make its meaning depend on lookup order.
    #[test]
    fn the_committed_file_parses_and_names_only_slugs_pm_writes() {
        let k = keys();
        for provider in ["openai", "gemini", "anthropic"] {
            assert!(k.providers.contains_key(provider), "{provider}");
        }
        for (name, section) in &k.providers {
            for (path, slug) in &section.known_unpriced {
                assert!(
                    CostNote::from_slug(slug).is_some(),
                    "{name}: {path} -> {slug}"
                );
            }
            let mut all: Vec<&String> = section
                .known_priced
                .iter()
                .chain(section.known_info.iter())
                .chain(section.known_unpriced.keys())
                .collect();
            let n = all.len();
            all.sort();
            all.dedup();
            assert_eq!(all.len(), n, "{name}: a key is in two lists");
        }
    }

    /// Every usage object PM has recorded from a real provider walks clean: no note, so an ordinary
    /// reply is never called "at least". The canary's own list is what keeps this true as providers
    /// change.
    #[test]
    fn recorded_real_usage_objects_carry_no_note() {
        let anthropic = [
            json!({"input_tokens":23,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,
                   "cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0},
                   "output_tokens":8,"service_tier":"standard","inference_geo":"global","speed":"standard"}),
            json!({"input_tokens":45,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,
                   "output_tokens":36,"output_tokens_details":{"thinking_tokens":32}}),
        ];
        for u in anthropic {
            assert_eq!(note(CloudProvider::Anthropic, u), None);
        }
        let gemini = json!({"completion_tokens":2,"prompt_tokens":6906,
                            "prompt_tokens_details":{"cached_tokens":4861},"total_tokens":6908});
        assert_eq!(note(CloudProvider::Gemini, gemini), None);
        let openai = json!({"prompt_tokens":10,"completion_tokens":100,"total_tokens":110,
                            "prompt_tokens_details":{"cached_tokens":4,"audio_tokens":0},
                            "completion_tokens_details":{"reasoning_tokens":64,"audio_tokens":0,
                                "accepted_prediction_tokens":0,"rejected_prediction_tokens":0}});
        assert_eq!(note(CloudProvider::OpenAi, openai), None);
    }

    #[test]
    fn a_known_unpriced_key_above_zero_names_its_slug() {
        let audio = json!({"prompt_tokens":10,"completion_tokens":5,
                           "prompt_tokens_details":{"audio_tokens":7}});
        assert_eq!(
            note(CloudProvider::OpenAi, audio),
            Some(CostNote::AudioTokens)
        );
        let out_audio = json!({"completion_tokens_details":{"audio_tokens":1}});
        assert_eq!(
            note(CloudProvider::OpenAi, out_audio),
            Some(CostNote::AudioTokens)
        );
        let prediction = json!({"completion_tokens_details":{"rejected_prediction_tokens":3}});
        assert_eq!(
            note(CloudProvider::OpenAi, prediction),
            Some(CostNote::PredictionTokens)
        );
        let tool =
            json!({"input_tokens":1,"output_tokens":1,"server_tool_use":{"web_search_requests":2}});
        assert_eq!(
            note(CloudProvider::Anthropic, tool),
            Some(CostNote::ServerToolUse)
        );
        let one_hour = json!({"cache_creation":{"ephemeral_1h_input_tokens":9}});
        assert_eq!(
            note(CloudProvider::Anthropic, one_hour),
            Some(CostNote::PartialUsage)
        );
        let gem_audio = json!({"prompt_tokens_details":{"audio_tokens":4}});
        assert_eq!(
            note(CloudProvider::Gemini, gem_audio),
            Some(CostNote::AudioTokens)
        );
        // The same keys at zero are not news.
        let zero = json!({"prompt_tokens_details":{"audio_tokens":0},
                          "completion_tokens_details":{"accepted_prediction_tokens":0}});
        assert_eq!(note(CloudProvider::OpenAi, zero), None);
    }

    /// A numeric key in no list is a new kind of token, and only its slug survives: the key's name
    /// (the vendor's text) is not in the answer.
    #[test]
    fn an_unlisted_numeric_key_is_a_new_token_type_and_its_name_is_dropped() {
        let new_key = json!({"prompt_tokens":10,"completion_tokens":5,
                             "completion_tokens_details":{"hologram_tokens":3}});
        assert_eq!(
            note(CloudProvider::OpenAi, new_key),
            Some(CostNote::NewTokenType)
        );
        let top = json!({"input_tokens":1,"output_tokens":1,"batch_tokens":2});
        assert_eq!(
            note(CloudProvider::Anthropic, top),
            Some(CostNote::NewTokenType)
        );
        // Not numbers: never walked, never a new type.
        let words = json!({"input_tokens":1,"output_tokens":1,"service_tier":"priority",
                           "inference_geo":"eu","speed":null,"diagnostics":{"x":"y"},"list":[1,2]});
        assert_eq!(note(CloudProvider::Anthropic, words), None);
    }

    /// A known type outranks a new one, whatever order the provider listed the keys in.
    #[test]
    fn a_known_type_outranks_a_new_one() {
        let both = json!({"zzz_new_tokens":1,"completion_tokens_details":{"audio_tokens":1}});
        assert_eq!(
            note(CloudProvider::OpenAi, both),
            Some(CostNote::AudioTokens)
        );
        assert_eq!(
            worse(Some(CostNote::NewTokenType), Some(CostNote::ServerToolUse)),
            Some(CostNote::ServerToolUse)
        );
        assert_eq!(
            worse(None, Some(CostNote::NoUsage)),
            Some(CostNote::NoUsage)
        );
    }

    #[test]
    fn gemini_modalities_other_than_text_are_a_non_text_prompt() {
        let image = json!({"promptTokenCount":300,"promptTokensDetails":[
            {"modality":"TEXT","tokenCount":40},{"modality":"IMAGE","tokenCount":260}]});
        assert_eq!(
            note(CloudProvider::Gemini, image),
            Some(CostNote::NonTextPrompt)
        );
        let text = json!({"promptTokenCount":40,"promptTokensDetails":[
            {"modality":"TEXT","tokenCount":40}]});
        assert_eq!(note(CloudProvider::Gemini, text), None);
    }

    /// OpenRouter reports its own cost (and extra keys beside it), so it is never walked.
    #[test]
    fn openrouter_is_never_walked() {
        let u = json!({"prompt_tokens":1,"cost":0.01,"cost_details":{"upstream_inference_cost":0.01},
                       "is_byok":false,"anything_tokens":5});
        assert_eq!(note(CloudProvider::OpenRouter, u), None);
    }

    /// A deeply nested object is not walked past [`MAX_DEPTH`] and does not recurse without bound.
    #[test]
    fn a_runaway_nesting_is_cut_off() {
        let mut v = json!({"x_tokens": 5});
        for _ in 0..200 {
            v = json!({ "a": v });
        }
        assert_eq!(note(CloudProvider::OpenAi, v), None);
    }
}
