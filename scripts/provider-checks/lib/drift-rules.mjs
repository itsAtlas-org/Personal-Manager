// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The authority rule of the keyless drift check, as pure functions over parsed pages (lib/parse-prices.mjs)
// and the catalogue (src-tauri/direct_models.json):
//
//   - The VENDOR'S OWN page is the authority. It disagreeing with the catalogue is a FINDING
//     (`price-drift`, both numbers shown: public data). A page that parses to nothing, or that has no row
//     for a catalogue model, is `source-unparsable`: a restructured page is an alert, not silence.
//   - OpenRouter is a second opinion only (it ran ~10% off Google for 3.8 Flash). It disagreeing while
//     the vendor agrees with the catalogue is a NOTICE, and it is never the basis for an edit.
//   - Dated steps: the catalogue carries `price_changes`. The vendor's price is compared with what the
//     catalogue says applies TODAY (and yesterday, because Google bills by Pacific date and its page lags
//     a UTC day), so neither a step due tomorrow nor one that has just landed cries drift. A step due
//     within 14 days is a notice ("confirm on the vendor page"), and the vendor's announced steps are
//     compared with the catalogue's so a wrong date is found before the day.
//   - A price column or row PM does not know, on a model PM lists, is a FINDING (`new-price-dimension`):
//     it may be a cost PM is not counting. Batch prices are NOT compared: PM never calls a batch API, so
//     a batch change cannot raise PM's costs.
//   - Nothing here edits the catalogue; it feeds the spend maths, so a human and a PR change it.
//
// Pure Node built-ins, ESM, no dependencies (INVARIANTS.md I-18).

import { finding, notice } from "./findings.mjs";
import { effective } from "./parse-prices.mjs";
import { short } from "./check.mjs";

/** Micro-dollars at 1% relative. */
const close = (a, b) => Math.abs(a - b) <= 0.01 * Math.max(Math.abs(a), Math.abs(b));
// NOT exact, on purpose (the brief says "exact for multipliers"): the page's own long price over its own
// short price is a ratio of two numbers that each carry the 1% price tolerance, so an exact ratio would
// call 0.126 against 0.125 a multiplier change (5 becomes 4.9603; the test "is quiet within 1%" pins
// it). Twice the price tolerance is the least that is consistent with it, and a real change in a
// multiplier (5 to 4, 2 to 1.5) is far outside it.
const sameMult = (a, b) => Math.abs(a - b) <= 0.02 * Math.max(Math.abs(a), Math.abs(b));

/** The day before `date` (YYYY-MM-DD, UTC). */
export function dayBefore(date) {
  const d = new Date(`${date}T00:00:00Z`);
  d.setUTCDate(d.getUTCDate() - 1);
  return d.toISOString().slice(0, 10);
}

/** `date` plus `days`. */
export function addDays(date, days) {
  const d = new Date(`${date}T00:00:00Z`);
  d.setUTCDate(d.getUTCDate() + days);
  return d.toISOString().slice(0, 10);
}

/** The catalogue's prices on a date: the last announced step that has taken effect, else the row's own (direct_models.rs `prices_on`). */
export function pricesOn(model, date) {
  const steps = model.price_changes ?? [];
  let cur = {
    price_in: model.price_in,
    price_cached_in: model.price_cached_in,
    price_out: model.price_out,
  };
  for (const s of steps) if (s.from <= date) cur = s;
  return { price_in: cur.price_in, price_cached_in: cur.price_cached_in, price_out: cur.price_out };
}

/** What each page row key is for a catalogue model. */
export const rowKey = {
  anthropic: (m) => m.name.toLowerCase(),
  openai: (m) => m.wire,
  gemini: (m) => m.wire,
};

/** The prices the vendor page may show today: the catalogue's today, and yesterday's (Pacific date lag). */
function acceptable(model, field, today) {
  return [pricesOn(model, today)[field], pricesOn(model, dayBefore(today))[field]];
}

const METRICS = [
  ["in", "price_in"],
  ["cached", "price_cached_in"],
  ["out", "price_out"],
];

/**
 * One provider's page against its catalogue section. Returns `{ findings, notices }`.
 * `parsed` is a parse-prices result; `pageKey` names the source in sources.json.
 */
export function comparePrices({ provider, section, parsed, pageKey, today, page }) {
  const findings = [];
  const notices = [];
  if (parsed.rows.size === 0) {
    const what = parsed.unparsed[0] ?? "no rows were parsed";
    return {
      findings: [finding("source-unparsable", provider, { source: pageKey, what })],
      notices,
    };
  }
  for (const m of section.models) {
    const row = parsed.rows.get(rowKey[provider](m));
    if (!row) {
      findings.push(
        finding("source-unparsable", provider, {
          source: pageKey,
          what: `no row for ${short(m.wire)}`,
        }),
      );
      continue;
    }
    const price = (field, listed, catalogue) =>
      findings.push(
        finding("price-drift", provider, {
          model: m.wire,
          field,
          listed,
          catalogue,
          source: pageKey,
        }),
      );
    for (const [dim, field] of METRICS) {
      const listed = effective(row[dim], today);
      if (listed === null) {
        // The page shows no price where the catalogue has one (say "Not available"), or a dated schedule
        // with no step covering today (only "through <date>" left): not a number to compare.
        if (m[field] > 0)
          findings.push(
            finding("source-unparsable", provider, {
              source: pageKey,
              what:
                row[dim] === null
                  ? `no ${dim} price for ${short(m.wire)}`
                  : `no ${dim} price for ${short(m.wire)} covers today`,
            }),
          );
        continue;
      }
      const want = acceptable(m, field, today);
      if (!want.some((w) => close(listed, w))) price(field, listed, want[0]);
    }
    if (m.price_cache_write > 0) {
      const listed = effective(row.write, today);
      // A cache-write price the catalogue has and the page does not show (or no longer dates for today) is
      // an alert of its own: the cost-creep case is exactly a price that stops being stated.
      if (listed === null)
        findings.push(
          finding("source-unparsable", provider, {
            source: pageKey,
            what: `no cache write price for ${short(m.wire)}`,
          }),
        );
      else if (!close(listed, m.price_cache_write))
        price("price_cache_write", listed, m.price_cache_write);
    }
    // Long-context tier: the page's own long price over its own short price must equal the catalogue's
    // multiplier. Where the catalogue has a tier of 1.0 everywhere there is nothing to compare.
    const lc = m.long_context;
    if (lc && row.long) {
      for (const [dim, mult] of [
        ["in", lc.in_mult],
        ["cached", lc.cached_mult],
        ["write", lc.write_mult],
        ["out", lc.out_mult],
      ]) {
        const base = effective(row[dim], today);
        const long = row.long[dim];
        if (base && long && base > 0 && !sameMult(long / base, mult))
          price(`long_${dim}_mult`, Math.round((long / base) * 1e4) / 1e4, mult);
      }
      if (row.longThreshold && lc.threshold && row.longThreshold !== lc.threshold)
        price("long_threshold", row.longThreshold, lc.threshold);
    } else if (lc && !row.long && [lc.in_mult, lc.out_mult].some((x) => x !== 1)) {
      // The higher price over the long-prompt threshold (Haiku's row of its own, OpenAI's and Google's
      // long-context columns): a page that loses it, by a rename, a removal or a reworded line, would
      // leave long prompts under-priced without a single number differing, so it is an alert for every vendor.
      findings.push(
        finding("source-unparsable", provider, {
          source: pageKey,
          what: `no long-prompt row for ${short(m.wire)}`,
        }),
      );
    }
    // Free tier (Gemini only: null elsewhere on both sides).
    if (
      provider === "gemini" &&
      row.free !== null &&
      m.free_tier !== null &&
      row.free !== m.free_tier
    ) {
      findings.push(
        finding("free-tier-changed", provider, {
          model: m.wire,
          listed: String(row.free),
          catalogue: String(m.free_tier),
          source: pageKey,
        }),
      );
    }
    // Vendor-announced dated steps against the catalogue's, for input, cached input and output alike: the
    // run is weekly, so a step changed only on the cached or output price must be found before its day,
    // not up to six days after it by `price-drift`.
    for (const [dim, field] of METRICS) {
      const schedule = row[dim] ?? [];
      const announced = schedule.filter((e) => e.from !== undefined && e.from > dayBefore(today));
      for (const e of announced) {
        const step = (m.price_changes ?? []).find((s) => s.from === e.from);
        if (!step || !close(step[field], e.value))
          findings.push(
            finding("price-step-differs", provider, {
              model: m.wire,
              field,
              date: e.from,
              listed: e.value,
              // A step with no cached price (null in the catalogue) is "none" too: a finding field is a
              // number or a string, and "the vendor starts offering caching" is the case this check is for.
              catalogue: step?.[field] ?? "none",
              source: pageKey,
            }),
          );
      }
      for (const s of m.price_changes ?? []) {
        if (s.from <= dayBefore(today) || !(s[field] > 0)) continue;
        const seen = schedule.some((e) => e.from === s.from);
        // A page that shows only one flat price cannot confirm a step; only a page that dates its prices can contradict it.
        if (!seen && schedule.some((e) => e.from !== undefined || e.until !== undefined))
          findings.push(
            finding("price-step-differs", provider, {
              model: m.wire,
              field,
              date: s.from,
              listed: "none",
              catalogue: s[field],
              source: pageKey,
            }),
          );
      }
    }
    // A price PM does not know on this model's row.
    for (const header of new Set(row.extra))
      findings.push(
        finding("new-price-dimension", provider, {
          model: m.wire,
          header: short(header),
          source: pageKey,
        }),
      );
    // Cache storage per hour is not a token price PM counts; a change is a notice.
    const base = page.cache_storage_per_hour?.[m.wire];
    if (row.storage && base !== undefined) {
      const now = effective(row.storage, today);
      if (now !== null && !close(now, base))
        notices.push(notice("cache-storage-changed", provider, { model: m.wire }));
    }
  }
  return { findings, notices };
}

/** Models the vendor lists in PM's families that the catalogue lacks and PM has not said to ignore. */
export function newModels({ provider, section, parsed, page, pageKey }) {
  const known = new Set([
    ...section.models.map((m) => rowKey[provider](m)),
    ...(page.known_other_models ?? []).map((n) => n.toLowerCase()),
  ]);
  const family = FAMILY[provider];
  const out = [];
  for (const [key, row] of parsed.rows) {
    // Mythos is never offered: FAMILY names the families PM offers, and it is not one of them.
    if (known.has(key.toLowerCase()) || !family(key)) continue;
    const v = (dim) => effective(row[dim], "9999-12-31") ?? "none";
    out.push(
      finding("new-model", provider, {
        model: short(key.replace(/\s+/g, "-")),
        in: v("in"),
        cached: v("cached"),
        out: v("out"),
        source: pageKey,
      }),
    );
  }
  return out;
}

/**
 * The model families PM offers, by row key (static patterns; a page's text is never a pattern). They are
 * wide on purpose: "when new models are released" is the signal, so the next generation (gpt-7, Gemini 6,
 * a 4.1 or a "-preview" suffix) must not slip past a pattern written for today's names. The noise valve is
 * `known_other_models` in sources.json.
 */
const FAMILY = {
  anthropic: (key) => /^claude (fable|opus|sonnet|haiku) \d/.test(key.toLowerCase()),
  openai: (key) => /^gpt-[6-9](\.\d+)?(-[a-z]+)+$/.test(key),
  gemini: (key) => /^gemini-([3-9]|[1-9]\d)(\.\d+)?-[a-z]+(-[a-z]+)*$/.test(key),
};

/** Catalogue steps due within 14 days: a notice each ("confirm on the vendor page"). Needs no page. */
export function stepsDue(catalogue, today) {
  const out = [];
  const limit = addDays(today, 14);
  for (const section of catalogue.providers) {
    for (const m of section.models) {
      for (const s of m.price_changes ?? []) {
        if (s.from > today && s.from <= limit)
          out.push(notice("price-step-due", section.provider, { model: m.wire, date: s.from }));
      }
    }
  }
  return out;
}

/**
 * OpenRouter against the catalogue: notices only. A price differing while the vendor page agrees is
 * `cross-check-only`; no row is `cross-check-no-row`; an expiry the catalogue lacks is
 * `openrouter-expiry-differs`.
 */
export function crossCheck({ catalogue, rows, today, vendors, agreed }) {
  const notices = [];
  for (const section of catalogue.providers) {
    const prefix = vendors[section.provider];
    for (const m of section.models) {
      const id = `${prefix}/${m.wire}`.replace(/\./g, "-");
      const row = rows.get(id);
      if (!row) {
        notices.push(notice("cross-check-no-row", section.provider, { model: m.wire }));
        continue;
      }
      const want = pricesOn(m, today);
      const off = [
        [row.in, want.price_in],
        [row.out, want.price_out],
        [row.cached, want.price_cached_in],
      ].some(([listed, cat]) => listed !== null && cat !== null && !close(listed, cat));
      // Only when the vendor's own page agreed (a vendor finding already covers the other case).
      if (off && agreed(section.provider, m.wire))
        notices.push(notice("cross-check-only", section.provider, { model: m.wire }));
      if (row.expiry && row.expiry !== m.retires_on)
        notices.push(
          notice("openrouter-expiry-differs", section.provider, {
            model: m.wire,
            date: short(row.expiry),
          }),
        );
    }
  }
  return notices;
}
