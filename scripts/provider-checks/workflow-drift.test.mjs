// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The security shape of .github/workflows/provider-drift.yml, as text (zizmor, `just zizmor`, is the
// gate for the general rules). This workflow reads the public web and files issues, and that is all: it
// must never gain a secret, an environment, an OIDC grant, a pull-request trigger, or a way for the job
// that reads the web to post.

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const text = readFileSync(join(root, ".github", "workflows", "provider-drift.yml"), "utf8");
const noComments = text
  .split("\n")
  .filter((l) => !/^\s*#/.test(l))
  .join("\n");

function jobs() {
  const body = noComments.slice(noComments.indexOf("\njobs:\n") + 7);
  const out = {};
  const heads = [...body.matchAll(/^ {2}([a-z][a-z0-9-]*):\n/gm)];
  heads.forEach(
    (m, i) =>
      (out[m[1]] = body.slice(m.index, i + 1 < heads.length ? heads[i + 1].index : undefined)),
  );
  return out;
}
const J = jobs();

describe("provider-drift.yml triggers", () => {
  it("runs on schedule and manual dispatch only, Mondays 06:17 UTC", () => {
    const on = noComments.slice(
      noComments.indexOf("\non:\n"),
      noComments.indexOf("\npermissions:"),
    );
    assert.deepEqual([...on.matchAll(/^ {2}([a-z_]+):/gm)].map((m) => m[1]).sort(), [
      "schedule",
      "workflow_dispatch",
    ]);
    assert.doesNotMatch(noComments, /pull_request|\bpush:|workflow_run|issue_comment/);
    assert.match(noComments, /cron: "17 6 \* \* 1"/);
  });

  it("has workflow-level contents: read and the queue shared with the canary, which never cancels a run", () => {
    const head = noComments.slice(0, noComments.indexOf("\njobs:\n"));
    assert.match(head, /\npermissions:\n {2}contents: read\n/);
    assert.match(head, /concurrency:\n {2}group: provider-checks\n {2}cancel-in-progress: false/);
  });
});

describe("provider-drift.yml holds no secret of any kind", () => {
  it("no secrets, no environment, no id-token, no artifact, no cache, no write scope but issues on the posting jobs", () => {
    assert.doesNotMatch(noComments, /secrets\./);
    assert.doesNotMatch(noComments, /environment:/);
    assert.doesNotMatch(noComments, /id-token/);
    assert.doesNotMatch(noComments, /upload-artifact|actions\/cache|cache:/);
    assert.doesNotMatch(noComments, /set -x|curl -v|--verbose|ACTIONS_STEP_DEBUG/);
    assert.ok(!/contents: write|pull-requests|actions: write|packages/.test(noComments));
  });

  it("the header says what it is and what it does not do", () => {
    assert.match(text, /NO secrets, NO environment and NO `id-token`/);
    assert.match(text, /Batch prices are not compared/);
    assert.match(text, /never edits the catalogue/);
  });
});

describe("provider-drift.yml jobs", () => {
  it("has the one reading job, the one reporter and the self-test, each with a timeout and its own permissions", () => {
    assert.deepEqual(Object.keys(J).sort(), ["drift", "report", "selftest"]);
    for (const [name, job] of Object.entries(J)) {
      assert.match(job, /timeout-minutes: \d+/, name);
      assert.match(job, /\n {4}permissions:\n/, name);
    }
  });

  it("the job that reads the web cannot post; only the two reporter jobs can write issues", () => {
    for (const [name, job] of Object.entries(J)) {
      assert.equal(/issues: write/.test(job), name !== "drift", `issues: write in ${name}`);
    }
    assert.match(J.drift, /permissions:\n {6}contents: read\n/);
  });

  it("no ${{ }} appears inside any run: line, and the only ones in env are the token and the report's own needs", () => {
    for (const [name, job] of Object.entries(J)) {
      for (const line of job.split("\n")) {
        if (/^\s+run:/.test(line)) assert.doesNotMatch(line, /\$\{\{/, `${name}: ${line.trim()}`);
      }
    }
    const exprs = [...noComments.matchAll(/\$\{\{ ([^}]+) \}\}/g)].map((m) => m[1]).sort();
    assert.deepEqual(exprs, [
      "!inputs.selftest",
      "always() && !inputs.selftest",
      "github.token",
      "github.token",
      "inputs.selftest",
      "needs.drift.outputs.result",
      "needs.drift.result",
      "steps.drift.outputs.result",
    ]);
  });

  it("every action is SHA-pinned with a version comment, checkout drops its credentials, setup-node runs the declared major", () => {
    const uses = [...text.matchAll(/^\s*- uses: (\S+)(.*)$/gm)];
    assert.equal(uses.length, 6);
    for (const m of uses) assert.match(m[0], /@[0-9a-f]{40} # v\d/, m[1]);
    assert.equal(
      (text.match(/persist-credentials: false/g) ?? []).length,
      uses.filter((m) => m[1].startsWith("actions/checkout")).length,
    );
    const engines = JSON.parse(readFileSync(join(root, "package.json"), "utf8")).engines.node;
    const major = /(\d+)/.exec(engines)[1];
    for (const m of text.matchAll(/node-version: (\d+)/g)) assert.equal(m[1], major);
  });

  it("copies the pins provider-canary.yml already uses, not new ones", () => {
    const canary = readFileSync(join(root, ".github", "workflows", "provider-canary.yml"), "utf8");
    const pins = (t) =>
      new Set([...t.matchAll(/uses: (\S+@[0-9a-f]{40} # v\S+)/g)].map((m) => m[1]));
    for (const p of pins(text)) assert.ok(pins(canary).has(p), p);
  });

  it("the reporter waits for the reading job, runs when it failed, and reads its result from env", () => {
    assert.match(J.report, /needs: \[drift\]/);
    assert.match(J.report, /if: \$\{\{ always\(\) && !inputs\.selftest \}\}/);
    assert.match(J.report, /RESULT_DRIFT: \$\{\{ needs\.drift\.outputs\.result \}\}/);
    assert.match(J.report, /JOB_DRIFT: \$\{\{ needs\.drift\.result \}\}/);
    assert.match(J.report, /run: node scripts\/provider-checks\/report\.mjs drift/);
  });

  it("the reading job writes one `result` output from its own script, which exists", () => {
    assert.match(J.drift, /outputs:\n {6}result: \$\{\{ steps\.drift\.outputs\.result \}\}/);
    assert.match(J.drift, /run: node scripts\/provider-checks\/drift\.mjs/);
    assert.match(J.drift, /if: \$\{\{ !inputs\.selftest \}\}/);
    assert.ok(existsSync(join(root, "scripts", "provider-checks", "drift.mjs")));
  });
});
