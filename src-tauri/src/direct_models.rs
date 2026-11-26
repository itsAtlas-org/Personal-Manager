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
use crate::openrouter::{Completion, Usage};

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
    /// The ordered catalogue ids the key check asks to tell a paid key from a free one: each has
    /// `free_tier: false`, so a free key answers it with a limit-0 429 and a paid key with a reply.
    /// Empty for a provider with no tiers.
    #[serde(default)]
    pub probe_models: Vec<String>,
    pub models: Vec<DirectModel>,
}

/// A price that takes effect on a date: all three NEW prices (a null cached price means none).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PriceChange {
    /// The first UTC day it applies (YYYY-MM-DD).
    pub from: String,
    pub price_in: f64,
    pub price_cached_in: Option<f64>,
    pub price_out: f64,
}

/// What a model costs on a given day, USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Prices {
    pub price_in: f64,
    pub price_cached_in: Option<f64>,
    pub price_out: f64,
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
    /// The most PM can send as input: the window minus the output limit PM always reserves (the API
    /// counts prompt plus `max_tokens` against the window). The chat context meter reads this.
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
    /// Whether the provider gives this model a free allowance: null where the provider has no such
    /// thing (OpenAI), false for a model only a paid key can use, true where a free key can too.
    #[serde(default)]
    pub free_tier: Option<bool>,
    /// Prices that change on a date the provider has announced, ascending by `from`. Empty for most.
    #[serde(default)]
    pub price_changes: Vec<PriceChange>,
    /// How this model's thinking can be switched off, where the provider allows it: only
    /// `"disabled"` is legal in this version (Anthropic's Haiku 5.5, at effort `low` or below
    /// `xhigh`). Null on every other row: Fable and Opus cannot turn thinking off at all, and
    /// Sonnet's own switch (`between_tools`) is parked. The data lives here so the next model line
    /// edits a row, not code.
    #[serde(default)]
    pub thinking_off: Option<String>,
}

/// Whether a name (an id, a wire id or a display name) is Anthropic's restricted Mythos line, which
/// PM never offers, saves or calls (Bobby, 2026-11-26: "never mythos"). Case-insensitive.
pub(crate) fn names_mythos(name: &str) -> bool {
    name.to_ascii_lowercase().contains("mythos")
}

impl DirectModel {
    /// The prices that apply on `today` (a UTC date): the last announced change that has taken effect,
    /// else the row's own. A string compare is right for YYYY-MM-DD. Google bills by Pacific date, so a
    /// change lands up to a day off in UTC; the cost is an estimate and the Usage copy says so.
    pub(crate) fn prices_on(&self, today: &str) -> Prices {
        self.price_changes
            .iter()
            .rev()
            .find(|c| c.from.as_str() <= today)
            .map_or(
                Prices {
                    price_in: self.price_in,
                    price_cached_in: self.price_cached_in,
                    price_out: self.price_out,
                },
                |c| Prices {
                    price_in: c.price_in,
                    price_cached_in: c.price_cached_in,
                    price_out: c.price_out,
                },
            )
    }

    /// The next announced price change after `today`, for the picker's tip.
    pub(crate) fn next_change(&self, today: &str) -> Option<&PriceChange> {
        self.price_changes.iter().find(|c| c.from.as_str() > today)
    }

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

// ---- what a reply cost: provider-neutral arithmetic ------------------------------------------------

/// Today in UTC, as YYYY-MM-DD, for the dated prices.
pub(crate) fn utc_today() -> String {
    chrono::Utc::now().date_naive().to_string()
}

/// Turn what the server reported into what PM logs. PURE over its inputs.
///
/// `Completion.model` is ALWAYS left as a catalogue id, never None and never a raw wire id: the
/// catalogue row the served snapshot maps to, else the model that was asked for. About 25 callers log
/// `completion.model.or(Some(plan.primary_model_id()))`, and on a local-then-cloud route that primary
/// is the LOCAL model, so a None would book a direct provider's tokens and a computed price under the
/// local model's name. A direct arm holds one model, so the requested one is the only honest
/// fallback. The cost is worked out here too (a direct provider reports none). Tests pass a fixed
/// `today`, never the real one.
pub(crate) fn settle_on(
    mut completion: Completion,
    requested: &DirectModel,
    today: &str,
) -> Completion {
    let served = completion.model.as_deref().and_then(served_model_id);
    let priced = served.unwrap_or(requested);
    completion.model = Some(priced.id.clone());
    completion.usage.cost = cost_usd(priced, &completion.usage, today);
    completion
}

/// The catalogue row a served model name stands for: an exact wire id, else the same with a leading
/// `models/` (Google's native list names) or a trailing `-YYYY-MM-DD` snapshot date taken off. `None`
/// for a model PM doesn't list.
pub(crate) fn served_model_id(served_wire: &str) -> Option<&'static DirectModel> {
    let served_wire = served_wire.strip_prefix("models/").unwrap_or(served_wire);
    if let Some(m) = find_wire(served_wire) {
        return Some(m);
    }
    // "-" + YYYY-MM-DD is 11 bytes. The name comes off the wire, so cut with `get`: a multi-byte
    // character straddling the offset must be a miss, never a panic inside a billed stream.
    if served_wire.len() > 11 {
        let head = served_wire.get(..served_wire.len() - 11)?;
        let tail = served_wire.get(served_wire.len() - 11..)?;
        let date = tail.strip_prefix('-')?;
        if chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok() {
            return find_wire(head);
        }
    }
    None
}

/// What a request cost on `today`, from the tokens the provider reported and PM's bundled prices
/// (USD).
///
/// P is the prompt, C the part read from the cache, W the part written to it, O the completion
/// (reasoning included; the reasoning count is never added again). The ordinary input is
/// `max(0, P - C - W)`. Over the model's long-context line the WHOLE request is billed at the
/// higher rates. `None` when no usage came back at all, so the row shows "-" and never a made-up $0.
///
/// The cache arithmetic assumes the prompt count INCLUDES the cached and written parts. For Gemini that
/// is verified: build day (2026-11-26, step 7) sent one ~6.9K-token prompt twice and the second reply
/// said `cached_tokens` 4861 inside `prompt_tokens` 6906, so charging the cached share at the cached
/// price (or at the full input price when a row has none) never under-counts. For OpenAI it is how the
/// Responses guide shows it and is strongly implied for chat completions, but it is unverified there
/// (live test L4).
pub(crate) fn cost_usd(m: &DirectModel, u: &Usage, today: &str) -> Option<f64> {
    if u.prompt_tokens.is_none() && u.completion_tokens.is_none() {
        return None;
    }
    let prices = m.prices_on(today);
    let prompt = u.prompt_tokens.unwrap_or(0).max(0);
    let cached = u.cached_tokens.unwrap_or(0).max(0);
    let written = u.cache_write_tokens.unwrap_or(0).max(0);
    let out = u.completion_tokens.unwrap_or(0).max(0);
    let ordinary = (prompt - cached - written).max(0);

    let lc = &m.long_context;
    let long = prompt > lc.threshold;
    let (in_m, cached_m, write_m, out_m) = if long {
        (lc.in_mult, lc.cached_mult, lc.write_mult, lc.out_mult)
    } else {
        (1.0, 1.0, 1.0, 1.0)
    };
    let cached_price = prices.price_cached_in.unwrap_or(prices.price_in);
    let total = ordinary as f64 * prices.price_in * in_m
        + cached as f64 * cached_price * cached_m
        + written as f64 * m.price_cache_write * write_m
        + out as f64 * prices.price_out * out_m;
    Some(total / 1_000_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEVELS: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

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
                if s.provider == "openai" {
                    assert!(
                        !m.effort_levels.iter().any(|l| l == "minimal"),
                        "{} lists minimal, which no GPT-6.x model takes",
                        m.id
                    );
                    assert_eq!(m.free_tier, None, "{}: OpenAI has no free tier", m.id);
                    assert!(m.price_changes.is_empty());
                } else if s.provider == "anthropic" {
                    assert_eq!(m.free_tier, None, "{}: Anthropic has no free tier", m.id);
                    assert!(m.price_changes.is_empty());
                    // Anthropic's own words are low..max; "none" and "minimal" are other providers'.
                    assert!(!m.offers("none") && !m.offers("minimal"), "{}", m.id);
                } else {
                    assert!(
                        m.free_tier.is_some(),
                        "{} must say whether it has a free tier",
                        m.id
                    );
                    // No Gemini row takes "none": it only exists on the 2.5 models PM doesn't list.
                    assert!(!m.offers("none"), "{} lists none", m.id);
                }
                // Dated prices: strictly ascending real dates, sane numbers, cached no dearer.
                let mut last: Option<&str> = None;
                for c in &m.price_changes {
                    assert!(is_date(&c.from), "{} has a bad change date", m.id);
                    assert!(
                        last.is_none_or(|l| c.from.as_str() > l),
                        "{} changes out of order",
                        m.id
                    );
                    last = Some(&c.from);
                    for p in [c.price_in, c.price_out]
                        .into_iter()
                        .chain(c.price_cached_in)
                    {
                        assert!(
                            p.is_finite() && p >= 0.0,
                            "{} has a bad changed price",
                            m.id
                        );
                    }
                    if let Some(x) = c.price_cached_in {
                        assert!(
                            x <= c.price_in,
                            "{} caches dearer than it reads after a change",
                            m.id
                        );
                    }
                }
                if let Some(d) = &m.effort_default {
                    assert!(m.offers(d), "{} defaults to a level it lacks", m.id);
                }
                // Mythos is never offered, in any field (Bobby, 2026-11-26). A future edit that adds
                // a row for it fails the build here, the third of three layers (see `anthropic_direct`).
                for field in [&m.id, &m.wire, &m.name] {
                    assert!(!names_mythos(field), "{} names Mythos", m.id);
                }
                // Thinking can only be switched off the one way, and only on a row that offers `low`
                // (Haiku's off switch is a 400 above `high`; the background default is low).
                if let Some(off) = &m.thinking_off {
                    assert_eq!(off, "disabled", "{}: only \"disabled\" is legal", m.id);
                    assert!(m.offers("low"), "{} turns thinking off but lacks low", m.id);
                    assert_eq!(s.provider, "anthropic", "{}", m.id);
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
            // A provider with tiers: the probe list, and defaults a FREE key can use. An untouched
            // role on a paid-only model would 429 with limit 0 on a free key's first call.
            if s.provider == "gemini" {
                assert!(
                    s.probe_models.len() >= 2,
                    "a retirement must not leave the probe nothing"
                );
                for id in &s.probe_models {
                    let m = s
                        .models
                        .iter()
                        .find(|m| &m.id == id)
                        .expect("a probe row exists");
                    assert_eq!(
                        m.free_tier,
                        Some(false),
                        "{id} must be paid-only to tell tiers apart"
                    );
                }
                for id in [&s.default_chat, &s.default_background] {
                    let m = s.models.iter().find(|m| &m.id == id).unwrap();
                    assert_eq!(
                        m.free_tier,
                        Some(true),
                        "{id} is a default, so a free key must reach it"
                    );
                }
            } else {
                assert!(s.probe_models.is_empty());
            }
        }
        // Wires are global (`find_wire` is): no wire in two sections.
        let mut wires = std::collections::HashSet::new();
        for m in cat.providers.iter().flat_map(|s| s.models.iter()) {
            assert!(
                wires.insert(m.wire.clone()),
                "{} is in two sections",
                m.wire
            );
            // Deprecated or moving ids never ship: 2.5 retires and re-routes on 2026-12-01, and an
            // alias moves under PM.
            assert!(
                !m.wire.starts_with("gemini-2.5-") && !m.wire.ends_with("-latest"),
                "{}",
                m.wire
            );
        }
    }

    /// Which levels each Gemini row takes is the wire contract: a level a model doesn't list is a
    /// 400 in someone's chat. `minimal` exists on the two Flash-Lite rows only.
    #[test]
    fn gemini_levels_follow_each_models_native_set() {
        let levels = |id: &str| find(id).unwrap().effort_levels.clone();
        for id in [
            "gemini/gemini-3.5-flash-lite",
            "gemini/gemini-3.1-flash-lite",
        ] {
            assert_eq!(levels(id), ["minimal", "low", "medium", "high"], "{id}");
        }
        for id in [
            "gemini/gemini-3.8-flash",
            "gemini/gemini-3.1-pro-preview",
            "gemini/gemini-4-carbon",
            "gemini/gemini-4-argon",
        ] {
            assert_eq!(levels(id), ["low", "medium", "high"], "{id}");
        }
        for id in ["gemini/gemini-4-neon", "gemini/gemini-4-hydrogen"] {
            assert_eq!(levels(id), ["low", "medium", "high", "xhigh"], "{id}");
        }
    }

    #[test]
    fn a_dated_price_applies_from_its_day_and_not_before() {
        let carbon = find("gemini/gemini-4-carbon").unwrap();
        let flash = find("gemini/gemini-3.8-flash").unwrap();
        assert_eq!(carbon.prices_on("2026-11-30").price_in, 2.0);
        assert_eq!(carbon.prices_on("2026-11-30").price_out, 10.0);
        assert_eq!(carbon.prices_on("2026-12-01").price_in, 4.0);
        assert_eq!(carbon.prices_on("2026-12-01").price_out, 20.0);
        assert_eq!(carbon.prices_on("2026-12-01").price_cached_in, Some(0.2));
        assert_eq!(flash.prices_on("2026-12-31").price_in, 0.75);
        assert_eq!(flash.prices_on("2027-01-01").price_in, 1.5);
        assert_eq!(flash.prices_on("2027-01-01").price_out, 7.5);
        // A row with no change prices the same on every day.
        let lite = find("gemini/gemini-3.1-flash-lite").unwrap();
        assert_eq!(lite.prices_on("2020-01-01"), lite.prices_on("2099-01-01"));
        assert_eq!(carbon.next_change("2026-11-30").unwrap().from, "2026-12-01");
        assert!(carbon.next_change("2026-12-01").is_none());
    }

    fn usage(prompt: i64, completion: i64) -> Usage {
        Usage {
            prompt_tokens: Some(prompt),
            completion_tokens: Some(completion),
            ..Usage::default()
        }
    }

    #[test]
    fn the_cost_follows_the_price_on_the_day_and_the_long_context_line() {
        let flash = find("gemini/gemini-3.8-flash").unwrap();
        let one_m = usage(1_000_000, 1_000_000);
        let near = |a: Option<f64>, b: f64| (a.unwrap() - b).abs() < 1e-9;
        assert!(near(cost_usd(flash, &one_m, "2026-12-31"), 0.75 + 3.75));
        assert!(near(cost_usd(flash, &one_m, "2027-01-01"), 1.5 + 7.5));
        let carbon = find("gemini/gemini-4-carbon").unwrap();
        assert!(near(cost_usd(carbon, &one_m, "2026-11-30"), 2.0 + 10.0));
        assert!(near(cost_usd(carbon, &one_m, "2026-12-01"), 4.0 + 20.0));
        // 3.1 Pro: a 250,000-token prompt is over its 200,000 line, so the WHOLE request is billed at
        // 4 in and 18 out.
        let pro = find("gemini/gemini-3.1-pro-preview").unwrap();
        let long = usage(250_000, 1_000_000);
        assert!(near(cost_usd(pro, &long, "2026-11-26"), 0.25 * 4.0 + 18.0));
        let short = usage(200_000, 1_000_000);
        assert!(near(cost_usd(pro, &short, "2026-11-26"), 0.2 * 2.0 + 12.0));
        assert_eq!(cost_usd(pro, &Usage::default(), "2026-11-26"), None);
    }

    #[test]
    fn a_served_name_maps_to_its_row_with_or_without_models_or_a_date() {
        for served in ["gemini-3.8-flash", "models/gemini-3.8-flash"] {
            assert_eq!(
                served_model_id(served).unwrap().id,
                "gemini/gemini-3.8-flash"
            );
        }
        assert_eq!(
            served_model_id("gpt-6.2-sol-2026-11-01").unwrap().id,
            "openai/gpt-6.2-sol"
        );
        assert!(served_model_id("gemini-9").is_none());
        // A multi-byte character at the cut point is a miss, never a panic.
        assert!(served_model_id("gemini-3.8-flash-é-2026").is_none());
    }

    /// The model is ALWAYS a catalogue id, so a local-then-cloud plan never logs the local model's
    /// name against a direct provider's tokens.
    #[test]
    fn settling_always_names_a_catalogue_model_and_prices_it_on_the_given_day() {
        let flash = find("gemini/gemini-3.8-flash").unwrap();
        let reply = |model: Option<&str>| Completion {
            text: String::new(),
            model: model.map(str::to_string),
            usage: usage(1_000_000, 0),
            truncated: false,
        };
        let c = settle_on(reply(Some("models/gemini-3.8-flash")), flash, "2026-12-31");
        assert_eq!(c.model.as_deref(), Some("gemini/gemini-3.8-flash"));
        assert!((c.usage.cost.unwrap() - 0.75).abs() < 1e-9);
        let c = settle_on(reply(Some("gemini-3.8-flash")), flash, "2027-01-01");
        assert!((c.usage.cost.unwrap() - 1.5).abs() < 1e-9);
        // An unknown or missing served name falls back to the model that was asked for.
        for served in [Some("mystery-1"), None] {
            let c = settle_on(reply(served), flash, "2026-12-31");
            assert_eq!(c.model.as_deref(), Some("gemini/gemini-3.8-flash"));
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

    /// Anthropic's four rows, exactly: the ids, the one note, the one row that can switch thinking
    /// off, the neutral long-context block on three and Haiku's whole-request 5x above 100,000, and
    /// the defaults (Fable is never one).
    #[test]
    fn anthropics_four_rows_are_exactly_the_agreed_ones() {
        let rows = models(CloudProvider::Anthropic);
        let ids: Vec<&str> = rows.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "anthropic/claude-fable-5-2",
                "anthropic/claude-opus-5-5",
                "anthropic/claude-sonnet-5-5",
                "anthropic/claude-haiku-5-5",
            ]
        );
        for m in rows {
            assert_eq!(m.context, 1_000_000);
            assert_eq!(m.max_output, 128_000);
            // The room for input is the window minus the output PM always reserves.
            assert_eq!(m.max_input, m.context - m.max_output, "{}", m.id);
            assert_eq!(m.effort_levels, ["low", "medium", "high", "xhigh", "max"]);
            assert!(m.price_cached_in.unwrap() <= m.price_in);
            assert!(m.price_cache_write >= m.price_in);
            let lc = &m.long_context;
            if m.wire == "claude-haiku-5-5" {
                assert_eq!(
                    (
                        lc.threshold,
                        lc.in_mult,
                        lc.cached_mult,
                        lc.write_mult,
                        lc.out_mult
                    ),
                    (100_000, 5.0, 5.0, 5.0, 5.0)
                );
            } else {
                // Neutral: the threshold is the window itself and nothing multiplies.
                assert_eq!(lc.threshold, i64::from(m.context), "{}", m.id);
                assert_eq!(
                    (lc.in_mult, lc.cached_mult, lc.write_mult, lc.out_mult),
                    (1.0, 1.0, 1.0, 1.0)
                );
            }
            // Only Haiku can switch thinking off, and only the one way.
            assert_eq!(
                m.thinking_off.as_deref(),
                (m.wire == "claude-haiku-5-5").then_some("disabled"),
                "{}",
                m.id
            );
            // Only Fable carries a note, and it is Bobby's sentence.
            assert_eq!(
                m.note.as_deref(),
                (m.wire == "claude-fable-5-2")
                    .then_some("Anthropic keeps what you send to Fable for 30 days."),
                "{}",
                m.id
            );
        }
        let s = section(CloudProvider::Anthropic).unwrap();
        assert_eq!(s.default_chat, "anthropic/claude-sonnet-5-5");
        assert_eq!(s.default_background, "anthropic/claude-haiku-5-5");
        assert!(s.probe_models.is_empty());
        // Every provider row has a section and the reverse.
        for d in crate::direct_providers::PROVIDERS {
            assert!(section(d.provider).is_some());
        }
        assert_eq!(
            catalogue().providers.len(),
            crate::direct_providers::PROVIDERS.len()
        );
    }

    #[test]
    fn mythos_is_recognised_by_name_in_any_case() {
        for yes in [
            "claude-mythos-5-2",
            "Claude-MYTHOS-preview",
            "anthropic/claude-mythos-5-1",
            "Claude Mythos 5.2",
        ] {
            assert!(names_mythos(yes), "{yes}");
        }
        for no in ["claude-fable-5-2", "anthropic/claude-haiku-5-5", ""] {
            assert!(!names_mythos(no), "{no}");
        }
    }

    /// The Anthropic rows' cost arithmetic with the real numbers: no `price_changes`, so any date
    /// prices the same, and a long Haiku request is billed whole at 5x.
    #[test]
    fn anthropic_costs_do_not_depend_on_the_day() {
        let haiku = find("anthropic/claude-haiku-5-5").unwrap();
        let u = usage(100_001, 1000);
        assert_eq!(
            cost_usd(haiku, &u, "2020-01-01"),
            cost_usd(haiku, &u, "2099-01-01")
        );
        let near = |a: Option<f64>, b: f64| (a.unwrap() - b).abs() < 1e-9;
        assert!(near(cost_usd(haiku, &u, "2026-11-26"), 0.0525005));
        assert!(near(
            cost_usd(haiku, &usage(100_000, 1000), "2026-11-26"),
            0.0105
        ));
    }
}
