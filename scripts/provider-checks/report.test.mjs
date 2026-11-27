// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The issue reporter against a fake `gh` that keeps a little issue tracker and records every call.
// The rule under test: only an `ok` leg whose job succeeded may open, change or close an issue;
// anything else, and anything it cannot read, leaves every issue exactly as it was.

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { finding, makeResult, notice } from "./lib/findings.mjs";
import { assertPublic, ddmmyyyy, issueBody, reportCanary, reportSelftest } from "./report.mjs";
import { fakeGh, tracker } from "./testkit.mjs";

const REPO = "itsAtlas-org/Personal-Manager";
const NOW = new Date(Date.UTC(2026, 10, 27, 5, 41));
const x = (n) => "a1B2c3D4e5".repeat(Math.ceil(n / 10)).slice(0, n);

const mutating = (gh) =>
  gh.calls.filter(
    (c) =>
      (c.args[0] === "issue" && ["create", "edit", "comment", "close"].includes(c.args[1])) ||
      c.args[0] === "label",
  );
const result = (status, extra = {}) => JSON.stringify(makeResult(status, extra));
const env = (parts) => ({
  GITHUB_REPOSITORY: REPO,
  RESULT_OPENAI: result("ok"),
  RESULT_GEMINI: result("ok"),
  RESULT_ANTHROPIC: result("ok"),
  JOB_OPENAI: "success",
  JOB_GEMINI: "success",
  JOB_ANTHROPIC: "success",
  ...parts,
});
const F = (provider = "openai", model = "gpt-6-luna") =>
  finding("model-missing", provider, { model });
const openBody = (provider, findings) =>
  issueBody({ provider, kind: "canary", findings, date: "20-11-2026" });
const run = (e, gh) => reportCanary({ env: e, gh, now: NOW, write: () => {} });

describe("an ok leg", () => {
  it("with no findings and no open issue touches nothing, and looks only at the REST list", () => {
    const gh = tracker();
    run(env({}), gh);
    assert.deepEqual(mutating(gh), []);
    assert.equal(gh.calls.length, 3, "one lookup per provider");
    for (const c of gh.calls) {
      assert.equal(c.args[0], "api");
      assert.match(
        c.args[1],
        /^repos\/itsAtlas-org\/Personal-Manager\/issues\?labels=provider-check:(openai|gemini|anthropic),provider-check:canary&state=open&per_page=5$/,
      );
      assert.ok(
        !c.args.includes("--search") && !c.args.includes("list"),
        "never the lagging search index",
      );
    }
  });

  it("with findings and no open issue creates ONE issue: labels, assignee, marker, fingerprint, DD-MM-YYYY", () => {
    const gh = tracker();
    run(
      env({ RESULT_ANTHROPIC: result("ok", { findings: [F("anthropic", "claude-haiku-5-5")] }) }),
      gh,
    );
    const create = gh.calls.filter((c) => c.args[1] === "create" && c.args[0] === "issue");
    assert.equal(create.length, 1);
    const a = create[0].args;
    assert.equal(a[a.indexOf("--title") + 1], "Provider check (canary): Anthropic");
    assert.equal(a[a.indexOf("--assignee") + 1], "BBYU26");
    assert.deepEqual(
      a.flatMap((v, i) => (a[i - 1] === "--label" ? [v] : [])),
      ["provider-check", "provider-check:anthropic", "provider-check:canary"],
    );
    assert.match(create[0].input, /<!-- pm-provider-check:anthropic:canary -->/);
    assert.match(create[0].input, /<!-- fp:[0-9a-f]{8} -->/);
    assert.match(create[0].input, /Checked 27-11-2026 \(UTC\)/);
    assert.match(create[0].input, /claude-haiku-5-5/);
    const labels = gh.calls.filter((c) => c.args[0] === "label");
    assert.equal(labels.length, 3);
    assert.ok(
      labels.every((c) => c.args.includes("--force")),
      "labels are created idempotently",
    );
    assertPublic(create[0].input);
  });

  it("an open issue with the SAME fingerprint is left alone (no weekly spam)", () => {
    const gh = tracker([{ provider: "openai", body: openBody("openai", [F()]) }]);
    run(env({ RESULT_OPENAI: result("ok", { findings: [F()] }) }), gh);
    assert.deepEqual(mutating(gh), []);
  });

  it("no open issue, but the newest closed one was closed NOT PLANNED with the same findings: nothing is touched", () => {
    const gh = tracker(
      [],
      [{ provider: "openai", reason: "not_planned", body: openBody("openai", [F()]) }],
    );
    const lines = run(env({ RESULT_OPENAI: result("ok", { findings: [F()] }) }), gh);
    assert.deepEqual(mutating(gh), []);
    assert.ok(lines.some((l) => /acknowledged \(closed as not planned\)/.test(l)));
    const lookups = gh.calls.filter((c) => /state=closed&per_page=5$/.test(c.args[1]));
    assert.equal(lookups.length, 1, "one closed-issue lookup, by labels, small page");
  });

  it("a DIFFERENT fingerprint after a not-planned close opens a new issue", () => {
    const gh = tracker(
      [],
      [{ provider: "openai", reason: "not_planned", body: openBody("openai", [F()]) }],
    );
    run(env({ RESULT_OPENAI: result("ok", { findings: [F("openai", "gpt-6-nova")] }) }), gh);
    assert.equal(gh.calls.filter((c) => c.args[0] === "issue" && c.args[1] === "create").length, 1);
  });

  it("a canary finding whose listed value MOVED after a not-planned close opens a new issue (4f reversed #1157 decision 13)", () => {
    const retire = (listed) =>
      finding("model-retirement-changed", "anthropic", {
        model: "claude-fable-5-2",
        listed,
        catalogue: "none",
      });
    const closed = [
      {
        provider: "anthropic",
        reason: "not_planned",
        body: openBody("anthropic", [retire("2027-01-01")]),
      },
    ];
    const same = tracker([], closed);
    run(env({ RESULT_ANTHROPIC: result("ok", { findings: [retire("2027-01-01")] }) }), same);
    assert.deepEqual(mutating(same), [], "the acknowledged date stays acknowledged");
    const moved = tracker([], closed);
    run(env({ RESULT_ANTHROPIC: result("ok", { findings: [retire("2027-03-01")] }) }), moved);
    assert.equal(
      moved.calls.filter((c) => c.args[0] === "issue" && c.args[1] === "create").length,
      1,
      "a moved date is news again",
    );
  });

  it("a close as COMPLETED (the reporter's own) is not an acknowledgement: the finding opens again", () => {
    const gh = tracker(
      [],
      [{ provider: "openai", reason: "completed", body: openBody("openai", [F()]) }],
    );
    run(env({ RESULT_OPENAI: result("ok", { findings: [F()] }) }), gh);
    assert.equal(gh.calls.filter((c) => c.args[0] === "issue" && c.args[1] === "create").length, 1);
  });

  it("only the NEWEST closed issue counts: an older not-planned one behind a newer completed one does not", () => {
    const gh = tracker(
      [],
      [
        { provider: "openai", reason: "not_planned", body: openBody("openai", [F()]) },
        { provider: "openai", reason: "completed", body: openBody("openai", [F()]) },
      ],
    );
    run(env({ RESULT_OPENAI: result("ok", { findings: [F()] }) }), gh);
    assert.equal(gh.calls.filter((c) => c.args[0] === "issue" && c.args[1] === "create").length, 1);
  });

  it("the reporter closes its own issues as completed (so its closes never look acknowledged)", () => {
    const gh = tracker([{ provider: "openai", body: openBody("openai", [F()]) }]);
    run(env({}), gh);
    assert.equal(gh.issues[0].state_reason, "completed");
  });

  it("a different set of problems edits the body once and comments once", () => {
    const gh = tracker([{ provider: "openai", body: openBody("openai", [F()]) }]);
    const more = [F(), finding("cache-never-hit", "openai", { step: "cache" })];
    run(env({ RESULT_OPENAI: result("ok", { findings: more }) }), gh);
    assert.deepEqual(
      mutating(gh).map((c) => c.args[1]),
      ["edit", "comment"],
    );
    assert.match(gh.calls.find((c) => c.args[1] === "comment").input, /^Changed as of 27-11-2026/);
    assert.match(gh.issues[0].body, /cache-never-hit/);
  });

  it("a SECOND model gone under the same finding id changes the issue (the fingerprint sees the model)", () => {
    const gh = tracker([{ provider: "openai", body: openBody("openai", [F()]) }]);
    run(env({ RESULT_OPENAI: result("ok", { findings: [F(), F("openai", "gpt-6-nova")] }) }), gh);
    assert.deepEqual(
      mutating(gh).map((c) => c.args[1]),
      ["edit", "comment"],
    );
    assert.match(gh.issues[0].body, /gpt-6-nova/);
  });

  it("clear with the job successful comments 'clear as of DD-MM-YYYY' and closes it as completed", () => {
    const gh = tracker([
      { provider: "gemini", body: openBody("gemini", [F("gemini", "gemini-4-neon")]) },
    ]);
    run(env({}), gh);
    assert.deepEqual(
      mutating(gh).map((c) => c.args.slice(0, 2).join(" ")),
      ["issue comment", "issue close"],
    );
    assert.equal(gh.calls.find((c) => c.args[1] === "comment").input, "Clear as of 27-11-2026.");
    assert.ok(gh.calls.find((c) => c.args[1] === "close").args.includes("completed"));
    assert.equal(gh.issues[0].state, "closed");
  });

  it("only that provider's issue is closed, and a notice never opens one", () => {
    const gh = tracker([
      { provider: "openai", body: openBody("openai", [F()]) },
      { provider: "gemini", body: openBody("gemini", [F("gemini", "g")]) },
    ]);
    run(
      env({
        RESULT_OPENAI: result("ok", { findings: [F()] }),
        RESULT_ANTHROPIC: result("ok", { notices: [notice("fallback-key-used", "anthropic")] }),
      }),
      gh,
    );
    assert.deepEqual(
      gh.issues.map((i) => i.state),
      ["open", "closed"],
    );
    assert.equal(gh.calls.filter((c) => c.args[0] === "issue" && c.args[1] === "create").length, 0);
  });
});

describe("a leg that did not finish is never 'clear', and touches no issue", () => {
  const openAll = () =>
    tracker(
      ["openai", "gemini", "anthropic"].map((p) => ({
        provider: p,
        body: openBody(p, [F(p, "m")]),
      })),
    );
  const cases = {
    skipped: { RESULT_OPENAI: result("skipped", { note: "not configured" }) },
    inconclusive: {
      RESULT_OPENAI: result("inconclusive", { note: "no usable answer after 4 tries (HTTP 503)" }),
    },
    error: { RESULT_OPENAI: result("error", { note: "canary key needs credit" }) },
    "a missing output": { RESULT_OPENAI: undefined },
    "an empty output": { RESULT_OPENAI: "" },
    "an unparsable output": { RESULT_OPENAI: "{not json" },
    "a crashed leg (job failure, no output)": { RESULT_OPENAI: "", JOB_OPENAI: "failure" },
    "a cancelled job that had written ok": { JOB_OPENAI: "cancelled" },
    "a skipped job": { JOB_OPENAI: "skipped" },
    "a failed job that had written ok": { JOB_OPENAI: "failure" },
    "an ok result that lists a finding of another provider": {
      RESULT_OPENAI: JSON.stringify({
        status: "ok",
        findings: [F("gemini", "m")],
        notices: [],
        note: "",
      }),
    },
    "an ok result carrying a key-shaped field": {
      RESULT_OPENAI: JSON.stringify({
        status: "ok",
        findings: [
          { id: "model-missing", provider: "openai", fields: { model: "sk-ant-" + x(24) } },
        ],
        notices: [],
        note: "",
      }),
    },
  };
  for (const [name, patch] of Object.entries(cases)) {
    it(`${name}: no issue is opened, edited, commented on or closed`, () => {
      const gh = openAll();
      const before = JSON.stringify(gh.issues);
      const lines = run(env(patch), gh);
      // OpenAI's issue is number 100: no call may name it, and none may create an OpenAI issue.
      assert.deepEqual(
        mutating(gh).filter(
          (c) => c.args.includes("100") || c.args.includes("provider-check:openai"),
        ),
        [],
      );
      assert.equal(gh.issues[0].state, "open");
      assert.equal(
        JSON.stringify(gh.issues.slice(0, 1)),
        JSON.stringify(JSON.parse(before).slice(0, 1)),
      );
      assert.ok(lines.join("\n").includes("OpenAI"));
    });
  }

  it("no issue is created for a leg that errored even when it had a finding in hand", () => {
    const gh = tracker();
    run(
      env({
        RESULT_OPENAI: JSON.stringify({ status: "error", findings: [F()], notices: [], note: "" }),
      }),
      gh,
    );
    assert.deepEqual(mutating(gh), []);
  });
});

describe("a run that checked nothing", () => {
  const skipped = result("skipped", { note: "not configured" });
  const nothing = { RESULT_OPENAI: skipped, RESULT_GEMINI: skipped, RESULT_ANTHROPIC: skipped };

  it("is flagged (the report step fails) when every leg skipped or gave no result, and still touches no issue", () => {
    const gh = tracker([{ provider: "openai", body: openBody("openai", [F()]) }]);
    const lines = run(env(nothing), gh);
    assert.equal(lines.allSkipped, true);
    assert.ok(lines.some((l) => /NOTHING WAS CHECKED/.test(l)));
    assert.deepEqual(mutating(gh), []);
    const none = run(
      env({ RESULT_OPENAI: "", RESULT_GEMINI: "", RESULT_ANTHROPIC: "" }),
      tracker(),
    );
    assert.equal(none.allSkipped, true);
  });

  it("is quiet when only one leg skips (the Claude leg alone going away stays green)", () => {
    const lines = run(env({ RESULT_ANTHROPIC: skipped }), tracker());
    assert.equal(lines.allSkipped, false);
    assert.ok(!lines.some((l) => /NOTHING WAS CHECKED/.test(l)));
    assert.equal(run(env({}), tracker()).allSkipped, false);
  });
});

describe("the issue text", () => {
  it("passes the public-text whitelist and is built only from templates", () => {
    const body = issueBody({
      provider: "openai",
      kind: "canary",
      findings: [
        F(),
        finding("new-token-type", "openai", {
          step: "chat",
          field: "prompt_tokens_details.video_tokens",
        }),
      ],
      date: "27-11-2026",
    });
    assertPublic(body);
    assert.match(body, /PM may not be pricing a new token type/);
    assert.throws(() => assertPublic("token sk-ant-" + x(24)), /whitelist/);
    assert.throws(() => assertPublic("aé"), /whitelist/);
  });

  it("dates are DD-MM-YYYY in UTC", () => {
    assert.equal(ddmmyyyy(new Date(Date.UTC(2026, 0, 5, 23, 59))), "05-01-2026");
  });

  it("a missing repository is a loud failure, not a silent skip", () => {
    assert.throws(
      () => reportCanary({ env: {}, gh: tracker(), write: () => {} }),
      /GITHUB_REPOSITORY/,
    );
  });
});

describe("self-test", () => {
  it("files a synthetic issue, finds it again by REST, sees the same findings change nothing, and closes it", () => {
    const gh = tracker();
    const lines = reportSelftest({
      env: { GITHUB_REPOSITORY: REPO },
      gh,
      now: NOW,
      write: () => {},
    });
    assert.equal(
      gh.calls.filter((c) => c.args[0] === "issue" && c.args[1] === "create").length,
      1,
      "exactly one issue was filed",
    );
    assert.equal(gh.issues.length, 1);
    assert.equal(gh.issues[0].state, "closed");
    assert.ok(gh.issues[0].labels.includes("provider-check:selftest"));
    assert.match(
      lines[1],
      /Filed: issue opened\. Second run: issue already open, unchanged\. Cleared: clear \(issue closed\)\./,
    );
  });

  it("fails (a red job) when the REST list cannot see the issue it just filed", () => {
    const gh = fakeGh((args) => (args[0] === "api" ? "[]" : ""));
    assert.throws(
      () => reportSelftest({ env: { GITHUB_REPOSITORY: REPO }, gh, now: NOW, write: () => {} }),
      /not found by the REST list/,
    );
  });
});
