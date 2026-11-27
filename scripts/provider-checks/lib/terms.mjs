// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Terms and data-retention pages: PHRASE assertions, never page hashes. A whole page is noisy (OpenAI's
// carries time-bound lines, Anthropic's changes whenever a model joins its list), so this keeps a short
// list per page of the sentences PM's own wording rests on. A sentence that is gone is a real finding,
// and it also catches a redirect to an unrelated page. Matching is on normalised text: markup, links
// and emphasis stripped, entities decoded, curly quotes straightened, NFKC, spaces collapsed, lower
// case. The phrases come from sources.json (committed, PM's own words); the page's text is only ever
// searched for them, never used as a pattern.
//
// Pure Node built-ins, ESM, no dependencies (INVARIANTS.md I-18).

import { finding, notice } from "./findings.mjs";
import { short } from "./check.mjs";

const ENTITIES = {
  "&quot;": '"',
  "&amp;": "&",
  "&lt;": "<",
  "&gt;": ">",
  "&nbsp;": " ",
  "&#39;": "'",
  "&apos;": "'",
  "&rsquo;": "'",
  "&lsquo;": "'",
  "&ldquo;": '"',
  "&rdquo;": '"',
};

/** Page text as one lower-case line of plain words, comparable with a phrase run through the same function. */
export function normaliseText(raw) {
  let t = String(raw)
    // End tags may carry spaces or junk before the ">" ("</script >"): match them too, so a script
    // body never reads as page wording.
    .replace(/<script\b[^>]*>[\s\S]*?<\/script\b[^>]*>/gi, " ")
    .replace(/<style\b[^>]*>[\s\S]*?<\/style\b[^>]*>/gi, " ")
    .replace(/<[^>]*>/g, " ")
    .replace(/\{%[^%]*%\}/g, " ")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1");
  for (const [entity, ch] of Object.entries(ENTITIES)) t = t.split(entity).join(ch);
  return t
    .replace(/[‘’]/g, "'")
    .replace(/[“”]/g, '"')
    .replace(/[*_`#>|]/g, " ")
    .normalize("NFKC")
    .replace(/\s+/g, " ")
    .trim()
    .toLowerCase();
}

/** The sentence of `norm` that contains `marker` (split on ". "), or null. */
export function sentenceWith(norm, marker) {
  const at = norm.indexOf(marker);
  if (at < 0) return null;
  const start = norm.lastIndexOf(". ", at);
  const end = norm.indexOf(". ", at);
  return norm.slice(start < 0 ? 0 : start + 2, end < 0 ? undefined : end + 1);
}

/**
 * The phrase assertions of one terms page. `page` is its entry in sources.json:
 *   phrases        [{ id, text }]  each must be present
 *   covered_models { marker }      Anthropic: the sentence that lists the models needing 30-day retention
 *   stamp          "march 23, 2026"  the page's "Effective" date as last read (a change is a notice)
 * Returns `{ findings, notices }`. `catalogue` is the provider's catalogue section.
 */
export function checkTerms({ provider, text, pageKey, page, section }) {
  const findings = [];
  const notices = [];
  const norm = normaliseText(text);
  for (const p of page.phrases ?? []) {
    if (!norm.includes(normaliseText(p.text)))
      findings.push(finding("phrase-vanished", provider, { source: pageKey, phrase: p.id }));
  }
  if (page.covered_models) {
    const marker = normaliseText(page.covered_models.marker);
    const sentence = sentenceWith(norm, marker);
    if (sentence === null) {
      findings.push(
        finding("phrase-vanished", provider, { source: pageKey, phrase: page.covered_models.id }),
      );
    } else {
      // The alarm that matters most: a model PM lists as not needing 30-day retention (anything but
      // Fable) appearing in the sentence that says which models do. And each Fable PM lists must be in it.
      for (const m of section.models) {
        const named = sentence.includes(normaliseText(m.name));
        const isFable = /fable/i.test(m.name);
        if (isFable && !named)
          findings.push(
            finding("phrase-vanished", provider, {
              source: pageKey,
              phrase: `${page.covered_models.id}-${short(m.wire)}`,
            }),
          );
        if (!isFable && named)
          findings.push(
            finding("covered-model-listed", provider, { model: m.wire, source: pageKey }),
          );
      }
    }
  }
  if (page.stamp) {
    const m = /effective ([a-z]+ \d{1,2}, \d{4})/.exec(norm);
    if (!m || m[1] !== page.stamp) notices.push(notice("terms-stamp-changed", provider, {}));
  }
  return { findings, notices };
}
