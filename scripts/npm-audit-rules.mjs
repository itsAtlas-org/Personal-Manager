// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The JS dependency CVE gate's rules and its accepted list, kept apart from the command
// (scripts/npm-audit.mjs, `just npm-audit`, pr.yml's `dependencies` job) so that the command never
// needs an "am I the entry point?" guard: such a guard can fail open (a symlinked path, a Windows
// path form) and skip the audit with exit 0. The gate fails on any npm
// advisory at `moderate` or above, as `npm audit --audit-level=moderate` did: moderate, not high, so
// a moderate-rated sanitizer or prototype-pollution advisory in the render path can't pass green
// (L-7).
//
// One narrow way out, for an advisory nobody can fix yet: an entry in ACCEPTED below. It names one
// advisory (its GHSA id) and the package it is against, says why and who decided, and carries an
// end date. The advisory is let through only while BOTH hold:
//   - today (UTC) is on or before `until`; after that it fails again, so someone looks again;
//   - it does not reach the shipped app: a second audit of the production tree alone
//     (`npm audit --omit=dev`) must not report it. If the package ever becomes a dependency of
//     something PM ships, the gate fails with no edit needed.
// Every other advisory fails exactly as before. An entry whose advisory no longer appears (a fixed
// version landed, or the advisory was withdrawn) is reported as stale, a warning, so it gets removed.
// A report npm couldn't produce (no network, a broken lockfile) fails; it never reads as clean.
//
// Pure Node built-ins, no dependencies (I-18).

// Empty since 2026-11-24: picomatch's GHSA-fc5q-39xc-vjpf (Bobby's exception of 2026-11-17, #1121) was withdrawn by
// GitHub on 2026-11-18, so npm audit no longer reports it and the stale-entry warning asked for its removal.
/** Advisories accepted for now. Adding one is the decision; state why, who, and until when. */
export const ACCEPTED = [];

const LEVELS = { info: 0, low: 1, moderate: 2, high: 3, critical: 4 };
const FLOOR = LEVELS.moderate;
const GHSA = /^GHSA(-[23456789cfghjmpqrvwx]{4}){3}$/;
const DAY = /^\d{4}-\d{2}-\d{2}$/;

/** The GHSA id in an advisory's URL, or null. */
export function ghsaOf(url) {
  const m = /\/(GHSA-[a-z0-9]{4}-[a-z0-9]{4}-[a-z0-9]{4})\b/i.exec(String(url ?? ""));
  return m ? m[1] : null;
}

/** Parses `npm audit --json` output, or throws saying why it can't be trusted as a report. */
export function readReport(stdout, label) {
  let report;
  try {
    report = JSON.parse(stdout);
  } catch {
    throw new Error(
      `${label}: npm audit printed no JSON report (no network, or a broken lockfile?)`,
    );
  }
  if (report === null || typeof report !== "object" || Array.isArray(report))
    throw new Error(`${label}: npm audit's output is not a report`);
  if (report.error) {
    const why = report.error.summary ?? report.error.code ?? JSON.stringify(report.error);
    throw new Error(`${label}: npm audit failed: ${why}`);
  }
  if (typeof report.vulnerabilities !== "object" || report.vulnerabilities === null)
    throw new Error(`${label}: npm audit's report has no vulnerabilities section`);
  return report;
}

/** The key of one advisory against one package. An advisory (one GHSA id) can list several
 *  packages, and accepting it for one must never hide another, whatever order npm prints them in. */
export const keyOf = (id, pkg) => `${id}|${pkg}`;

/** Every (advisory, package) pair in one report, by {@link keyOf}. A package listed only because it
 *  depends on a vulnerable one (its `via` holds names, not advisories) adds nothing: the advisory is
 *  counted where it is. */
export function advisories(report) {
  const out = new Map();
  for (const [name, vuln] of Object.entries(report.vulnerabilities ?? {})) {
    for (const via of vuln?.via ?? []) {
      if (via === null || typeof via !== "object") continue;
      const id = ghsaOf(via.url) ?? `npm-advisory-${via.source ?? "unknown"}`;
      const pkg = String(via.name ?? name);
      out.set(keyOf(id, pkg), {
        id,
        package: pkg,
        severity: String(via.severity ?? vuln.severity ?? "critical"),
        title: String(via.title ?? ""),
        url: String(via.url ?? ""),
      });
    }
  }
  return out;
}

/** What is wrong with an ACCEPTED entry, or null. */
export function entryProblem(e) {
  if (e === null || typeof e !== "object") return "is not an object";
  if (typeof e.id !== "string" || !GHSA.test(e.id))
    return `has id ${JSON.stringify(e.id)}, not a GHSA id`;
  if (typeof e.package !== "string" || e.package.trim() === "") return "names no package";
  if (
    typeof e.until !== "string" ||
    !DAY.test(e.until) ||
    Number.isNaN(Date.parse(`${e.until}T00:00:00Z`))
  )
    return `has until ${JSON.stringify(e.until)}, not a YYYY-MM-DD date`;
  if (typeof e.why !== "string" || e.why.trim().length < 20) return "does not say why";
  if (typeof e.decided !== "string" || e.decided.trim() === "") return "does not say who decided";
  return null;
}

/**
 * The gate's decision. `full` and `prod` are the reports of the whole tree and of the shipped tree
 * alone; `today` is a UTC YYYY-MM-DD date.
 * Returns { failures: string[], accepted: string[], stale: string[] }; failures empty means pass.
 */
export function decide(full, prod, accepted, today) {
  const failures = [];
  const ok = [];
  const byId = new Map();
  for (const e of accepted) {
    const problem = entryProblem(e);
    if (problem) {
      failures.push(`ACCEPTED entry ${JSON.stringify(e?.id ?? e)} ${problem}`);
      continue;
    }
    if (byId.has(e.id)) {
      failures.push(`ACCEPTED lists ${e.id} twice`);
      continue;
    }
    byId.set(e.id, e);
  }
  const found = advisories(full);
  const shipped = advisories(prod);
  for (const a of found.values()) {
    const level = LEVELS[a.severity] ?? LEVELS.critical;
    if (level < FLOOR) continue;
    const say = `${a.id} (${a.package}, ${a.severity}): ${a.title}${a.url ? ` ${a.url}` : ""}`;
    const e = byId.get(a.id);
    const key = keyOf(a.id, a.package);
    if (!e) failures.push(say);
    else if (e.package !== a.package)
      failures.push(`${say}: accepted for ${e.package}, not ${a.package}`);
    else if (today > e.until)
      failures.push(`${say}: its acceptance ended on ${e.until}; look again`);
    else if (shipped.has(key))
      failures.push(`${say}: it now reaches the shipped app, so its acceptance no longer holds`);
    else
      ok.push(
        `${a.id} (${a.package}, ${a.severity}): accepted until ${e.until}, build and test tools only. ${e.decided}.`,
      );
  }
  // An advisory in the shipped tree is in the whole tree too; this catches a report that disagrees.
  for (const a of shipped.values()) {
    if ((LEVELS[a.severity] ?? LEVELS.critical) >= FLOOR && !found.has(keyOf(a.id, a.package)))
      failures.push(
        `${a.id} (${a.package}, ${a.severity}): in the shipped tree's report only: ${a.title}`,
      );
  }
  const stale = [...byId.values()]
    .filter((e) => !found.has(keyOf(e.id, e.package)))
    .map(
      (e) =>
        `${e.id} (${e.package}) is accepted in scripts/npm-audit-rules.mjs but no longer reported: remove the entry`,
    );
  return { failures, accepted: ok, stale };
}
