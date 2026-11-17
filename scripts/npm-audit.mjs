// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// `just npm-audit` (and pr.yml's `dependencies` job): runs `npm audit` over the whole tree and over
// the shipped tree alone (`--omit=dev`), and fails unless scripts/npm-audit-rules.mjs's `decide`
// passes both reports. The rules and the ACCEPTED list live there. This file is only the command and
// always runs: there is deliberately no "am I the entry point?" guard, which could fail open.
//
// Pure Node built-ins, no dependencies (I-18).

import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { ACCEPTED, decide, readReport } from "./npm-audit-rules.mjs";

function audit(args, label, cwd) {
  const npm = process.platform === "win32" ? "npm.cmd" : "npm";
  const r = spawnSync(npm, ["audit", "--json", ...args], {
    cwd,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    shell: process.platform === "win32",
  });
  if (r.error) throw new Error(`${label}: could not run npm audit: ${r.error.message}`);
  return readReport(r.stdout, label);
}

function main() {
  const root = join(dirname(fileURLToPath(import.meta.url)), "..");
  let full;
  let prod;
  try {
    full = audit([], "npm audit", root);
    prod = audit(["--omit=dev"], "npm audit --omit=dev", root);
  } catch (e) {
    console.error(`✗ npm-audit: ${e.message}`);
    process.exit(1);
  }
  const today = new Date().toISOString().slice(0, 10);
  const { failures, accepted, stale } = decide(full, prod, ACCEPTED, today);
  for (const s of stale) console.warn(`! npm-audit: ${s}`);
  for (const a of accepted) console.log(`· npm-audit: ${a}`);
  if (failures.length > 0) {
    console.error(`✗ npm-audit: ${failures.length} advisory(ies) at moderate or above:`);
    for (const f of failures) console.error(`  - ${f}`);
    console.error(
      "Fix it (npm update / an override), or, if nobody can, add a dated entry to ACCEPTED in scripts/npm-audit-rules.mjs with the decision.",
    );
    process.exit(1);
  }
  console.log(
    `✓ npm-audit: no advisory at moderate or above${accepted.length ? `, apart from ${accepted.length} accepted for the build and test tools only` : ""}`,
  );
}

main();
