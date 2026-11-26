// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The bundled list of the models PM offers on a provider the person reaches with their own key.
//!
//! It is PM's own list because the provider's `/v1/models` carries none of what PM needs: no
//! context window, no output limit, no price, no effort levels. So names, windows, prices and each
//! model's own effort levels live in `direct_models.json`, hand-curated and dated (`checked_on` per
//! model and per file), and the key check only intersects it with what the key can see. A model
//! PM can't price is a model PM can't offer: spend would read as nothing.
//!
//! Prices are USD per MILLION tokens in the file (readable) and are converted where they are used.
//! They are NEVER written into `model_pricing`: that table's staleness is `MAX(fetched_at)`, so
//! invented rows would make PM think OpenRouter's prices were fresh and stop refreshing them, and a
//! person who uses only their own key must not contact OpenRouter at all (#1069).
//!
//! The same `include_str!` + `OnceLock` shape as `local_catalog.rs`; a parse-guard test keeps a bad
//! row from ever landing.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::llm_gateway::CloudProvider;

/// The whole file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Catalogue {
    // Shape fields: carried so `deny_unknown_fields` validates the whole file, and so the
    // parse-guard test can assert on them.
    #[allow(dead_code)]
    pub schema_version: u32,
    #[allow(dead_code)]
    pub catalogue_version: u32,
    /// When the prices and levels were last checked against the provider's own pages (UTC date).
    pub checked_on: String,
    pub providers: Vec<ProviderSection>,
}

/// One provider's models and its defaults.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderSection {
    /// The stored token ("openai"), the same word as [`CloudProvider::as_str`].
    pub provider: String,
    /// When the data terms the Settings card summarises were last read (UTC date).
    pub terms_checked_on: String,
    /// The model a role gets before the person picks one.
    pub default_chat: String,
    pub default_background: String,
    pub models: Vec<DirectModel>,
}

/// What a request costs more once it is long: the provider bills the WHOLE request at the higher
/// rates, not only the part over the line.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LongContext {
    /// Prompt tokens above which the multipliers apply.
    pub threshold: i64,
    pub in_mult: f64,
    pub cached_mult: f64,
    pub write_mult: f64,
    pub out_mult: f64,
}

/// One model PM offers.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DirectModel {
    /// The OpenRouter-style id (`openai/<wire id>`), which is how PM names the model at rest: in the
    /// settings, in the usage log and in the picker.
    pub id: String,
    /// The id the provider's own API knows it by. Only the provider's module sends it.
    pub wire: String,
    pub name: String,
    pub context: u32,
    #[allow(dead_code)]
    pub max_input: u32,
    /// The most the model can write in one reply. PM sends this as the output limit.
    pub max_output: u32,
    pub price_in: f64,
    pub price_cached_in: Option<f64>,
    pub price_cache_write: f64,
    pub price_out: f64,
    pub long_context: LongContext,
    /// The levels THIS model accepts, lowest first. PM offers exactly these ("we tailor it").
    pub effort_levels: Vec<String>,
    /// The level the model uses when the field is omitted, where the provider says so.
    #[allow(dead_code)]
    pub effort_default: Option<String>,
    pub input_modalities: Vec<String>,
    pub checked_on: String,
    /// A date the provider has announced this model goes away, if it has.
    pub retires_on: Option<String>,
    /// A line the picker shows under the name (null on every OpenAI row).
    pub note: Option<String>,
}

impl DirectModel {
    /// Whether the model takes this effort level.
    pub(crate) fn offers(&self, level: &str) -> bool {
        self.effort_levels.iter().any(|l| l == level)
    }

    /// Whether the provider's announced shutdown date has come.
    pub(crate) fn retired(&self, today: &str) -> bool {
        self.retires_on.as_deref().is_some_and(|d| d <= today)
    }
}

static CATALOGUE_JSON: &str = include_str!("../direct_models.json");
static CATALOGUE: OnceLock<Catalogue> = OnceLock::new();

/// The parsed file (parsed once). Panics only if the COMMITTED JSON is malformed, which the
/// parse-guard test prevents from ever landing.
pub(crate) fn catalogue() -> &'static Catalogue {
    CATALOGUE.get_or_init(|| {
        serde_json::from_str(CATALOGUE_JSON)
            .expect("committed direct_models.json must be valid (see parse-guard test)")
    })
}

/// When the prices were checked.
pub(crate) fn checked_on() -> &'static str {
    &catalogue().checked_on
}

/// A provider's section, if PM has one.
pub(crate) fn section(provider: CloudProvider) -> Option<&'static ProviderSection> {
    catalogue()
        .providers
        .iter()
        .find(|s| s.provider == provider.as_str())
}

/// A provider's models, empty for a provider PM has no list for.
pub(crate) fn models(provider: CloudProvider) -> &'static [DirectModel] {
    section(provider).map_or(&[], |s| s.models.as_slice())
}

/// A model by its OpenRouter-style id, on any provider.
pub(crate) fn find(id: &str) -> Option<&'static DirectModel> {
    catalogue()
        .providers
        .iter()
        .flat_map(|s| s.models.iter())
        .find(|m| m.id == id)
}

/// A model by the id its provider's API uses.
pub(crate) fn find_wire(wire: &str) -> Option<&'static DirectModel> {
    catalogue()
        .providers
        .iter()
        .flat_map(|s| s.models.iter())
        .find(|m| m.wire == wire)
}

/// The model a role gets before a person picks one.
pub(crate) fn default_for(
    provider: CloudProvider,
    role: crate::llm_gateway::Role,
) -> Option<&'static DirectModel> {
    let s = section(provider)?;
    let id = match role {
        crate::llm_gateway::Role::Chat => &s.default_chat,
        crate::llm_gateway::Role::Background => &s.default_background,
    };
    s.models.iter().find(|m| &m.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEVELS: [&str; 6] = ["none", "low", "medium", "high", "xhigh", "max"];

    fn is_date(s: &str) -> bool {
        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
    }

    /// The committed file parses and every row is sane. A bad price or a misspelt level would
    /// otherwise surface as a wrong bill or a 400 in somebody's chat.
    #[test]
    fn the_committed_catalogue_is_valid() {
        let cat = catalogue();
        let today = chrono::Utc::now().date_naive().to_string();
        assert!(is_date(&cat.checked_on) && cat.checked_on <= today);
        let mut ids = std::collections::HashSet::new();
        for s in &cat.providers {
            assert!(is_date(&s.terms_checked_on) && s.terms_checked_on <= today);
            assert!(!s.models.is_empty(), "{} has no models", s.provider);
            for m in &s.models {
                assert!(ids.insert(m.id.clone()), "{} listed twice", m.id);
                let prefix = format!("{}/", s.provider);
                assert!(m.id.starts_with(&prefix), "{} lacks {prefix}", m.id);
                assert!(!m.wire.is_empty());
                assert_eq!(m.id, format!("{prefix}{}", m.wire), "id is provider/wire");
                assert!(!m.name.trim().is_empty());
                for p in [m.price_in, m.price_cache_write, m.price_out]
                    .into_iter()
                    .chain(m.price_cached_in)
                {
                    assert!(p.is_finite() && p >= 0.0, "{} has a bad price", m.id);
                }
                if let Some(c) = m.price_cached_in {
                    assert!(c <= m.price_in, "{} caches dearer than it reads", m.id);
                }
                let lc = &m.long_context;
                assert!(lc.threshold > 0);
                for x in [lc.in_mult, lc.cached_mult, lc.write_mult, lc.out_mult] {
                    assert!(x.is_finite() && x >= 1.0, "{} shrinks when long", m.id);
                }
                assert!(m.max_output <= m.context && m.max_input <= m.context);
                assert!(!m.effort_levels.is_empty());
                let mut last = None;
                for level in &m.effort_levels {
                    let at = LEVELS
                        .iter()
                        .position(|l| l == level)
                        .unwrap_or_else(|| panic!("{} lists unknown level {level}", m.id));
                    assert!(last.is_none_or(|l| at > l), "{} levels out of order", m.id);
                    last = Some(at);
                }
                assert!(
                    !m.effort_levels.iter().any(|l| l == "minimal"),
                    "{} lists minimal, which no GPT-6.x model takes",
                    m.id
                );
                if let Some(d) = &m.effort_default {
                    assert!(m.offers(d), "{} defaults to a level it lacks", m.id);
                }
                assert!(is_date(&m.checked_on) && m.checked_on <= today);
                if let Some(r) = &m.retires_on {
                    assert!(is_date(r));
                }
                if let Some(n) = &m.note {
                    assert!((1..=200).contains(&n.chars().count()));
                }
                assert!(!m.input_modalities.is_empty());
            }
            assert!(s.models.iter().any(|m| m.id == s.default_chat));
            assert!(s.models.iter().any(|m| m.id == s.default_background));
        }
    }

    #[test]
    fn a_wire_id_and_an_id_find_the_same_row() {
        let m = find("openai/gpt-6.2-sol").expect("sol");
        assert_eq!(find_wire("gpt-6.2-sol").map(|w| &w.id), Some(&m.id));
        assert!(find("gpt-6.2-sol").is_none(), "an id is the namespaced one");
        assert!(find_wire("openai/gpt-6.2-sol").is_none());
    }

    #[test]
    fn minimal_is_offered_by_no_model_and_none_only_where_the_provider_takes_it() {
        assert!(!find("openai/gpt-6.2-sol").unwrap().offers("none"));
        assert!(!find("openai/gpt-6.2-astra").unwrap().offers("none"));
        assert!(find("openai/gpt-6-luna").unwrap().offers("none"));
        assert!(find("openai/gpt-6-luna").unwrap().offers("low"));
    }
}
