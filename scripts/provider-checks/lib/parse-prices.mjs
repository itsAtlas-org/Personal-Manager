// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Readers for the vendors' own pricing pages (the keyless drift check, drift.mjs). Each takes the page
// text and returns the same shape, so one comparison (lib/drift-rules.mjs) judges all three:
//
//   { rows: Map<key, Row>, unparsed: string[] }
//   Row = { name, in, cached, write, out,   // each null, or a list of { value, from?, until? }
//           long,                           // null, or { in, cached, write, out } as plain numbers
//           longThreshold,                  // tokens, where the page says it
//           free,                           // true / false / null: has a free tier
//           storage,                        // Gemini's cache storage per hour, as a schedule
//           extra }                         // header texts PM does not know that carry a price here
//
// Columns are found by their HEADER TEXT, never by position: a vendor that adds or reorders a column
// must not make one number read as another. A header that is neither a price PM knows nor one it
// knowingly ignores (`known.priced`, `known.ignored` in sources.json) and that holds a price on a row
// is reported in `extra`, because it may be a cost PM is not counting. A page that parses to nothing is
// the caller's finding (a restructured page is an alert, never silence).
//
// The page's text is DATA: nothing here builds a pattern from it, evaluates it or follows a link in it.
//
// Pure Node built-ins, ESM, no dependencies (INVARIANTS.md I-18).

const MONTHS = [
  "january",
  "february",
  "march",
  "april",
  "may",
  "june",
  "july",
  "august",
  "september",
  "october",
  "november",
  "december",
];

/** "November 30, 2026" -> "2026-11-30", or null. */
export function isoFromWords(text) {
  const m = /([A-Za-z]+)\s+(\d{1,2}),\s*(\d{4})/.exec(String(text));
  if (!m) return null;
  const month = MONTHS.indexOf(m[1].toLowerCase());
  if (month < 0) return null;
  const p = (n) => String(n).padStart(2, "0");
  return `${m[3]}-${p(month + 1)}-${p(Number(m[2]))}`;
}

/** The first dollar amount in a cell ("$0.10 / MTok<sup>2</sup>" -> 0.1), or null ("-", "Not available"). */
export function money(cell) {
  const m = /\$\s*([0-9][0-9,]*(?:\.[0-9]+)?)/.exec(String(cell));
  return m ? Number(m[1].replace(/,/g, "")) : null;
}

/** A header text as a plain word, for a finding: "Short context 12h write" -> "short-context-12h-write". */
export function slug(text) {
  return (
    String(text)
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 60) || "none"
  );
}

/** A cell as plain text: tags, template tags and links gone, spaces collapsed. */
export function plain(cell) {
  return String(cell)
    .replace(/\{%[^%]*%\}/g, " ")
    .replace(/<[^>]*>/g, " ")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
}

/** The price a schedule shows on `date` (YYYY-MM-DD), or null. */
export function effective(schedule, date) {
  if (!Array.isArray(schedule)) return null;
  for (const e of schedule) {
    if ((e.from === undefined || e.from <= date) && (e.until === undefined || date <= e.until))
      return e.value;
  }
  return null;
}

/** A price with no date on it: a one-entry schedule. */
const flat = (value) => (value === null ? null : [{ value }]);

/** Markdown tables, each with the heading above it: `{ heading, header: string[], rows: string[][] }`. */
export function mdTables(text) {
  const lines = String(text).split("\n");
  const out = [];
  let heading = "";
  const cells = (line) =>
    line
      .trim()
      .replace(/^\|/, "")
      .replace(/\|$/, "")
      .split("|")
      .map((c) => c.trim());
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (/^#{1,6}\s/.test(line)) heading = line.replace(/^#+\s*/, "").trim();
    if (!line.trim().startsWith("|") || i + 1 >= lines.length) continue;
    if (!/^\|?\s*:?-{2,}/.test(lines[i + 1].trim())) continue;
    const header = cells(line);
    const rows = [];
    let j = i + 2;
    while (j < lines.length && lines[j].trim().startsWith("|")) rows.push(cells(lines[j++]));
    out.push({ heading, header, rows });
    i = j - 1;
  }
  return out;
}

/** Whether a cell holds a price (a dollar amount), as opposed to "-" or "Not available". */
const hasPrice = (cell) => money(cell) !== null;

/**
 * Map each column of a table to what it is: `{ dim }` for a price PM knows, `{ ignored: true }` for
 * one it knowingly does not use, `{ extra: slug }` for any other. `known` is the page's own lists from
 * sources.json: `priced` (header text -> dimension, lower case) and `ignored` (header texts).
 */
function classifyHeaders(header, known) {
  return header.map((h, i) => {
    const key = plain(h).toLowerCase();
    if (i === 0) return { name: true };
    if (Object.hasOwn(known.priced, key)) return { dim: known.priced[key] };
    if (known.ignored.includes(key)) return { ignored: true };
    return { extra: slug(key) };
  });
}

const emptyRow = (name) => ({
  name,
  in: null,
  cached: null,
  write: null,
  out: null,
  long: null,
  longThreshold: null,
  free: null,
  storage: null,
  extra: [],
});

/** Put one table row into `row` through the column map. */
function fillRow(row, cols, cells) {
  const long = {};
  cols.forEach((c, i) => {
    const cell = cells[i] ?? "";
    if (c.dim) {
      const v = money(cell);
      if (c.dim.startsWith("long_")) {
        if (v !== null) long[c.dim.slice(5)] = v;
      } else row[c.dim] = flat(v);
    } else if (c.extra && hasPrice(cell)) row.extra.push(c.extra);
  });
  if (Object.keys(long).length > 0)
    row.long = { in: null, cached: null, write: null, out: null, ...long };
}

/**
 * Anthropic's pricing page (the `.md` twin of the docs page): one table, one row per model, and for
 * Haiku 5.5 two rows by prompt length. Rows are keyed by the lower-case display name.
 */
export function parseAnthropicPricing(text, known) {
  const rows = new Map();
  const unparsed = [];
  const table = mdTables(text).find(
    (t) =>
      t.header.some((h) => /^model$/i.test(plain(h))) && t.header.some((h) => /input/i.test(h)),
  );
  if (!table) return { rows, unparsed: ["no model price table"] };
  const cols = classifyHeaders(table.header, known);
  for (const cells of table.rows) {
    const label = plain(cells[0] ?? "");
    const name = label.replace(/\s*\(.*$/, "").trim();
    if (!name) continue;
    const qualifier = label.slice(name.length).toLowerCase();
    // Haiku's page rows: "(for prompts up to 100,000 tokens)" and "(for prompts over 100,000 tokens)".
    const isLong = qualifier.includes("prompts over");
    const key = name.toLowerCase();
    const row = rows.get(key) ?? emptyRow(name);
    const part = emptyRow(name);
    fillRow(part, cols, cells);
    if (isLong) {
      row.long = {
        in: part.in?.[0].value ?? null,
        cached: part.cached?.[0].value ?? null,
        write: part.write?.[0].value ?? null,
        out: part.out?.[0].value ?? null,
      };
      const m = /([0-9][0-9,]*)\s*tokens/.exec(qualifier);
      if (m) row.longThreshold = Number(m[1].replace(/,/g, ""));
    } else {
      Object.assign(row, { in: part.in, cached: part.cached, write: part.write, out: part.out });
    }
    row.extra.push(...part.extra);
    rows.set(key, row);
  }
  return { rows, unparsed };
}

/**
 * OpenAI's pricing page: only the "Standard" section counts (batch, flex, fast and ultrafast are
 * tiers PM never sends). Its first table has short- and long-context columns, found by header text.
 * Rows are keyed by the model id with any "(<272K context length)" note dropped.
 */
export function parseOpenAIPricing(text, known) {
  const rows = new Map();
  const start = String(text).search(/^#{1,6}\s*Standard pricing data\s*$/m);
  if (start < 0) return { rows, unparsed: ["no Standard pricing section"] };
  // The section runs from the end of its heading line to the next heading of any level.
  const afterHeading = String(text).indexOf("\n", start);
  const rest = afterHeading < 0 ? "" : String(text).slice(afterHeading + 1);
  const next = rest.search(/^#{1,6}\s/m);
  const section = next < 0 ? rest : rest.slice(0, next);
  for (const table of mdTables(`### Standard\n${section}`)) {
    if (!table.header.some((h) => /input/i.test(h))) continue;
    const cols = classifyHeaders(table.header, known);
    for (const cells of table.rows) {
      const label = plain(cells[0] ?? "");
      const name = label.replace(/\s*\(.*$/, "").trim();
      if (!name || rows.has(name)) continue;
      const row = emptyRow(name);
      fillRow(row, cols, cells);
      const longNote = /<\s*([0-9]+)\s*K/i.exec(label);
      if (longNote) row.longThreshold = Number(longNote[1]) * 1000;
      rows.set(name, row);
    }
  }
  return { rows, unparsed: [] };
}

/**
 * One cell of Gemini's price table as a schedule. Lines are read by shape: "through <date>" ends a
 * price, "starting <date>" begins one, "(audio)" is another modality PM does not send, "prompts >" is
 * the long-context tier, "storage price" is the cache storage fee (not a token price).
 */
export function parseGeminiCell(lines) {
  const out = { base: [], high: [], storage: [], highThreshold: null };
  for (const raw of lines) {
    const line = plain(raw);
    if (!line || /^implicit caching/i.test(line)) continue;
    const value = money(line);
    if (value === null) continue;
    if (/\(audio\)/i.test(line)) continue;
    const entry = { value };
    const until = /through\s+([A-Za-z]+\s+\d{1,2},\s*\d{4})/i.exec(line);
    const from = /starting\s+([A-Za-z]+\s+\d{1,2},\s*\d{4})/i.exec(line);
    if (until) entry.until = isoFromWords(until[1]);
    if (from) entry.from = isoFromWords(from[1]);
    if (/storage price/i.test(line)) out.storage.push(entry);
    else if (/prompts\s*>/i.test(line)) {
      out.high.push(entry);
      const t = /prompts\s*>\s*([0-9]+)\s*k/i.exec(line);
      if (t) out.highThreshold = Number(t[1]) * 1000;
    } else out.base.push(entry);
  }
  return out;
}

/** A Gemini section's table: the `{% table %}` of the "Standard" tab, as rows of three items. */
function geminiStandardTable(section) {
  const tab =
    /\{%\s*tab\s+title="Standard"\s*%\}([\s\S]*?)(?=\{%\s*tab\s+title=|\{%\s*\/tabs\s*%\}|$)/.exec(
      section,
    );
  if (!tab) return null;
  const table = /\{%\s*table\s*%\}([\s\S]*?)\{%\s*\/table\s*%\}/.exec(tab[1]);
  if (!table) return null;
  const rows = [];
  for (const block of table[1].split(/^\s*---\s*$/m)) {
    const items = [];
    for (const line of block.split("\n")) {
      const t = line.trim();
      if (t.startsWith("* ")) items.push([t.slice(2)]);
      else if (t && items.length > 0) items[items.length - 1].push(t);
    }
    if (items.length > 0) rows.push(items);
  }
  return rows;
}

/**
 * Gemini's pricing page: a template-tagged markdown page, one `## Name {% id="wire-id" %}` section
 * per model, each with a Standard tab holding a "Feature | Free Tier | Paid Tier" table written as
 * list items. Rows are keyed by the section's id (the wire id). A "Feature" row PM does not know and
 * that holds a price is reported in `extra`. The Batch, Flex and Priority tabs are never read.
 */
export function parseGeminiPricing(text, known) {
  const rows = new Map();
  const unparsed = [];
  const parts = String(text).split(/^## /m).slice(1);
  for (const part of parts) {
    const head = part.split("\n")[0];
    const id = /\{%\s*id="([A-Za-z0-9._-]+)"\s*%\}/.exec(head);
    if (!id) continue;
    const table = geminiStandardTable(part);
    if (!table) continue;
    const row = emptyRow(plain(head));
    const header = table[0]?.map((item) => plain(item.join(" "))) ?? [];
    for (const h of header) {
      const key = h.toLowerCase();
      if (!Object.hasOwn(known.priced, key) && !known.ignored.includes(key) && key !== "feature")
        row.extra.push(slug(key));
    }
    for (const items of table.slice(1)) {
      // A footnote star after a label ("Grounding with Google Search *") is not part of its name.
      const label = plain(items[0].join(" "))
        .replace(/\s*\*+/g, "")
        .toLowerCase();
      const free = plain((items[1] ?? []).join(" ")).toLowerCase();
      const paid = items[2] ?? [];
      const dim = Object.hasOwn(known.rows, label) ? known.rows[label] : null;
      if (dim === null && !known.ignored.includes(label)) {
        if (paid.some((l) => money(l) !== null)) row.extra.push(slug(label));
        continue;
      }
      if (dim === null) continue;
      const cell = parseGeminiCell(paid);
      if (dim === "in" || dim === "out" || dim === "cached") {
        row[dim] = cell.base.length > 0 ? cell.base : null;
        if (cell.high.length > 0) {
          row.long = row.long ?? { in: null, cached: null, write: null, out: null };
          row.long[dim] = cell.high[0].value;
          row.longThreshold = cell.highThreshold ?? row.longThreshold;
        }
        if (dim === "cached" && cell.storage.length > 0) row.storage = cell.storage;
        if (dim === "in") {
          // The free column of the input row says whether there is a free tier at all.
          if (free.includes("free of charge")) row.free = true;
          else if (free.includes("not available")) row.free = false;
        }
      }
    }
    rows.set(id[1], row);
  }
  if (rows.size === 0) unparsed.push("no model sections with a Standard price table");
  return { rows, unparsed };
}

/** OpenRouter's model list as `Map<normalised id, { in, cached, write, out, expiry }>` (USD per million tokens). */
export function parseOpenRouter(text) {
  let data;
  try {
    data = JSON.parse(text)?.data;
  } catch {
    return { rows: new Map(), unparsed: ["not JSON"] };
  }
  if (!Array.isArray(data)) return { rows: new Map(), unparsed: ["no data list"] };
  const per = (v) => {
    const n = Number(v);
    return v === undefined || v === null || v === "" || !Number.isFinite(n)
      ? null
      : Math.round(n * 1e6 * 1e6) / 1e6;
  };
  const rows = new Map();
  for (const m of data) {
    // `:batch`, `:free` and similar are different offers of the same model: never the price PM pays.
    if (!m || typeof m.id !== "string" || m.id.includes(":")) continue;
    const p = m.pricing ?? {};
    rows.set(m.id.replace(/\./g, "-"), {
      in: per(p.prompt),
      out: per(p.completion),
      cached: per(p.input_cache_read),
      write: per(p.input_cache_write),
      expiry: typeof m.expiration_date === "string" ? m.expiration_date : null,
    });
  }
  return { rows, unparsed: rows.size === 0 ? ["no models"] : [] };
}
