// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The shared issue reporter of the provider checks: `node report.mjs canary` (after the keyed jobs),
// `node report.mjs drift` (after the keyless drift job) or `node report.mjs selftest`. It holds no
// secret and posts with the job's own read/issue token through the preinstalled `gh`. One issue per
// (provider, check kind), found by labels through the REST list (strongly consistent; `gh issue list
// --search` can lag and cause duplicates) and confirmed by a hidden body marker.
//
//   - status `ok`, no findings, the job's own result `success`, an open issue  -> comment, close.
//   - findings, no open issue                                                   -> create it.
//   - findings, an open issue with the SAME fingerprint                         -> nothing (no weekly spam).
//   - findings, an open issue with a DIFFERENT fingerprint                      -> edit it and comment once.
//   - findings, no open issue, the newest closed one was closed "not planned" with the SAME fingerprint
//                                                                                -> nothing (Bobby
//     acknowledged it; a different fingerprint is new news and opens a fresh issue). The reporter
//     itself closes with reason `completed`, so its own closes never look acknowledged.
//   - anything else (`skipped`, `inconclusive`, `error`, a missing or invalid output, a job result of
//     failure, cancelled or skipped)                                            -> NO issue is touched,
//     and never closed: not knowing is not "clear".
// Notices go to the job summary only; only a FINDING opens an issue.
//
// WHITELIST: a result is accepted only if it validates against lib/findings.mjs, and every line of an
// issue body or summary is printable text with no secret shape. Issue bodies, comments and summaries are
// public: never a key, an organisation, workspace or account id, a request id, a balance or spend
// figure, or a raw reply.
//
// Pure Node built-ins, ESM, no dependencies (INVARIANTS.md I-18).

import { execFileSync } from "node:child_process";
import {
  describe,
  fingerprint,
  NOTICES,
  parseResult,
  PROVIDER_NAMES,
  PROVIDERS,
  finding,
} from "./lib/findings.mjs";
import { loadSources } from "./lib/data.mjs";
import { log, summary } from "./lib/log.mjs";
import { looksSecret } from "./lib/redact.mjs";
import { isEntry } from "./lib/leg.mjs";

const ASSIGNEE = "BBYU26";
const LINE = /^[\x20-\x7e]{0,500}$/;

/** `gh` for real: arguments in, stdout out, an optional stdin. A non-zero exit throws (the job goes red). */
export function realGh(args, { input } = {}) {
  return execFileSync("gh", args, { input, encoding: "utf8", maxBuffer: 8 * 1024 * 1024 });
}

/** DD-MM-YYYY in UTC. */
export function ddmmyyyy(date) {
  const p = (n) => String(n).padStart(2, "0");
  return `${p(date.getUTCDate())}-${p(date.getUTCMonth() + 1)}-${date.getUTCFullYear()}`;
}

const markerOf = (provider, kind) => `<!-- pm-provider-check:${provider}:${kind} -->`;

/** The issue body: fixed text, the fingerprint and marker, and the template sentence of each finding. */
export function issueBody({ provider, kind, findings, date, urls = {} }) {
  const lines = [
    markerOf(provider, kind),
    `<!-- fp:${fingerprint(findings)} -->`,
    `The weekly provider ${kind} found something that changed on ${PROVIDER_NAMES[provider]}'s side. Checked ${date} (UTC).`,
    "",
    "### What it found",
  ];
  for (const f of findings) {
    const d = describe(f);
    lines.push(`- **${f.id}** (${d.severity}): ${d.detail}`);
    // The drift check names the page a finding came from; the address is the one PM's own
    // sources.json holds (never text from a reply), so a reader can open it.
    const url =
      f.fields?.source && Object.hasOwn(urls, f.fields.source) ? urls[f.fields.source] : undefined;
    if (url) lines.push(`  - Page: ${url}`);
    lines.push(`  - Next step: ${d.action}`);
  }
  // The self-test belongs to both workflows and has no file of its own.
  const opener =
    kind === "selftest"
      ? "the provider-check workflows' self-test"
      : `\`.github/workflows/provider-${kind}.yml\``;
  lines.push(
    "",
    `This issue is opened and closed by ${opener}. It closes itself when a later run finds nothing, and a run that cannot finish never closes it. Nothing here is a key, an account detail or a vendor reply.`,
  );
  return lines.join("\n");
}

/** Throw if any line of text carries a secret shape or a character a template never makes. */
export function assertPublic(text) {
  for (const line of String(text).split("\n")) {
    if (!LINE.test(line) || looksSecret(line))
      throw new Error("report: a line did not pass the public-text whitelist");
  }
}

/** The labels of one (provider, kind) issue. */
export const labelsFor = (provider, kind) => [
  "provider-check",
  `provider-check:${provider}`,
  `provider-check:${kind}`,
];

/** Issues of one (provider, kind) in `state`, newest first: REST list by labels, confirmed by the body marker. */
function listMarked({ gh, repo, provider, kind, state }) {
  const labels = labelsFor(provider, kind).slice(1).join(",");
  const out = gh(["api", `repos/${repo}/issues?labels=${labels}&state=${state}&per_page=5`]);
  let list;
  try {
    list = JSON.parse(out);
  } catch {
    throw new Error("report: the issue list was not JSON");
  }
  if (!Array.isArray(list)) throw new Error("report: the issue list was not a list");
  return list.filter(
    (i) =>
      i &&
      !i.pull_request &&
      typeof i.body === "string" &&
      i.body.includes(markerOf(provider, kind)),
  );
}

/** The open issue for (provider, kind), or null. */
export const findOpen = (a) => listMarked({ ...a, state: "open" })[0] ?? null;

/** The newest closed issue for (provider, kind), or null (the list is newest first). */
export const findLatestClosed = (a) => listMarked({ ...a, state: "closed" })[0] ?? null;

const labelsEnsured = new Set();
function ensureLabels({ gh, repo, provider, kind }) {
  for (const label of labelsFor(provider, kind)) {
    if (labelsEnsured.has(`${repo}:${label}`)) continue;
    gh([
      "label",
      "create",
      label,
      "--repo",
      repo,
      "--force",
      "--color",
      "5319e7",
      "--description",
      "Opened by the weekly provider checks",
    ]);
    labelsEnsured.add(`${repo}:${label}`);
  }
}

// The fingerprint ignores counts and codes but not the vendor's listed price (lib/findings.mjs): a price
// that moves again is news, and an issue closed as "not planned" for the old number must not hide it.
/** Bring one (provider, kind) issue in line with `findings` (an `ok` leg with a successful job). */
export function sync({ gh, repo, provider, kind, findings, date, urls }) {
  const open = findOpen({ gh, repo, provider, kind });
  if (findings.length === 0) {
    if (!open) return "clear (no issue open)";
    gh(["issue", "comment", String(open.number), "--repo", repo, "--body-file", "-"], {
      input: `Clear as of ${date}.`,
    });
    gh(["issue", "close", String(open.number), "--repo", repo, "--reason", "completed"]);
    return "clear (issue closed)";
  }
  const body = issueBody({ provider, kind, findings, date, urls });
  assertPublic(body);
  if (!open) {
    // Closed as "not planned" with these same findings means Bobby chose to live with them: without this
    // the same issue would come back every week and he could not make it stop.
    const closed = findLatestClosed({ gh, repo, provider, kind });
    if (
      closed?.state_reason === "not_planned" &&
      closed.body.includes(`<!-- fp:${fingerprint(findings)} -->`)
    )
      return "acknowledged (closed as not planned), nothing touched";
    ensureLabels({ gh, repo, provider, kind });
    const args = [
      "issue",
      "create",
      "--repo",
      repo,
      "--title",
      `Provider check (${kind}): ${PROVIDER_NAMES[provider]}`,
      "--body-file",
      "-",
      "--assignee",
      ASSIGNEE,
    ];
    for (const l of labelsFor(provider, kind)) args.push("--label", l);
    gh(args, { input: body });
    return "issue opened";
  }
  if (open.body.includes(`<!-- fp:${fingerprint(findings)} -->`))
    return "issue already open, unchanged";
  gh(["issue", "edit", String(open.number), "--repo", repo, "--body-file", "-"], { input: body });
  gh(["issue", "comment", String(open.number), "--repo", repo, "--body-file", "-"], {
    input: `Changed as of ${date}: the set of problems is different. The description above is current.`,
  });
  return "issue updated";
}

/** The canary report over the three legs' outputs. Returns the lines written to the summary. */
export function reportCanary({ env, gh, now = new Date(), write = summary }) {
  const repo = env.GITHUB_REPOSITORY;
  if (!repo) throw new Error("report: GITHUB_REPOSITORY is not set");
  const date = ddmmyyyy(now);
  const lines = [`## Provider canary, ${date}`];
  let working = 0; // legs that gave a usable result other than "skipped"
  for (const provider of PROVIDERS) {
    const key = provider.toUpperCase();
    const job = env[`JOB_${key}`] ?? "";
    const parsed = parseResult(env[`RESULT_${key}`], provider);
    lines.push("", `### ${PROVIDER_NAMES[provider]}`);
    if (!parsed.ok) {
      lines.push(
        `No usable result (${parsed.reason}); job ${job || "unknown"}. No issue was touched.`,
      );
      continue;
    }
    const r = parsed.value;
    if (r.status !== "skipped") working += 1;
    lines.push(`Status: ${r.status}${r.note ? ` (${r.note})` : ""}; job ${job || "unknown"}.`);
    for (const f of r.findings) lines.push(`- FINDING ${f.id}: ${describe(f).detail}`);
    for (const n of r.notices) lines.push(`- notice ${n.id}: ${NOTICES[n.id]}`);
    if (job !== "success" || r.status !== "ok") {
      lines.push('No issue was touched: a leg that did not finish ok is never "clear".');
      continue;
    }
    lines.push(
      `Issue: ${sync({ gh, repo, provider, kind: "canary", findings: r.findings, date })}.`,
    );
    log({ provider: "report", step: `sync_${provider}`, ok: true });
  }
  // Nothing was checked at all: every leg skipped or gave no result (no secrets yet, or the environment
  // was renamed or emptied). A green run would look healthy while the canary watches nothing, so the
  // report step fails and the line says why. One leg alone skipping (Claude, say) stays quiet.
  const allSkipped = working === 0;
  if (allSkipped)
    lines.push(
      "",
      "NOTHING WAS CHECKED: every leg was skipped or gave no result. Check the provider-canary environment and its secrets.",
    );
  write(lines, env);
  return Object.assign(lines, { allSkipped });
}

/**
 * The drift report: one job output covers every provider, so it is filed one provider at a time.
 * Same rules as the canary: only a result that validates, with status `ok` and a successful job, can
 * open, change or close an issue; a provider the run could not fully check (`unchecked`) is left alone.
 * Notices go to the summary only. `urls` is source key -> address, from sources.json.
 */
export function reportDrift({ env, gh, urls = {}, now = new Date(), write = summary }) {
  const repo = env.GITHUB_REPOSITORY;
  if (!repo) throw new Error("report: GITHUB_REPOSITORY is not set");
  const date = ddmmyyyy(now);
  const job = env.JOB_DRIFT ?? "";
  const lines = [`## Provider drift check, ${date}`];
  const parsed = parseResult(env.RESULT_DRIFT, null);
  if (!parsed.ok) {
    // A run that cannot say what it found must not look green: the job goes red and GitHub emails.
    lines.push(
      `No usable result (${parsed.reason}); job ${job || "unknown"}. No issue was touched.`,
      "",
      "NOTHING WAS REPORTED: the drift result could not be read. Open the drift job's log.",
    );
    write(lines, env);
    return Object.assign(lines, { failed: true });
  }
  const r = parsed.value;
  lines.push(`Status: ${r.status}${r.note ? ` (${r.note})` : ""}; job ${job || "unknown"}.`);
  const finished = job === "success" && r.status === "ok";
  for (const provider of PROVIDERS) {
    lines.push("", `### ${PROVIDER_NAMES[provider]}`);
    const own = r.findings.filter((f) => f.provider === provider);
    for (const f of own) lines.push(`- FINDING ${f.id}: ${describe(f).detail}`);
    for (const n of r.notices.filter((x) => x.provider === provider)) {
      const who = [n.model, n.date].filter(Boolean).join(" ");
      lines.push(`- notice ${n.id}${who ? ` (${who})` : ""}: ${NOTICES[n.id]}`);
    }
    if (!finished) {
      lines.push('No issue was touched: a run that did not finish ok is never "clear".');
      continue;
    }
    // A provider with one page unreachable (its terms page, say) is not fully known, so it is never
    // CLEARED: but a real finding from the pages that did answer (a price that moved) still opens or
    // updates its issue, or a blocked sibling page would hide the change that costs money.
    if (r.unchecked.includes(provider) && own.length === 0) {
      lines.push("Not fully checked this week, so its issue was left alone.");
      continue;
    }
    lines.push(`Issue: ${sync({ gh, repo, provider, kind: "drift", findings: own, date, urls })}.`);
    log({ provider: "report", step: `sync_${provider}`, ok: true });
  }
  // Every provider unchecked (the runner is blocked everywhere, say): nothing was read, and a green run
  // would look healthy for months. As the canary does, say so and fail.
  const nothingChecked = r.status === "inconclusive" && r.unchecked.length === PROVIDERS.length;
  if (nothingChecked)
    lines.push(
      "",
      "NOTHING WAS CHECKED: every provider's pages were unreachable this week. Open the drift job's log.",
    );
  write(lines, env);
  return Object.assign(lines, { failed: nothingChecked });
}

/** The self-test: file a synthetic issue, prove the REST lookup finds it, close it. No keys. */
export function reportSelftest({ env, gh, now = new Date(), write = summary }) {
  const repo = env.GITHUB_REPOSITORY;
  if (!repo) throw new Error("report: GITHUB_REPOSITORY is not set");
  const date = ddmmyyyy(now);
  const provider = "openai";
  const kind = "selftest";
  const findings = [finding("model-missing", provider, { model: "selftest-not-a-real-model" })];
  const first = sync({ gh, repo, provider, kind, findings, date });
  const found = findOpen({ gh, repo, provider, kind });
  if (!found) throw new Error("report: the issue just filed was not found by the REST list");
  // The same findings again must change nothing (de-duplication), then a clear closes it.
  const again = sync({ gh, repo, provider, kind, findings, date });
  const closed = sync({ gh, repo, provider, kind, findings: [], date });
  const lines = [
    `## Provider checks self-test, ${date}`,
    `Filed: ${first}. Second run: ${again}. Cleared: ${closed}.`,
  ];
  write(lines, env);
  return lines;
}

if (isEntry(import.meta.url)) {
  const mode = process.argv[2];
  if (mode === "canary") {
    const lines = reportCanary({ env: process.env, gh: realGh });
    if (lines.allSkipped) process.exitCode = 1;
  } else if (mode === "drift") {
    const pages = loadSources().drift.pages;
    const urls = Object.fromEntries(Object.entries(pages).map(([k, p]) => [k, p.url]));
    if (reportDrift({ env: process.env, gh: realGh, urls }).failed) process.exitCode = 1;
  } else if (mode === "selftest") reportSelftest({ env: process.env, gh: realGh });
  else {
    log({ provider: "report", msg: "usage: report.mjs canary|drift|selftest" });
    process.exitCode = 2;
  }
}
