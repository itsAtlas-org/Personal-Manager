// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The keyless drift check against committed, raw excerpts of the vendors' own pages (fixtures/drift):
// the parsers, the authority rule (vendor page = authority, OpenRouter = second opinion), dated price
// steps, the pricing columns PM does not know, restructured pages, phrase assertions, and what a page
// that cannot be fetched may and may not do. The drifted and restructured pages are made here from the
// fixtures by `drifted`, which throws if the text it edits is not there.

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { driftFetch, drifted, fx, quiet } from "./testkit.mjs";
import { catalogueFor, loadCatalogue, loadSources } from "./lib/data.mjs";
import {
  capFindings,
  driftSummaryLines,
  main,
  MAX_PAGE_CHARS,
  MAX_PER_PROVIDER,
  runDrift,
} from "./drift.mjs";
import { addDays, comparePrices, dayBefore, pricesOn, rowKey } from "./lib/drift-rules.mjs";
import {
  effective,
  isoFromWords,
  money,
  mdTables,
  parseAnthropicPricing,
  parseGeminiPricing,
  parseOpenAIPricing,
  parseOpenRouter,
  slug,
} from "./lib/parse-prices.mjs";
import { normaliseText, sentenceWith } from "./lib/terms.mjs";
import {
  describe as describeFinding,
  finding,
  FINDINGS,
  MAX_FINDINGS,
  NOTICES,
  parseResult,
} from "./lib/findings.mjs";
import { assertPublic } from "./report.mjs";

quiet();

const SOURCES = loadSources();
const CATALOGUE = loadCatalogue();
const PAGES = SOURCES.drift.pages;
const U = {
  anthropic: PAGES["anthropic-pricing"].url,
  openai: PAGES["openai-pricing"].url,
  gemini: PAGES["gemini-pricing"].url,
  openrouter: PAGES["openrouter-models"].url,
  aRet: PAGES["anthropic-retention"].url,
  oData: PAGES["openai-data"].url,
  gTerms: PAGES["gemini-terms"].url,
};
const text = (name) => fx(`drift/${name}`);
const ANTHROPIC = text("anthropic-pricing.md.txt");
const OPENAI = text("openai-pricing.md.txt");
const GEMINI = text("gemini-pricing.md.txt");
const OR = JSON.stringify(text("openrouter-models.json"));
const A_RET = text("anthropic-retention.md.txt");
const O_DATA = text("openai-data.md.txt");
const G_TERMS = text("gemini-terms.html.txt");

const TODAY = "2026-11-27";
const NO_SLEEP = async () => {};

/** Run the whole check on the fixtures, with some pages replaced. */
function run(pages = {}, { today = TODAY, catalogue = CATALOGUE, sources = SOURCES } = {}) {
  const fetch = driftFetch(pages);
  return runDrift({ sources, catalogue, today, deps: { fetch, sleep: NO_SLEEP } }).then((r) =>
    Object.assign(r, { fetch }),
  );
}
const ids = (r) => r.findings.map((f) => f.id);
const noticeIds = (r) => r.notices.map((n) => n.id);
const one = (r, id) => {
  const hits = r.findings.filter((f) => f.id === id);
  assert.equal(hits.length, 1, `${id} x${hits.length}: ${JSON.stringify(r.findings)}`);
  return hits[0];
};

/** Replace `from` with `to` only inside the Gemini section whose id is `id`. */
function inSection(page, id, from, to) {
  const parts = page.split(/^## /m);
  const i = parts.findIndex((p) => p.includes(`{% id="${id}" %}`));
  if (i < 0) throw new Error(`no section ${id}`);
  parts[i] = drifted(parts[i], from, to);
  return parts.join("## ");
}
/** A new column on a markdown price table: a header text and one cell on every row that starts `rowPrefix`. */
function addColumn(page, rowPrefix, header, cell) {
  return page
    .split("\n")
    .map((l) => {
      if (l.startsWith("| Model |")) return `${l} ${header} |`;
      if (/^\| :?-{2,}/.test(l)) return `${l} --- |`;
      return l.startsWith(rowPrefix) ? `${l} ${cell} |` : l;
    })
    .join("\n");
}

/** OpenRouter's fixture with one model's entry edited: `patch(model)` mutates it. */
function orEdit(id, patch) {
  const j = JSON.parse(OR);
  const m = j.data.find((x) => x.id === id);
  if (!m) throw new Error(`no OpenRouter row ${id}`);
  patch(m);
  return JSON.stringify(j);
}

describe("the fixtures reproduce what the pages said on 2026-11-27", () => {
  it("every catalogue model is found on its vendor's page with the catalogue's own prices: nothing to report", async () => {
    const r = await run();
    assert.equal(r.status, "ok");
    assert.deepEqual(r.findings, []);
    assert.deepEqual(r.unchecked ?? [], []);
  });

  it("the only notices are the two Gemini steps due on 2026-12-01, and OpenRouter being 10% over Google for 3.8 Flash", async () => {
    const r = await run();
    const got = r.notices.map((n) => `${n.id}:${n.model ?? ""}:${n.date ?? ""}`).sort();
    assert.deepEqual(got, [
      "cross-check-only:gemini-3.8-flash:",
      "price-step-due:gemini-4-argon:2026-12-01",
      "price-step-due:gemini-4-carbon:2026-12-01",
    ]);
  });

  it("asks only the addresses in sources.json, with the generic User-Agent, no credential, and follows redirects", async () => {
    const r = await run();
    const asked = r.fetch.calls.map((c) => c.url).sort();
    assert.deepEqual(
      asked,
      Object.values(PAGES)
        .map((p) => p.url)
        .sort(),
    );
    for (const c of r.fetch.calls) {
      assert.deepEqual(Object.keys(c.init.headers).sort(), ["accept", "user-agent"]);
      assert.equal(c.init.headers["user-agent"], "pm-provider-check");
      assert.equal(c.init.redirect, "follow");
      assert.equal(c.init.method, "GET");
    }
  });

  it("is the same on each side of both dated steps (Google bills by Pacific date, so its page lags a UTC day)", async () => {
    for (const today of [
      "2026-11-30",
      "2026-12-01",
      "2026-12-02",
      "2026-12-31",
      "2027-01-01",
      "2027-01-02",
    ]) {
      const r = await run({}, { today });
      assert.deepEqual(r.findings, [], today);
    }
  });
});

describe("the parsers read columns by header text", () => {
  const anthropic = parseAnthropicPricing(ANTHROPIC, PAGES["anthropic-pricing"].known);
  const openai = parseOpenAIPricing(OPENAI, PAGES["openai-pricing"].known);
  const gemini = parseGeminiPricing(GEMINI, PAGES["gemini-pricing"].known);

  it("Anthropic: one row per model; Haiku has a second row for prompts over 100,000 tokens", () => {
    const sonnet = anthropic.rows.get("claude sonnet 5.5");
    assert.deepEqual(
      [sonnet.in, sonnet.cached, sonnet.write, sonnet.out].map((e) => e[0].value),
      [2, 0.1, 2.5, 10],
    );
    const haiku = anthropic.rows.get("claude haiku 5.5");
    assert.equal(haiku.in[0].value, 0.1);
    assert.deepEqual(haiku.long, { in: 0.5, cached: 0.05, write: 0.625, out: 2.5 });
    assert.equal(haiku.longThreshold, 100000);
  });

  it("a table whose columns are in another order still reads right (a synthetic table, hand-made for this test)", () => {
    const table = [
      "| Model | Output tokens | Cache hits and refreshes | Base input tokens | 5m cache writes | 1h cache writes |",
      "| :---- | :---- | :---- | :---- | :---- | :---- |",
      "| Claude Test 1 | $9 / MTok | $0.3 / MTok | $3 / MTok | $3.75 / MTok | $6 / MTok |",
    ].join("\n");
    const r = parseAnthropicPricing(table, PAGES["anthropic-pricing"].known);
    const row = r.rows.get("claude test 1");
    assert.deepEqual(
      [row.in, row.cached, row.write, row.out].map((e) => e[0].value),
      [3, 0.3, 3.75, 9],
    );
    assert.deepEqual(row.extra, []);
  });

  it("OpenAI: short and long context columns, and the Batch table is never read", () => {
    const luna = openai.rows.get("gpt-6-luna");
    assert.deepEqual(
      [luna.in, luna.cached, luna.write, luna.out].map((e) => e[0].value),
      [0.1, 0.01, 0.125, 0.5],
    );
    assert.deepEqual(luna.long, { in: 0.2, cached: 0.02, write: 0.25, out: 0.75 });
    assert.equal(luna.extra.length, 0, "the 12h write columns are known and ignored");
    assert.equal(openai.rows.get("gpt-6.2-sol").in[0].value, 2, "not the batch table's 1");
  });

  it("Gemini: dated prices as a schedule, the long-context tier, free tier, cache storage, and the Batch tab is never read", () => {
    const carbon = gemini.rows.get("gemini-4-carbon");
    assert.deepEqual(carbon.in, [
      { value: 2, until: "2026-11-30" },
      { value: 4, from: "2026-12-01" },
    ]);
    assert.equal(effective(carbon.in, "2026-11-30"), 2);
    assert.equal(effective(carbon.in, "2026-12-01"), 4);
    assert.equal(carbon.free, false);
    assert.equal(gemini.rows.get("gemini-4-hydrogen").free, true);
    const pro = gemini.rows.get("gemini-3.1-pro-preview");
    assert.deepEqual(pro.long, { in: 4, cached: 0.4, write: null, out: 18 });
    assert.equal(pro.longThreshold, 200000);
    assert.equal(pro.storage[0].value, 4.5);
    const flash = gemini.rows.get("gemini-3.8-flash");
    assert.equal(effective(flash.storage, TODAY), 0.5);
    assert.equal(effective(flash.storage, "2027-01-01"), 1);
    // The audio line of 3.1 Flash-Lite is another modality, not a second input price.
    assert.deepEqual(gemini.rows.get("gemini-3.1-flash-lite").in, [{ value: 0.25 }]);
  });

  it("OpenRouter: dots and dashes in ids are one thing, prices are per million tokens, :batch offers are skipped", () => {
    const r = parseOpenRouter(OR);
    assert.deepEqual(r.rows.get("anthropic/claude-sonnet-5-5"), {
      in: 2,
      out: 10,
      cached: 0.1,
      write: 2.5,
      expiry: null,
    });
    assert.ok(r.rows.has("google/gemini-3-8-flash"));
    assert.ok([...r.rows.keys()].every((k) => !k.includes(":")));
    assert.equal(parseOpenRouter("<html>").rows.size, 0);
    assert.equal(parseOpenRouter('{"data":"x"}').rows.size, 0);
  });

  it("the small readers", () => {
    assert.equal(money("$0.10 / MTok<sup>2</sup>"), 0.1);
    assert.equal(money("$1,050.5"), 1050.5);
    assert.equal(money("-"), null);
    assert.equal(money("Not available"), null);
    assert.equal(isoFromWords("starting December 1, 2026."), "2026-12-01");
    assert.equal(isoFromWords("sometime"), null);
    assert.equal(slug("Short context 12h write"), "short-context-12h-write");
    assert.equal(mdTables("### H\n| A | B |\n| --- | --- |\n| 1 | 2 |\n")[0].heading, "H");
    assert.equal(effective([{ value: 1, until: "2026-01-01" }], "2026-01-02"), null);
    assert.equal(dayBefore("2026-12-01"), "2026-11-30");
    assert.equal(addDays("2026-12-25", 14), "2027-01-08");
  });
});

describe("the authority rule", () => {
  it("a vendor price that differs from the catalogue is a price-drift finding with both numbers", async () => {
    const r = await run({
      [U.anthropic]: drifted(
        ANTHROPIC,
        "$0.10 / MTok<sup>2</sup> | $10 / MTok |",
        "$0.10 / MTok<sup>2</sup> | $12 / MTok |",
      ),
    });
    assert.deepEqual(one(r, "price-drift"), {
      id: "price-drift",
      provider: "anthropic",
      fields: {
        model: "claude-sonnet-5-5",
        field: "price_out",
        listed: 12,
        catalogue: 10,
        source: "anthropic-pricing",
      },
    });
    assert.deepEqual(ids(r), ["price-drift"]);
  });

  it("a changed cache-hit price (the 'racks up cost' kind) is a finding too, for Anthropic, OpenAI and Gemini", async () => {
    const r = await run({
      [U.anthropic]: drifted(ANTHROPIC, "$0.10 / MTok<sup>2</sup>", "$0.20 / MTok<sup>2</sup>"),
      [U.openai]: drifted(
        OPENAI,
        "| gpt-6-luna | $0.10 | $0.50 | $0.01 |",
        "| gpt-6-luna | $0.10 | $0.50 | $0.05 |",
      ),
      [U.gemini]: inSection(GEMINI, "gemini-4-hydrogen", "* $0.075 ", "* $0.15 "),
    });
    const cached = r.findings.filter((f) => f.fields.field === "price_cached_in");
    assert.deepEqual(cached.map((f) => f.provider).sort(), ["anthropic", "gemini", "openai"]);
  });

  it("the cache write price and the long-context multipliers are compared too", async () => {
    const r = await run({
      [U.openai]: drifted(
        OPENAI,
        "| gpt-6-luna | $0.10 | $0.50 | $0.01 | $0.125 | $0.25 | $0.20 | $0.75 | $0.02 | $0.25 | $0.50 |",
        "| gpt-6-luna | $0.10 | $0.50 | $0.01 | $0.20 | $0.25 | $0.20 | $1.50 | $0.02 | $0.40 | $0.50 |",
      ),
    });
    assert.deepEqual(r.findings.map((f) => f.fields.field).sort(), [
      "long_out_mult",
      "price_cache_write",
    ]);
    const mult = r.findings.find((f) => f.fields.field === "long_out_mult");
    assert.equal(mult.fields.listed, 3);
    assert.equal(mult.fields.catalogue, 1.5);
  });

  it("is quiet within 1%, and loud beyond it (0.126 against 0.125 was seen at LiteLLM)", async () => {
    const within = await run({
      [U.anthropic]: drifted(ANTHROPIC, "$0.125 / MTok", "$0.126 / MTok"),
    });
    assert.deepEqual(within.findings, []);
    const beyond = await run({
      [U.anthropic]: drifted(ANTHROPIC, "$0.125 / MTok", "$0.13 / MTok"),
    });
    assert.ok(beyond.findings.some((f) => f.fields.field === "price_cache_write"));
  });

  it("OpenRouter disagreeing while the vendor agrees is a NOTICE only, never a finding", async () => {
    const r = await run();
    assert.ok(noticeIds(r).includes("cross-check-only"));
    assert.deepEqual(r.findings, []);
  });

  it("OpenRouter AND the vendor disagreeing: the finding stands alone, with no 'cross-check only' for that model", async () => {
    const r = await run({
      [U.openai]: drifted(
        OPENAI,
        "| gpt-6-luna | $0.10 | $0.50 |",
        "| gpt-6-luna | $0.10 | $0.60 |",
      ),
      [U.openrouter]: orEdit("openai/gpt-6-luna", (m) => (m.pricing.completion = "0.0000006")),
    });
    assert.deepEqual(
      r.findings.map((f) => f.fields.model),
      ["gpt-6-luna", "gpt-6-luna"],
      "the output price, and the long-context multiplier it moves",
    );
    assert.ok(r.findings.some((f) => f.fields.field === "price_out"));
    assert.ok(!r.notices.some((n) => n.id === "cross-check-only" && n.model === "gpt-6-luna"));
  });

  it("OpenRouter alone moving never edits anything: a changed price there is a notice, a missing row a notice, a new expiry a notice", async () => {
    const r = await run({
      [U.openrouter]: orEdit("openai/gpt-6.2-sol", (m) => (m.pricing.prompt = "0.000003")),
    });
    assert.deepEqual(r.findings, []);
    assert.ok(r.notices.some((n) => n.id === "cross-check-only" && n.model === "gpt-6.2-sol"));
    const expiring = await run({
      [U.openrouter]: orEdit(
        "anthropic/claude-haiku-5.5",
        (m) => (m.expiration_date = "2027-02-01"),
      ),
    });
    assert.deepEqual(expiring.findings, []);
    assert.ok(
      expiring.notices.some(
        (n) =>
          n.id === "openrouter-expiry-differs" &&
          n.model === "claude-haiku-5-5" &&
          n.date === "2027-02-01",
      ),
    );
    const gone = JSON.parse(OR);
    gone.data = gone.data.filter((m) => m.id !== "openai/gpt-6-luna");
    const r2 = await run({ [U.openrouter]: JSON.stringify(gone) });
    assert.deepEqual(r2.findings, []);
    assert.deepEqual(
      r2.notices.filter((n) => n.id === "cross-check-no-row").map((n) => n.model),
      ["gpt-6-luna"],
    );
  });

  it("OpenRouter's :batch offer of a model is never compared", async () => {
    const r = await run({
      [U.openrouter]: orEdit("openai/gpt-6-luna:batch", (m) => (m.pricing.prompt = "0.5")),
    });
    assert.ok(!r.notices.some((n) => n.model === "gpt-6-luna"));
  });

  it("a catalogue wire id with no row on the vendor page is source-unparsable (a retired or renamed model is an alert)", async () => {
    const lines = ANTHROPIC.split("\n").filter((l) => !l.startsWith("| Claude Opus 5.5 |"));
    const r = await run({ [U.anthropic]: lines.join("\n") });
    const f = one(r, "source-unparsable");
    assert.deepEqual(f.fields, { source: "anthropic-pricing", what: "no row for claude-opus-5-5" });
  });

  it("the catalogue is never edited: it is byte-for-byte what it was, and the check holds no way to write one", async () => {
    const file = join(
      dirname(fileURLToPath(import.meta.url)),
      "..",
      "..",
      "src-tauri",
      "direct_models.json",
    );
    const before = createHash("sha256").update(readFileSync(file)).digest("hex");
    const copy = structuredClone(CATALOGUE);
    await run(
      {
        [U.anthropic]: drifted(
          ANTHROPIC,
          "$10 / MTok |\n| Claude Sonnet 5 |",
          "$12 / MTok |\n| Claude Sonnet 5 |",
        ),
      },
      { catalogue: copy },
    );
    assert.deepEqual(copy, CATALOGUE);
    assert.equal(createHash("sha256").update(readFileSync(file)).digest("hex"), before);
    const here = dirname(fileURLToPath(import.meta.url));
    for (const f of ["drift.mjs", "lib/drift-rules.mjs", "lib/parse-prices.mjs", "lib/terms.mjs"]) {
      const src = readFileSync(join(here, f), "utf8");
      assert.doesNotMatch(
        src,
        /writeFile|appendFile|rename\(|unlink|copyFile|execFile|spawn|child_process/,
        f,
      );
    }
  });
});

describe("a page that was restructured is an alert, never silence", () => {
  const NOTHING = "# Pricing\n\nOur pricing has moved: see the new pricing site.\n";

  it("a page that parses to ZERO rows is a source-unparsable finding, for each of the three vendors", async () => {
    const r = await run({ [U.anthropic]: NOTHING, [U.openai]: NOTHING, [U.gemini]: NOTHING });
    assert.deepEqual(r.findings.map((f) => `${f.id}:${f.provider}`).sort(), [
      "source-unparsable:anthropic",
      "source-unparsable:gemini",
      "source-unparsable:openai",
    ]);
    assert.ok(!ids(r).includes("price-drift"));
  });

  it("a renamed price column is both found as unknown and stops the known price from being read", async () => {
    const r = await run({
      [U.anthropic]: drifted(ANTHROPIC, "| Base input tokens |", "| Input |"),
    });
    assert.equal(r.findings.filter((f) => f.id === "new-price-dimension").length, 4);
    assert.ok(
      r.findings.some(
        (f) => f.id === "source-unparsable" && f.fields.what.startsWith("no in price"),
      ),
    );
  });

  it("a cache-write price the catalogue has and the page no longer shows is source-unparsable, never silence", async () => {
    const r = await run({
      [U.anthropic]: drifted(
        ANTHROPIC,
        "| Claude Fable 5.2 | $8 / MTok | $10 / MTok |",
        "| Claude Fable 5.2 | $8 / MTok | Not available |",
      ),
    });
    const f = r.findings.filter((x) => x.id === "source-unparsable");
    assert.deepEqual(
      f.map((x) => x.fields.what),
      ["no cache write price for claude-fable-5-2"],
    );
    assert.ok(!ids(r).includes("price-drift"));
  });

  it("a dated schedule with no step covering today is source-unparsable, never silence", () => {
    const m = catalogueFor(CATALOGUE, "gemini").models.find((x) => x.wire === "gemini-4-hydrogen");
    const rows = new Map([
      [
        rowKey.gemini(m),
        {
          in: [{ until: "2026-11-01", value: m.price_in }],
          cached: [{ value: m.price_cached_in }],
          out: [{ value: m.price_out }],
          write: null,
        },
      ],
    ]);
    const r = comparePrices({
      provider: "gemini",
      section: { models: [m] },
      parsed: { rows, unparsed: [] },
      pageKey: "gemini-pricing",
      today: TODAY,
      page: {},
    });
    assert.deepEqual(
      r.findings.filter((f) => f.id === "source-unparsable").map((f) => f.fields.what),
      [`no in price for ${m.wire} covers today`],
    );
    assert.ok(!r.findings.some((f) => f.id === "price-drift"));
  });

  it("OpenAI's Standard section moving, and Gemini's Standard tab being renamed, are each found", async () => {
    const o = await run({
      [U.openai]: drifted(OPENAI, "### Standard pricing data", "### Standard prices"),
    });
    assert.equal(one(o, "source-unparsable").fields.what, "no Standard pricing section");
    const g = await run({ [U.gemini]: GEMINI.split('title="Standard"').join('title="Default"') });
    assert.equal(one(g, "source-unparsable").provider, "gemini");
  });

  it("a page answering 404, or ending on another host, is a finding and not a pass", async () => {
    const gone = await run({ [U.openai]: { text: "Not found", status: 404 } });
    assert.equal(one(gone, "source-unparsable").fields.what, "the page answered HTTP 404");
    const moved = await run({ [U.openai]: { url: "https://elsewhere.example/pricing" } });
    assert.equal(one(moved, "source-unparsable").fields.what, "the page now ends on another host");
  });

  it("the final address after redirects is compared with sources.json: a change on the same host is a notice and no finding", async () => {
    const r = await run({ [U.gemini]: { url: "https://aistudio.google.com/docs/pricing" } });
    assert.deepEqual(r.findings, []);
    assert.ok(r.notices.some((n) => n.id === "source-url-changed" && n.provider === "gemini"));
  });
});

describe("a price column or row PM does not know on a model PM lists", () => {
  it("Anthropic: a new priced column is a finding naming the header, once per catalogue model (not for other models)", async () => {
    const r = await run({
      [U.anthropic]: addColumn(ANTHROPIC, "| Claude", "Priority output", "$1 / MTok"),
    });
    const hits = r.findings.filter((f) => f.id === "new-price-dimension");
    assert.equal(hits.length, 4);
    assert.ok(
      hits.every((f) => f.fields.header === "priority-output" && f.provider === "anthropic"),
    );
    assert.deepEqual(hits.map((f) => f.fields.model).sort(), [
      "claude-fable-5-2",
      "claude-haiku-5-5",
      "claude-opus-5-5",
      "claude-sonnet-5-5",
    ]);
  });

  it("OpenAI: a new write column on the Standard table", async () => {
    const r = await run({
      [U.openai]: addColumn(OPENAI, "| gpt-", "Short context 24h write", "$1.00"),
    });
    const hits = r.findings.filter((f) => f.id === "new-price-dimension");
    assert.equal(hits.length, 4);
    assert.ok(hits.every((f) => f.fields.header === "short-context-24h-write"));
  });

  it("a column that holds no price for a catalogue model (only for models PM does not list) is not a finding", async () => {
    const r = await run({
      [U.openai]: addColumn(OPENAI, "| gpt-5", "Short context 24h write", "$1.00"),
    });
    assert.deepEqual(r.findings, []);
  });

  it("Gemini: a new Feature row with a price in a catalogue model's Standard tab", async () => {
    const r = await run({
      [U.gemini]: inSection(
        GEMINI,
        "gemini-4-hydrogen",
        "  * Used to improve our products",
        "  * Thinking price\n  * Free of charge\n  * $3.00\n\n  ---\n\n  * Used to improve our products",
      ),
    });
    const f = one(r, "new-price-dimension");
    assert.deepEqual(f.fields, {
      model: "gemini-4-hydrogen",
      header: "thinking-price",
      source: "gemini-pricing",
    });
  });

  it("Gemini: a renamed price header (say, another currency) is found on every catalogue model", async () => {
    const r = await run({
      [U.gemini]: GEMINI.split("Paid Tier, per 1M tokens in USD").join(
        "Paid Tier, per 1M tokens in EUR",
      ),
    });
    assert.equal(r.findings.filter((f) => f.id === "new-price-dimension").length, 8);
  });

  it("BATCH prices are not compared: PM never calls a batch API, so a batch change cannot raise its costs", async () => {
    const r = await run({
      [U.gemini]: drifted(
        GEMINI,
        "$1.00 through November 30, 2026.",
        "$9.00 through November 30, 2026.",
      ),
      [U.openai]: drifted(
        OPENAI,
        "| gpt-6-luna | $0.05 | $0.25 | $0.005 |",
        "| gpt-6-luna | $0.50 | $2.50 | $0.05 |",
      ),
    });
    assert.deepEqual(r.findings, []);
  });
});

describe("new models, free tier and cache storage", () => {
  it("a model of PM's families that the catalogue lacks is a new-model finding with its prices; known others and Mythos are not", async () => {
    const row =
      "| Claude Sonnet 5.6 | $2 / MTok | $2.50 / MTok | $4 / MTok | $0.10 / MTok | $10 / MTok |";
    const mythos =
      "| Claude Mythos 5.3 | $8 / MTok | $10 / MTok | $16 / MTok | $0.20 / MTok | $40 / MTok |";
    const a = await run({
      [U.anthropic]: drifted(
        ANTHROPIC,
        "| Claude Sonnet 5 |",
        `${row}\n${mythos}\n| Claude Sonnet 5 |`,
      ),
    });
    assert.deepEqual(one(a, "new-model").fields, {
      model: "claude-sonnet-5.6",
      in: 2,
      cached: 0.1,
      out: 10,
      source: "anthropic-pricing",
    });
    const o = await run({
      [U.openai]: drifted(
        OPENAI,
        "| gpt-6.2-sol |",
        "| gpt-6.3-sol | $3.00 | $12.00 | $0.30 | $3.75 | $7.50 | $6.00 | $18.00 | $0.60 | $7.50 | $15.00 |\n| gpt-6.2-sol |",
      ),
    });
    assert.equal(one(o, "new-model").fields.model, "gpt-6.3-sol");
    const g = await run({
      [U.gemini]: drifted(
        GEMINI,
        "## Gemini 4 Hydrogen",
        `${GEMINI.slice(GEMINI.indexOf("## Gemini 4 Hydrogen"), GEMINI.indexOf("## Gemini 3.8 Flash")).replace(/gemini-4-hydrogen/g, "gemini-4-helium")}## Gemini 4 Hydrogen`,
      ),
    });
    assert.equal(one(g, "new-model").fields.model, "gemini-4-helium");
  });

  it("a Gemini model that gains or loses its free tier is a finding (the free-key refusal and the picker read it)", async () => {
    const r = await run({
      [U.gemini]: inSection(
        GEMINI,
        "gemini-4-neon",
        "* Input price\n  * Not available",
        "* Input price\n  * Free of charge",
      ),
    });
    assert.deepEqual(one(r, "free-tier-changed").fields, {
      model: "gemini-4-neon",
      listed: "true",
      catalogue: "false",
      source: "gemini-pricing",
    });
  });

  it("cache STORAGE per hour is a notice, never a finding", async () => {
    const r = await run({
      [U.gemini]: inSection(
        GEMINI,
        "gemini-3.8-flash",
        "$0.50 / 1,000,000 tokens per hour (storage price) through",
        "$0.75 / 1,000,000 tokens per hour (storage price) through",
      ),
    });
    assert.deepEqual(r.findings, []);
    assert.ok(
      r.notices.some((n) => n.id === "cache-storage-changed" && n.model === "gemini-3.8-flash"),
    );
  });
});

describe("dated price steps", () => {
  it("a catalogue step due within 14 days is a notice (exactly 14 is in, 15 is not); a step further out is silent", async () => {
    const due = (today) =>
      run({}, { today }).then((r) =>
        r.notices
          .filter((n) => n.id === "price-step-due")
          .map((n) => `${n.model}@${n.date}`)
          .sort(),
      );
    assert.deepEqual(await due("2026-11-17"), [
      "gemini-4-argon@2026-12-01",
      "gemini-4-carbon@2026-12-01",
    ]);
    assert.deepEqual(await due("2026-11-16"), []);
    assert.deepEqual(await due("2026-12-18"), ["gemini-3.8-flash@2027-01-01"]);
    assert.deepEqual(await due("2026-12-17"), []);
    assert.deepEqual(await due("2026-12-01"), [], "a step that has landed is no longer 'due'");
  });

  it("the vendor price is judged against prices_on(today), not against the row's own price", async () => {
    // The catalogue says Carbon is 2 until 2026-12-01, then 4: both days judged by the right number.
    const carbon = CATALOGUE.providers
      .find((p) => p.provider === "gemini")
      .models.find((m) => m.wire === "gemini-4-carbon");
    assert.equal(pricesOn(carbon, "2026-11-30").price_in, 2);
    assert.equal(pricesOn(carbon, "2026-12-01").price_in, 4);
    // A catalogue that FORGOT the step is price drift once the day comes (and a step-differs before it).
    const forgot = structuredClone(CATALOGUE);
    for (const m of forgot.providers.find((p) => p.provider === "gemini").models)
      m.price_changes = [];
    const after = await run({}, { today: "2026-12-02", catalogue: forgot });
    const drift = after.findings.filter((f) => f.id === "price-drift");
    assert.equal(drift.length, 6, "input, cached and output of Carbon and Argon");
    assert.ok(drift.every((f) => f.fields.listed === 2 * f.fields.catalogue));
    // ...and the 3.8 Flash step that is still ahead is announced by the vendor, for input, cached and
    // output alike, and missing here.
    assert.deepEqual(
      after.findings
        .filter((f) => f.id !== "price-drift")
        .map((f) => `${f.id}@${f.fields.date}:${f.fields.field}`)
        .sort(),
      [
        "price-step-differs@2027-01-01:price_cached_in",
        "price-step-differs@2027-01-01:price_in",
        "price-step-differs@2027-01-01:price_out",
      ],
    );
    const before = await run({}, { today: "2026-11-27", catalogue: forgot });
    assert.ok(
      ids(before).every((id) => id === "price-step-differs"),
      ids(before).join(),
    );
    const steps = new Set(before.findings.map((f) => `${f.fields.model}@${f.fields.date}`));
    assert.deepEqual([...steps].sort(), [
      "gemini-3.8-flash@2027-01-01",
      "gemini-4-argon@2026-12-01",
      "gemini-4-carbon@2026-12-01",
    ]);
    assert.equal(before.findings.length, 9, "each step is found for input, cached and output");
  });

  it("a step changed only on the CACHED or OUTPUT price is found before its day, while the input still matches", async () => {
    // Weekly runs: waiting for price-drift on the day would be up to six days late.
    const output = await run({
      [U.gemini]: inSection(
        GEMINI,
        "gemini-3.8-flash",
        "$7.50 starting January 1, 2027.",
        "$9.00 starting January 1, 2027.",
      ),
    });
    assert.deepEqual(one(output, "price-step-differs").fields, {
      model: "gemini-3.8-flash",
      field: "price_out",
      date: "2027-01-01",
      listed: 9,
      catalogue: 7.5,
      source: "gemini-pricing",
    });
    const cached = await run({
      [U.gemini]: inSection(
        GEMINI,
        "gemini-3.8-flash",
        "$0.15 starting January 1, 2027.",
        "$0.30 starting January 1, 2027.",
      ),
    });
    assert.equal(one(cached, "price-step-differs").fields.field, "price_cached_in");
  });

  it("a catalogue step with NO cached price, against a page that announces one, is a finding and not a crash", async () => {
    // "The vendor starts offering caching" on a dated step: the catalogue's null becomes "none".
    const nocache = structuredClone(CATALOGUE);
    const flash = nocache.providers
      .find((p) => p.provider === "gemini")
      .models.find((m) => m.wire === "gemini-3.8-flash");
    const step = flash.price_changes.find((s) => s.from === "2027-01-01");
    assert.ok(step, "the fixture catalogue has the 3.8 Flash step");
    step.price_cached_in = null;
    const r = await run({}, { catalogue: nocache });
    assert.equal(r.status, "ok");
    assert.deepEqual(one(r, "price-step-differs").fields, {
      model: "gemini-3.8-flash",
      field: "price_cached_in",
      date: "2027-01-01",
      listed: 0.15,
      catalogue: "none",
      source: "gemini-pricing",
    });
  });

  it("a vendor that moves a step's date, or announces a different price for it, is found before the day", async () => {
    const date = await run({
      [U.gemini]: drifted(
        GEMINI,
        "$4.00 starting December 1, 2026.",
        "$4.00 starting December 8, 2026.",
      ),
    });
    assert.deepEqual(
      date.findings
        .map((f) => `${f.id}@${f.fields.date}:${f.fields.listed}/${f.fields.catalogue}`)
        .sort(),
      ["price-step-differs@2026-12-01:none/4", "price-step-differs@2026-12-08:4/none"],
    );
    const price = await run({
      [U.gemini]: inSection(
        GEMINI,
        "gemini-3.8-flash",
        "$1.50 starting January 1, 2027.",
        "$2.50 starting January 1, 2027.",
      ),
    });
    assert.deepEqual(one(price, "price-step-differs").fields, {
      model: "gemini-3.8-flash",
      field: "price_in",
      date: "2027-01-01",
      listed: 2.5,
      catalogue: 1.5,
      source: "gemini-pricing",
    });
  });
});

describe("a page that cannot be fetched is never taken for 'clear'", () => {
  it("one vendor busy after every retry: that provider is UNCHECKED, the others are still judged, and nothing is a finding for it", async () => {
    const r = await run({
      [U.openai]: { text: "busy", status: 503 },
      [U.anthropic]: drifted(
        ANTHROPIC,
        "$0.10 / MTok<sup>2</sup> | $10 / MTok |",
        "$0.10 / MTok<sup>2</sup> | $12 / MTok |",
      ),
    });
    assert.equal(r.status, "ok");
    assert.deepEqual(r.unchecked, ["openai"]);
    assert.ok(r.notices.some((n) => n.id === "source-unreachable" && n.provider === "openai"));
    assert.deepEqual(ids(r), ["price-drift"]);
    assert.equal(
      r.fetch.calls.filter((c) => c.url === U.openai).length,
      4,
      "one try and three retries",
    );
  });

  it("a network error and a bot-wall 403 are the same: unreachable, not a rejected key and not drift", async () => {
    const r = await run({ [U.gemini]: "network", [U.gTerms]: { text: "denied", status: 403 } });
    assert.deepEqual(r.unchecked, ["gemini"]);
    assert.deepEqual(r.findings, []);
  });

  it("every page unreachable: the run is inconclusive and carries no finding", async () => {
    const pages = Object.fromEntries(
      Object.values(PAGES).map((p) => [p.url, { text: "x", status: 502 }]),
    );
    const r = await run(pages);
    assert.equal(r.status, "inconclusive");
    assert.deepEqual(r.findings, []);
    assert.deepEqual([...r.unchecked].sort(), ["anthropic", "gemini", "openai"]);
  });

  it("OpenRouter's list ending on another address or host is not read, and the note says so", async () => {
    const same = await run({ [U.openrouter]: { url: "https://openrouter.ai/api/v2/models" } });
    const off = await run({ [U.openrouter]: { url: "https://elsewhere.example/models" } });
    for (const r of [same, off]) {
      assert.equal(r.status, "ok");
      assert.deepEqual(r.findings, []);
      assert.match(r.note, /OpenRouter's list now answers from another address/);
      assert.ok(!noticeIds(r).includes("cross-check-only"), "a moved list is not judged at all");
    }
    assert.equal((await run()).note, "", "the real address says nothing");
  });

  it("OpenRouter unreachable costs the cross-check only, and says so", async () => {
    const r = await run({ [U.openrouter]: "network" });
    assert.equal(r.status, "ok");
    assert.deepEqual(r.findings, []);
    assert.match(r.note, /OpenRouter could not be read/);
  });

  it("a page address whose host is not in the allow-list is refused outright (nothing is sent)", async () => {
    const bad = structuredClone(SOURCES);
    bad.drift.pages["openai-pricing"].url = "https://evil.example/pricing.md";
    const fetch = driftFetch();
    await assert.rejects(
      runDrift({
        sources: bad,
        catalogue: CATALOGUE,
        today: TODAY,
        deps: { fetch, sleep: NO_SLEEP },
      }),
      /host is not in sources.json/,
    );
    // The exact host, never a prefix of the URL (CodeQL js/incomplete-url-substring-sanitization).
    assert.ok(!fetch.calls.some((c) => new URL(c.url).hostname === "evil.example"));
    const sneaky = structuredClone(SOURCES);
    sneaky.drift.pages["openai-pricing"].url =
      "https://developers.openai.com.evil.example/pricing.md";
    await assert.rejects(
      runDrift({
        sources: sneaky,
        catalogue: CATALOGUE,
        today: TODAY,
        deps: { fetch: driftFetch(), sleep: NO_SLEEP },
      }),
    );
  });

  it("a crash becomes status error with only the error's name, and the exit code goes red", async () => {
    const bad = structuredClone(SOURCES);
    bad.drift.pages["openai-pricing"].url = "https://evil.example/pricing.md";
    const before = process.exitCode;
    const r = await main(
      { GITHUB_OUTPUT: undefined, GITHUB_STEP_SUMMARY: undefined },
      { sources: bad, catalogue: CATALOGUE, today: TODAY, fetch: driftFetch(), sleep: NO_SLEEP },
    );
    const code = process.exitCode;
    process.exitCode = before;
    assert.equal(r.status, "error");
    assert.equal(r.note, "the script crashed (Error)");
    assert.equal(code, 1);
  });
});

describe("terms and retention pages: phrase assertions", () => {
  const phrases = (key) => PAGES[key].phrases.map((p) => p.id);

  it("all present on the committed excerpts: no finding, no notice from them", async () => {
    const r = await run();
    assert.ok(!ids(r).some((id) => id === "phrase-vanished" || id === "covered-model-listed"));
    assert.ok(!noticeIds(r).includes("terms-stamp-changed"));
  });

  it("BOTH Google clauses are pinned, as the page words them", () => {
    const g = PAGES["gemini-terms"].phrases.map((p) => p.text);
    assert.ok(
      g.includes(
        "You may use only Paid Services when making API Clients available to users in the European Economic Area, Switzerland, or the United Kingdom",
      ),
    );
    assert.ok(
      g.some((t) =>
        t.includes("apply to all Services, including Google AI Studio and unpaid quota"),
      ),
    );
    assert.equal(PAGES["gemini-terms"].stamp, "march 23, 2026");
  });

  it("each of Google's clauses vanishing is its own finding naming only the phrase id", async () => {
    const a = await run({
      [U.gTerms]: drifted(G_TERMS, "only Paid Services when", "any Services when"),
    });
    assert.deepEqual(
      a.findings.map((f) => f.fields.phrase),
      ["paid-services-only-eea-uk-ch"],
    );
    const b = await run({ [U.gTerms]: drifted(G_TERMS, "Studio and unpaid quota", "Studio only") });
    assert.deepEqual(
      b.findings.map((f) => f.fields.phrase),
      ["paid-terms-apply-to-unpaid-quota"],
    );
    assert.equal(a.findings[0].provider, "gemini");
  });

  it("the same words with other markup, wrapping, entities and case still match", async () => {
    const t = drifted(
      drifted(
        G_TERMS,
        "**You may use\nonly Paid Services",
        "You   MAY use only\n\n__Paid__ Services",
      ),
      "&quot;",
      '"',
    );
    const r = await run({ [U.gTerms]: t });
    assert.deepEqual(r.findings, []);
  });

  it("a redirect to an unrelated page takes every phrase with it, for all three vendors", async () => {
    const other = "<html><title>Sign in</title><body>Please sign in to continue.</body></html>";
    const r = await run({ [U.aRet]: other, [U.oData]: other, [U.gTerms]: other });
    const got = r.findings.map((f) => f.fields.phrase).sort();
    assert.deepEqual(
      got,
      [
        ...phrases("anthropic-retention"),
        "covered-models-sentence",
        ...phrases("openai-data"),
        ...phrases("gemini-terms"),
      ].sort(),
    );
  });

  it("each Anthropic and OpenAI phrase vanishing is found by its id", async () => {
    const cases = [
      [
        U.aRet,
        A_RET,
        "require 30-day retention",
        "require 7-day retention",
        "covered-models-30-day",
      ],
      [U.aRet, A_RET, "must have data retention enabled", "must have a plan", "retention-400-body"],
      [
        U.aRet,
        A_RET,
        "never used for model training without your express permission",
        "sometimes used for model training",
        "never-trained-on",
      ],
      [
        U.oData,
        O_DATA,
        "retained for up to 30 days",
        "retained for up to 90 days",
        "abuse-logs-30-days",
      ],
      [
        U.oData,
        O_DATA,
        "is not used to train or improve OpenAI models",
        "may be used to train OpenAI models",
        "not-used-to-train",
      ],
    ];
    for (const [url, page, from, to, id] of cases) {
      const r = await run({ [url]: drifted(page, from, to) });
      assert.ok(
        r.findings.some((f) => f.id === "phrase-vanished" && f.fields.phrase === id),
        id,
      );
    }
  });

  it("THE alarm: a model PM lists as not needing retention appearing in Anthropic's Covered Models sentence", async () => {
    const r = await run({
      [U.aRet]: drifted(
        A_RET,
        "Claude Fable 5.2, Claude Mythos 5.2,",
        "Claude Fable 5.2, Claude Opus 5.5, Claude Mythos 5.2,",
      ),
    });
    assert.deepEqual(one(r, "covered-model-listed").fields, {
      model: "claude-opus-5-5",
      source: "anthropic-retention",
    });
    assert.equal(one(r, "covered-model-listed").provider, "anthropic");
  });

  it("a Fable model PM lists dropping out of that sentence is found", async () => {
    const r = await run({
      [U.aRet]: drifted(A_RET, "Claude Fable 5.2, Claude Mythos 5.2,", "Claude Mythos 5.2,"),
    });
    assert.deepEqual(
      r.findings.map((f) => f.fields.phrase),
      ["covered-models-sentence-claude-fable-5-2"],
    );
  });

  it("a changed 'Effective' stamp is a notice only", async () => {
    const r = await run({
      [U.gTerms]: drifted(G_TERMS, "Effective March 23, 2026", "Effective June 1, 2026"),
    });
    assert.deepEqual(r.findings, []);
    assert.ok(r.notices.some((n) => n.id === "terms-stamp-changed" && n.provider === "gemini"));
  });

  it("normalisation and sentence helpers", () => {
    assert.equal(
      normaliseText("A **bold** [link](http://x.example/y) &amp; &quot;q&quot;\n’s"),
      'a bold link & "q" \'s',
    );
    assert.equal(normaliseText("<script>x()</script><p>Hi</p>"), "hi");
    assert.equal(sentenceWith("one. two is here. three.", "is here"), "two is here.");
    assert.equal(sentenceWith("one.", "zzz"), null);
  });
});

describe("what a finding may carry", () => {
  it("every drift finding and notice text is public: one printable line each, no secret shape, filled from sample fields", () => {
    for (const [id, spec] of Object.entries(FINDINGS)) {
      const fields = Object.fromEntries(spec.keys.map((k) => [k, k === "model" ? "a-model" : 1.5]));
      const d = describeFinding({ id, provider: "openai", fields });
      assertPublic(`- **${id}** (${d.severity}): ${d.detail}`);
      assertPublic(`  - Next step: ${d.action}`);
    }
    for (const [id, text] of Object.entries(NOTICES)) assertPublic(`- notice ${id}: ${text}`);
  });

  it("a vendor's own words cannot become a field: markup, links and key shapes are refused", () => {
    const ok = { model: "a", header: "x", source: "anthropic-pricing" };
    const result = (fields) =>
      JSON.stringify({
        status: "ok",
        findings: [{ id: "new-price-dimension", provider: "openai", fields }],
        notices: [],
      });
    assert.equal(parseResult(result(ok), null).ok, true);
    for (const header of [
      "<img src=x>",
      "[a](http://x.example)",
      "@someone",
      "sk-ant-abcdefgh12345678",
      "x".repeat(200),
      "two words",
    ]) {
      assert.equal(parseResult(result({ ...ok, header }), null).ok, false, header);
    }
    assert.equal(parseResult(result({ ...ok, extra: "x" }), null).ok, false);
  });

  it("a multi-provider result needs provider null; a canary leg still accepts only its own provider's findings", () => {
    const body = JSON.stringify({
      status: "ok",
      findings: [
        {
          id: "phrase-vanished",
          provider: "gemini",
          fields: { source: "gemini-terms", phrase: "p" },
        },
      ],
      notices: [
        { id: "price-step-due", provider: "gemini", model: "gemini-4-carbon", date: "2026-12-01" },
      ],
      unchecked: ["openai"],
    });
    assert.equal(parseResult(body, null).ok, true);
    assert.deepEqual(parseResult(body, null).value.unchecked, ["openai"]);
    assert.equal(parseResult(body, "openai").ok, false);
    assert.equal(parseResult(body.replace("openai", "nobody"), null).ok, false);
    assert.equal(
      parseResult(body.replace('"model":"gemini-4-carbon"', '"model":"<b>"'), null).ok,
      false,
    );
    assert.equal(
      parseResult(body.replace('"status":"ok"', '"status":"error"'), null).ok,
      false,
      "findings on a run that did not finish ok",
    );
  });

  it("the summary lines of a run say what was found, per provider, and pass the public-text check", async () => {
    const r = await run({ [U.openai]: { text: "busy", status: 503 } });
    const lines = driftSummaryLines(r, TODAY);
    for (const l of lines) assertPublic(l);
    assert.ok(lines.some((l) => l.includes("Not fully checked this week")));
  });
});

describe("a change that touches a whole page must still reach an issue", () => {
  // Every dollar amount on a page multiplied: a price rise across the vendor, a unit change.
  const scaled = (page, k) => page.replace(/\$(\d+(?:\.\d+)?)/g, (_, n) => `$${Number(n) * k}`);

  it("every price on all three pages up by half: a result the reporter accepts, with a 'read the page' finding per provider", async () => {
    const r = await run({
      [U.anthropic]: scaled(ANTHROPIC, 1.5),
      [U.openai]: scaled(OPENAI, 1.5),
      [U.gemini]: scaled(GEMINI, 1.5),
    });
    assert.equal(r.status, "ok");
    const parsed = parseResult(JSON.stringify(r), null);
    assert.equal(parsed.ok, true, parsed.reason);
    for (const provider of ["anthropic", "openai", "gemini"]) {
      const own = r.findings.filter((f) => f.provider === provider);
      assert.equal(own.length, MAX_PER_PROVIDER + 1, provider);
      assert.ok(
        own.some((f) => f.id === "price-drift"),
        provider,
      );
      assert.ok(
        own.some(
          (f) => f.id === "source-unparsable" && f.fields.what.startsWith("many differences"),
        ),
        provider,
      );
    }
  });

  it("capFindings keeps a bounded, stable set per provider and adds one finding only when it dropped some", () => {
    const many = (provider, n) =>
      Array.from({ length: n }, (_, i) =>
        finding("price-drift", provider, {
          model: `m-${String(i).padStart(2, "0")}`,
          field: "price_in",
          listed: 2,
          catalogue: 1,
          source: "openai-pricing",
        }),
      );
    const out = capFindings([...many("openai", 60).reverse(), ...many("gemini", MAX_PER_PROVIDER)]);
    assert.equal(out.filter((f) => f.provider === "openai").length, MAX_PER_PROVIDER + 1);
    assert.equal(out.filter((f) => f.provider === "gemini").length, MAX_PER_PROVIDER, "no extra");
    assert.deepEqual(
      out
        .filter((f) => f.id === "price-drift" && f.provider === "openai")
        .map((f) => f.fields.model),
      many("openai", MAX_PER_PROVIDER).map((f) => f.fields.model),
      "the first by model name, whatever order they came in",
    );
    assert.ok(out.length <= MAX_FINDINGS);
  });

  it("a page far larger than the real ones is source-unparsable, not worked through", async () => {
    const r = await run({ [U.anthropic]: ANTHROPIC + " ".repeat(MAX_PAGE_CHARS) });
    assert.deepEqual(
      r.findings.map((f) => `${f.id}:${f.provider}:${f.fields.what}`),
      ["source-unparsable:anthropic:the page is far larger than expected"],
    );
  });
});

describe("the long-prompt tier disappearing is found for every vendor", () => {
  it("OpenAI: the long-context price columns renamed away", async () => {
    let page = OPENAI;
    for (const h of [
      "Long context input",
      "Long context output",
      "Long context cached input",
      "Long context 30m write",
    ])
      page = drifted(page, `| ${h} |`, `| ${h.replace("context", "ctx")} |`);
    const r = await run({ [U.openai]: page });
    assert.ok(
      r.findings.some(
        (f) =>
          f.id === "source-unparsable" && f.fields.what === "no long-prompt row for gpt-6-luna",
      ),
      JSON.stringify(r.findings.map((f) => f.fields.what)),
    );
  });

  it("Gemini: the 'prompts > 200k' lines reworded", async () => {
    const page = GEMINI.split("prompts > 200k").join("larger prompts");
    assert.notEqual(page, GEMINI);
    const r = await run({ [U.gemini]: page });
    const pro = CATALOGUE.providers
      .find((p) => p.provider === "gemini")
      .models.find(
        (m) =>
          m.long_context && [m.long_context.in_mult, m.long_context.out_mult].some((x) => x !== 1),
      );
    assert.ok(pro, "the catalogue has a Gemini model with a long-context multiplier");
    assert.ok(
      r.findings.some(
        (f) =>
          f.id === "source-unparsable" && f.fields.what === `no long-prompt row for ${pro.wire}`,
      ),
      JSON.stringify(r.findings.map((f) => f.fields.what)),
    );
  });
});

describe("tests that used to pass for free", () => {
  it("a catalogue model that exists ONLY in OpenAI's Batch table has no Standard row: source-unparsable, never a 50%-off Standard price", async () => {
    // The first "| gpt-6.2-sol |" is the Standard table's; renaming it leaves the Batch table's row alone.
    const r = await run({ [U.openai]: drifted(OPENAI, "| gpt-6.2-sol |", "| gpt-6.2-solx |") });
    assert.ok(
      r.findings.some(
        (f) => f.id === "source-unparsable" && f.fields.what === "no row for gpt-6.2-sol",
      ),
    );
    assert.ok(
      !r.findings.some((f) => f.id === "price-drift" && f.fields.model === "gpt-6.2-sol"),
      "the Batch row's lower price was not read as Standard",
    );
  });

  it("a flat-priced page still shows the OLD price on the day a catalogue step lands (vendors' pages lag): quiet that day, drift the day after", async () => {
    const stepped = structuredClone(CATALOGUE);
    const luna = stepped.providers
      .find((p) => p.provider === "openai")
      .models.find((m) => m.wire === "gpt-6-luna");
    luna.price_changes = [
      { from: "2026-12-01", price_in: 0.2, price_cached_in: 0.02, price_out: 1.0 },
    ];
    const lunaFindings = (r) => r.findings.filter((f) => f.fields.model === "gpt-6-luna");
    assert.deepEqual(lunaFindings(await run({}, { today: "2026-12-01", catalogue: stepped })), []);
    const after = await run({}, { today: "2026-12-02", catalogue: stepped });
    assert.deepEqual(
      lunaFindings(after)
        .map((f) => `${f.id}:${f.fields.field}`)
        .sort(),
      ["price-drift:price_cached_in", "price-drift:price_in", "price-drift:price_out"],
    );
  });

  it("NFKC: full-width letters and a ligature on the page still match the phrase", async () => {
    assert.equal(normaliseText("ＡＰＩ Ｃｌｉｅｎｔｓ ﬁrst"), "api clients first");
    const r = await run({
      [U.gTerms]: drifted(
        G_TERMS,
        "Area, Switzerland, or the United Kingdom.**",
        "Area, Ｓｗｉｔｚｅｒｌａｎｄ, or the United Kingdom.**",
      ),
    });
    assert.ok(
      !r.findings.some((f) => f.id === "phrase-vanished"),
      JSON.stringify(r.findings.map((f) => f.fields.phrase)),
    );
  });

  it("OpenRouter's 'the vendor agreed' is only for a model the vendor's page had a row for", async () => {
    const moved = orEdit("openai/gpt-6-luna", (m) => (m.pricing.prompt = "0.000009"));
    const control = await run({ [U.openrouter]: moved });
    assert.ok(
      control.notices.some((n) => n.id === "cross-check-only" && n.model === "gpt-6-luna"),
      "with a row on the page the notice is raised",
    );
    const r = await run({
      [U.openrouter]: moved,
      [U.openai]: drifted(OPENAI, "| gpt-6-luna |", "| gpt-6-lunax |"),
    });
    assert.ok(
      r.findings.some((f) => f.fields.what === "no row for gpt-6-luna"),
      "the page lost the row",
    );
    assert.ok(
      !r.notices.some((n) => n.id === "cross-check-only" && n.model === "gpt-6-luna"),
      "so nobody says the vendor agreed",
    );
  });

  it("the next generation is a new model too: gpt-7, a Gemini preview suffix and a 4.1 minor", async () => {
    const o = await run({
      [U.openai]: drifted(
        OPENAI,
        "| gpt-6.2-sol |",
        "| gpt-7-sol | $3.00 | $12.00 | $0.30 | $3.75 | $7.50 | $6.00 | $18.00 | $0.60 | $7.50 | $15.00 |\n| gpt-6.2-sol |",
      ),
    });
    assert.equal(one(o, "new-model").fields.model, "gpt-7-sol");
    const two = await run({
      [U.openai]: drifted(
        OPENAI,
        "| gpt-6.2-sol |",
        "| gpt-6-codex-mini | $3.00 | $12.00 | $0.30 | $3.75 | $7.50 | $6.00 | $18.00 | $0.60 | $7.50 | $15.00 |\n| gpt-6.2-sol |",
      ),
    });
    assert.equal(one(two, "new-model").fields.model, "gpt-6-codex-mini", "two suffix parts");
    const hydrogen = GEMINI.slice(
      GEMINI.indexOf("## Gemini 4 Hydrogen"),
      GEMINI.indexOf("## Gemini 3.8 Flash"),
    );
    for (const wire of ["gemini-4-carbon-preview", "gemini-4.1-helium"]) {
      const g = await run({
        [U.gemini]: drifted(
          GEMINI,
          "## Gemini 4 Hydrogen",
          `${hydrogen.replace(/gemini-4-hydrogen/g, wire)}## Gemini 4 Hydrogen`,
        ),
      });
      assert.equal(one(g, "new-model").fields.model, wire);
    }
  });
});
