// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The security shape of .github/workflows/provider-canary.yml, as text (zizmor, `just zizmor`, is the
// gate for the general rules; these are the ones specific to a workflow that holds provider keys and an
// OIDC grant). The guard against a quiet edit that gives the posting job a secret, the keyed jobs a
// way to post, or a pull request a way to run any of it.

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const text = readFileSync(join(root, ".github", "workflows", "provider-canary.yml"), "utf8");
const noComments = text
  .split("\n")
  .filter((l) => !/^\s*#/.test(l))
  .join("\n");

/** The text of each job, by name. */
function jobs() {
  const body = noComments.slice(noComments.indexOf("\njobs:\n") + 7);
  const out = {};
  const re = /^ {2}([a-z][a-z0-9-]*):\n/gm;
  const heads = [...body.matchAll(re)];
  heads.forEach(
    (m, i) =>
      (out[m[1]] = body.slice(m.index, i + 1 < heads.length ? heads[i + 1].index : undefined)),
  );
  return out;
}
const J = jobs();

describe("provider-canary.yml triggers", () => {
  it("runs on schedule and manual dispatch only: no pull_request, no pull_request_target, no push", () => {
    const on = noComments.slice(
      noComments.indexOf("\non:\n"),
      noComments.indexOf("\npermissions:"),
    );
    assert.deepEqual([...on.matchAll(/^ {2}([a-z_]+):/gm)].map((m) => m[1]).sort(), [
      "schedule",
      "workflow_dispatch",
    ]);
    assert.doesNotMatch(noComments, /pull_request|\bpush:|workflow_run|issue_comment/);
    assert.match(noComments, /cron: "41 5 \* \* 3"/);
  });

  it("has workflow-level contents: read, and the shared queue that never cancels a run", () => {
    const head = noComments.slice(0, noComments.indexOf("\njobs:\n"));
    assert.match(head, /\npermissions:\n {2}contents: read\n/);
    assert.match(head, /concurrency:\n {2}group: provider-checks\n {2}cancel-in-progress: false/);
  });
});

describe("provider-canary.yml jobs", () => {
  it("has the three keyed jobs, one reporter and the self-test, each with a timeout and its own permissions", () => {
    assert.deepEqual(Object.keys(J).sort(), [
      "anthropic",
      "gemini",
      "openai",
      "report",
      "selftest",
    ]);
    for (const [name, job] of Object.entries(J)) {
      assert.match(job, /timeout-minutes: \d+/, name);
      assert.match(job, /\n {4}permissions:\n/, name);
    }
  });

  it("only the Anthropic job has id-token, and only the two reporter jobs can write issues", () => {
    for (const [name, job] of Object.entries(J)) {
      assert.equal(/id-token:/.test(job), name === "anthropic", `id-token in ${name}`);
      assert.equal(
        /issues: write/.test(job),
        name === "report" || name === "selftest",
        `issues: write in ${name}`,
      );
      assert.ok(!/contents: write|pull-requests|actions: write|packages/.test(job), name);
    }
  });

  it("the keyed jobs run on the provider-canary environment; the posting jobs hold no environment and no secret", () => {
    for (const name of ["openai", "gemini", "anthropic"])
      assert.match(J[name], /\n {4}environment: provider-canary\n/, name);
    for (const name of ["report", "selftest"]) {
      assert.doesNotMatch(J[name], /environment:/, name);
      assert.doesNotMatch(J[name], /secrets\./, name);
    }
  });

  it("secrets are bound at STEP env only, never job- or workflow-level, never inside run: text", () => {
    assert.doesNotMatch(
      noComments.slice(0, noComments.indexOf("\njobs:\n")),
      /secrets\./,
      "workflow-level",
    );
    for (const [name, job] of Object.entries(J)) {
      const lines = job.split("\n");
      lines.forEach((line, i) => {
        if (!/secrets\./.test(line)) return;
        assert.match(
          line,
          /^ {10}[A-Z_]+: \$\{\{ secrets\.[A-Z_]+ \}\}$/,
          `${name}: a secret outside a step env: ${line.trim()}`,
        );
        const stepEnv = lines
          .slice(0, i)
          .reverse()
          .find((l) => /^ {8}\S/.test(l));
        assert.match(
          stepEnv,
          /^ {8}env:$/,
          `${name}: ${line.trim()} sits under ${stepEnv?.trim()}`,
        );
      });
    }
    assert.match(J.openai, /OPENAI_CANARY_KEY: \$\{\{ secrets\.OPENAI_CANARY_KEY \}\}/);
    assert.match(J.gemini, /GEMINI_CANARY_KEY: \$\{\{ secrets\.GEMINI_CANARY_KEY \}\}/);
    for (const k of [
      "ANTHROPIC_ORG_ID",
      "ANTHROPIC_FEDERATION_RULE_ID",
      "ANTHROPIC_SERVICE_ACCOUNT_ID",
      "ANTHROPIC_WORKSPACE_ID",
      "ANTHROPIC_CANARY_KEY",
    ]) {
      assert.match(J.anthropic, new RegExp(`${k}: \\$\\{\\{ secrets\\.${k} \\}\\}`));
    }
  });

  it("no ${{ }} appears inside any run: line, and nothing echoes or traces", () => {
    for (const [name, job] of Object.entries(J)) {
      for (const line of job.split("\n")) {
        if (/^\s+run:/.test(line)) assert.doesNotMatch(line, /\$\{\{/, `${name}: ${line.trim()}`);
      }
    }
    assert.doesNotMatch(
      noComments,
      /set -x|curl -v|--verbose|ACTIONS_STEP_DEBUG|upload-artifact|actions\/cache|cache:/,
    );
  });

  it("every action is SHA-pinned with a version comment, checkout drops its credentials, setup-node runs the declared major", () => {
    const uses = [...text.matchAll(/^\s*- uses: (\S+)(.*)$/gm)];
    assert.ok(uses.length >= 10);
    for (const m of uses) assert.match(m[0], /@[0-9a-f]{40} # v\d/, m[1]);
    assert.equal(
      (text.match(/persist-credentials: false/g) ?? []).length,
      uses.filter((m) => m[1].startsWith("actions/checkout")).length,
    );
    const engines = JSON.parse(readFileSync(join(root, "package.json"), "utf8")).engines.node;
    const major = /(\d+)/.exec(engines)[1];
    for (const m of text.matchAll(/node-version: (\d+)/g)) assert.equal(m[1], major);
  });

  it("the reporter waits for all three legs, runs when one failed, and reads results from env, not from the shell text", () => {
    assert.match(J.report, /needs: \[openai, gemini, anthropic\]/);
    assert.match(J.report, /if: \$\{\{ always\(\) && !inputs\.selftest \}\}/);
    for (const p of ["OPENAI", "GEMINI", "ANTHROPIC"]) {
      assert.match(
        J.report,
        new RegExp(`RESULT_${p}: \\$\\{\\{ needs\\.${p.toLowerCase()}\\.outputs\\.result \\}\\}`),
      );
      assert.match(
        J.report,
        new RegExp(`JOB_${p}: \\$\\{\\{ needs\\.${p.toLowerCase()}\\.result \\}\\}`),
      );
    }
    assert.match(J.report, /run: node scripts\/provider-checks\/report\.mjs canary/);
  });

  it("each keyed job writes one `result` output and runs its own script, which exists", () => {
    for (const p of ["openai", "gemini", "anthropic"]) {
      assert.match(J[p], /outputs:\n {6}result: \$\{\{ steps\.canary\.outputs\.result \}\}/);
      assert.match(J[p], new RegExp(`run: node scripts/provider-checks/canary-${p}\\.mjs`));
      assert.ok(existsSync(join(root, "scripts", "provider-checks", `canary-${p}.mjs`)));
      assert.match(J[p], /if: \$\{\{ !inputs\.selftest \}\}/);
    }
    assert.ok(existsSync(join(root, "scripts", "provider-checks", "report.mjs")));
  });
});
