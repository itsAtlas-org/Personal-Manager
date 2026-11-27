// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The shared reporter on the drift kind, against the fake `gh` issue tracker. The same rule as the
// canary's: only an `ok` run whose job succeeded may open, change or close an issue, and only a FINDING
// does; a notice, an unfinished run, a missing output and a provider the run could not fully check each
// leave every issue exactly as it was.

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { finding, makeResult, notice } from "./lib/findings.mjs";
import { loadSources } from "./lib/data.mjs";
import { issueBody, reportDrift } from "./report.mjs";
import { tracker } from "./testkit.mjs";

const REPO = "itsAtlas-org/Personal-Manager";
const NOW = new Date(Date.UTC(2026, 10, 30, 6, 17));
const URLS = Object.fromEntries(
  Object.entries(loadSources().drift.pages).map(([k, p]) => [k, p.url]),
);

const mutating = (gh) =>
  gh.calls.filter(
    (c) =>
      (c.args[0] === "issue" && ["create", "edit", "comment", "close"].includes(c.args[1])) ||
      c.args[0] === "label",
  );
const drift = (provider = "openai", model = "gpt-6-luna", field = "price_out") =>
  finding("price-drift", provider, {
    model,
    field,
    listed: 0.6,
    catalogue: 0.5,
    source: provider === "openai" ? "openai-pricing" : "anthropic-pricing",
  });
const env = (result, job = "success") => ({
  GITHUB_REPOSITORY: REPO,
  RESULT_DRIFT:
    typeof result === "string" || result === undefined ? result : JSON.stringify(result),
  JOB_DRIFT: job,
});
const run = (e, gh) => reportDrift({ env: e, gh, urls: URLS, now: NOW, write: () => {} });
const openBody = (provider, findings) =>
  issueBody({ provider, kind: "drift", findings, date: "23-11-2026", urls: URLS });

describe("a finding opens ONE issue per provider and kind", () => {
  it("creates it with the drift labels, the marker, the fingerprint, the page address and the next step", () => {
    const gh = tracker();
    run(env(makeResult("ok", { findings: [drift()] })), gh);
    const creates = gh.calls.filter((c) => c.args[0] === "issue" && c.args[1] === "create");
    assert.equal(creates.length, 1);
    const a = creates[0].args;
    assert.equal(a[a.indexOf("--title") + 1], "Provider check (drift): OpenAI");
    assert.equal(a[a.indexOf("--assignee") + 1], "BBYU26");
    const labels = a.flatMap((x, i) => (a[i - 1] === "--label" ? [x] : []));
    assert.deepEqual(labels, ["provider-check", "provider-check:openai", "provider-check:drift"]);
    const body = creates[0].input;
    assert.match(body, /^<!-- pm-provider-check:openai:drift -->\n<!-- fp:[0-9a-f]{8} -->\n/);
    assert.match(
      body,
      /gpt-6-luna: the vendor page lists price_out as 0\.6; the catalogue says 0\.5/,
    );
    assert.match(body, / {2}- Page: https:\/\/developers\.openai\.com\/api\/docs\/pricing\.md\n/);
    assert.match(body, /Next step: Check the page, then update the row/);
    assert.match(body, /provider-drift\.yml/);
    assert.doesNotMatch(body, /provider-canary\.yml/);
  });

  it("looks for an open issue by the strongly consistent REST list (drift labels), never the search index", () => {
    const gh = tracker();
    run(env(makeResult("ok")), gh);
    assert.equal(gh.calls.length, 3, "one lookup per provider");
    for (const c of gh.calls) {
      assert.equal(c.args[0], "api");
      assert.match(
        c.args[1],
        /^repos\/itsAtlas-org\/Personal-Manager\/issues\?labels=provider-check:(openai|gemini|anthropic),provider-check:drift&state=open&per_page=5$/,
      );
    }
  });

  it("files each provider separately from the one result: two providers, two issues, one for the third only if it has findings", () => {
    const gh = tracker();
    run(
      env(
        makeResult("ok", { findings: [drift("openai"), drift("anthropic", "claude-haiku-5-5")] }),
      ),
      gh,
    );
    const titles = gh.calls
      .filter((c) => c.args[0] === "issue" && c.args[1] === "create")
      .map((c) => c.args[c.args.indexOf("--title") + 1])
      .sort();
    assert.deepEqual(titles, [
      "Provider check (drift): Anthropic",
      "Provider check (drift): OpenAI",
    ]);
  });

  it("the same findings again change nothing; a different set edits the body and comments once", () => {
    const f = drift();
    const gh = tracker([{ provider: "openai", kind: "drift", body: openBody("openai", [f]) }]);
    run(env(makeResult("ok", { findings: [f] })), gh);
    assert.deepEqual(mutating(gh), []);
    const g = tracker([{ provider: "openai", kind: "drift", body: openBody("openai", [f]) }]);
    run(env(makeResult("ok", { findings: [f, drift("openai", "gpt-6.2-sol", "price_in")] })), g);
    assert.deepEqual(
      mutating(g).map((c) => c.args.slice(0, 2).join(" ")),
      ["issue edit", "issue comment"],
    );
  });

  it("clears an open issue only for an ok run with zero findings from a successful job", () => {
    const body = openBody("openai", [drift()]);
    const gh = tracker([{ provider: "openai", kind: "drift", body }]);
    run(env(makeResult("ok")), gh);
    assert.deepEqual(
      mutating(gh).map((c) => c.args.slice(0, 2).join(" ")),
      ["issue comment", "issue close"],
    );
    assert.ok(gh.calls.find((c) => c.args[1] === "close").args.includes("completed"));
  });
});

describe("what never touches an issue", () => {
  const body = openBody("openai", [drift()]);
  const open = () => tracker([{ provider: "openai", kind: "drift", body }]);

  for (const [name, e] of [
    ["status skipped", env(makeResult("skipped"))],
    ["status inconclusive", env(makeResult("inconclusive", { note: "busy" }))],
    ["status error", env(makeResult("error", { note: "crashed" }))],
    ["a missing output", env(undefined)],
    ["an empty output", env("")],
    ["an output that is not JSON", env("not json")],
    ["a job that failed, with an ok-looking result", env(makeResult("ok"), "failure")],
    ["a job that was cancelled", env(makeResult("ok"), "cancelled")],
    ["a job that was skipped", env(makeResult("ok"), "skipped")],
    [
      "findings on a run that did not finish ok",
      env('{"status":"error","findings":[],"notices":[]}', "failure"),
    ],
  ]) {
    it(`${name}: no issue is opened, edited, commented on or closed, and the open one stays open`, () => {
      const gh = open();
      run(e, gh);
      assert.deepEqual(mutating(gh), []);
      assert.equal(gh.issues[0].state, "open");
    });
  }

  it("a NOTICE never creates an issue, or edits one, or closes one", () => {
    const gh = tracker();
    const notices = [
      notice("cross-check-only", "gemini", { model: "gemini-3.8-flash" }),
      notice("price-step-due", "gemini", { model: "gemini-4-carbon", date: "2026-12-01" }),
      notice("source-url-changed", "openai"),
      notice("terms-stamp-changed", "gemini"),
      notice("cache-storage-changed", "gemini", { model: "gemini-3.8-flash" }),
    ];
    const lines = run(env(makeResult("ok", { notices })), gh);
    assert.deepEqual(mutating(gh), []);
    assert.ok(lines.some((l) => l.includes("notice price-step-due (gemini-4-carbon 2026-12-01)")));
    // ...and with an issue already open, a notice-only week still CLOSES nothing it should not: the
    // run is clear of findings, so the open one closes, but only because the findings are gone.
    const open2 = tracker([
      {
        provider: "gemini",
        kind: "drift",
        body: openBody("gemini", [drift("gemini", "gemini-4-neon")]),
      },
    ]);
    run(env(makeResult("ok", { notices })), open2);
    assert.deepEqual(
      mutating(open2).map((c) => c.args.slice(0, 2).join(" ")),
      ["issue comment", "issue close"],
    );
  });

  it("a provider the run could not fully check is left alone, even with no findings for it", () => {
    const gh = open();
    const lines = run(env(makeResult("ok", { unchecked: ["openai"] })), gh);
    assert.deepEqual(mutating(gh), []);
    assert.ok(lines.some((l) => l.includes("Not fully checked this week")));
    assert.ok(
      !gh.calls.some((c) => c.args[0] === "api" && c.args[1].includes("provider-check:openai")),
    );
  });

  it("an unchecked provider with a REAL finding still files it (a blocked terms page must not hide a price change), but never clears", () => {
    const gh = tracker();
    run(env(makeResult("ok", { findings: [drift("openai")], unchecked: ["openai"] })), gh);
    const creates = gh.calls.filter((c) => c.args[0] === "issue" && c.args[1] === "create");
    assert.equal(creates.length, 1);
    assert.match(creates[0].input, /gpt-6-luna: the vendor page lists price_out/);
    // The same unchecked provider, zero findings, an open issue: untouched (not knowing is not clear).
    const open = tracker([
      { provider: "openai", kind: "drift", body: openBody("openai", [drift()]) },
    ]);
    run(env(makeResult("ok", { unchecked: ["openai"] })), open);
    assert.deepEqual(mutating(open), []);
    // And with the same finding it only stays as it is.
    const same = tracker([
      { provider: "openai", kind: "drift", body: openBody("openai", [drift()]) },
    ]);
    run(env(makeResult("ok", { findings: [drift()], unchecked: ["openai"] })), same);
    assert.deepEqual(mutating(same), []);
  });

  it("an unchecked provider does not stop the others being filed", () => {
    const gh = tracker();
    run(
      env(
        makeResult("ok", {
          findings: [drift("anthropic", "claude-haiku-5-5")],
          unchecked: ["openai"],
        }),
      ),
      gh,
    );
    assert.equal(gh.calls.filter((c) => c.args[0] === "issue" && c.args[1] === "create").length, 1);
  });

  it("a result that tries to smuggle text in is refused whole: a key-shaped field, a long string, a link", () => {
    for (const fields of [
      {
        model: "sk-ant-abcdefgh12345678",
        field: "price_in",
        listed: 1,
        catalogue: 2,
        source: "openai-pricing",
      },
      {
        model: "x".repeat(300),
        field: "price_in",
        listed: 1,
        catalogue: 2,
        source: "openai-pricing",
      },
      {
        model: "[a](http://x.example)",
        field: "price_in",
        listed: 1,
        catalogue: 2,
        source: "openai-pricing",
      },
    ]) {
      const gh = open();
      run(
        env({
          status: "ok",
          findings: [{ id: "price-drift", provider: "openai", fields }],
          notices: [],
        }),
        gh,
      );
      assert.deepEqual(mutating(gh), [], JSON.stringify(fields).slice(0, 40));
    }
    const gh = open();
    run(
      env({
        status: "ok",
        findings: [{ id: "not-a-finding", provider: "openai", fields: {} }],
        notices: [],
      }),
      gh,
    );
    assert.deepEqual(mutating(gh), []);
  });

  it("a missing repository is a loud failure, not a silent skip", () => {
    assert.throws(
      () => reportDrift({ env: {}, gh: tracker(), write: () => {} }),
      /GITHUB_REPOSITORY/,
    );
  });
});

describe("a run that cannot say what it found must not look green", () => {
  it("a result the whitelist refuses (more than 50 findings, say) touches nothing and FAILS the report step", () => {
    const sixty = Array.from({ length: 60 }, (_, i) => drift("openai", `gpt-6-m${i}`, "price_in"));
    const gh = tracker();
    const lines = run(env({ status: "ok", findings: sixty, notices: [] }), gh);
    assert.deepEqual(mutating(gh), []);
    assert.equal(lines.failed, true);
    assert.ok(lines.some((l) => l.includes("NOTHING WAS REPORTED")));
    for (const bad of [undefined, "", "not json"])
      assert.equal(run(env(bad), tracker()).failed, true);
  });

  it("every provider unchecked (inconclusive): no issue is touched, the line says nothing was checked, and the step FAILS", () => {
    const open = tracker([
      { provider: "openai", kind: "drift", body: openBody("openai", [drift()]) },
    ]);
    const lines = run(
      env(makeResult("inconclusive", { unchecked: ["openai", "gemini", "anthropic"] })),
      open,
    );
    assert.deepEqual(mutating(open), []);
    assert.equal(lines.failed, true);
    assert.ok(lines.some((l) => l.includes("NOTHING WAS CHECKED")));
  });

  it("an ordinary week, and a week with one provider unchecked, pass", () => {
    assert.equal(run(env(makeResult("ok")), tracker()).failed, false);
    assert.equal(run(env(makeResult("ok", { unchecked: ["gemini"] })), tracker()).failed, false);
  });
});

describe("a moving price is news; an acknowledged issue is per kind", () => {
  const priced = (listed) =>
    finding("price-drift", "openai", {
      model: "gpt-6-luna",
      field: "price_out",
      listed,
      catalogue: 0.5,
      source: "openai-pricing",
    });

  it("closed as 'not planned' for 0.6: the same finding stays acknowledged on the drift kind, a LATER move to 0.9 opens a fresh issue", () => {
    const closed = [
      {
        provider: "openai",
        kind: "drift",
        reason: "not_planned",
        body: openBody("openai", [priced(0.6)]),
      },
    ];
    const same = tracker([], closed);
    const lines = run(env(makeResult("ok", { findings: [priced(0.6)] })), same);
    assert.deepEqual(mutating(same), []);
    assert.ok(lines.some((l) => l.includes("acknowledged")));
    const moved = tracker([], closed);
    run(env(makeResult("ok", { findings: [priced(0.9)] })), moved);
    assert.equal(
      moved.calls.filter((c) => c.args[0] === "issue" && c.args[1] === "create").length,
      1,
      "0.9 is not what was acknowledged",
    );
  });

  it("an open issue for 0.6 is edited when the vendor moves it to 0.9 (the body must not keep the old number)", () => {
    const gh = tracker([
      { provider: "openai", kind: "drift", body: openBody("openai", [priced(0.6)]) },
    ]);
    run(env(makeResult("ok", { findings: [priced(0.9)] })), gh);
    assert.deepEqual(
      mutating(gh).map((c) => c.args.slice(0, 2).join(" ")),
      ["issue edit", "issue comment"],
    );
    assert.match(gh.issues[0].body, /lists price_out as 0\.9/);
  });
});

describe("the issue body", () => {
  it("the self-test's footer does not point at a workflow file that does not exist", () => {
    const body = issueBody({
      provider: "openai",
      kind: "selftest",
      findings: [drift()],
      date: "27-11-2026",
    });
    assert.doesNotMatch(body, /provider-selftest\.yml/);
    assert.match(body, /self-test/);
  });

  it("a source name that is a prototype key (constructor) never becomes a Page line", () => {
    const f = finding("price-drift", "openai", {
      model: "gpt-6-luna",
      field: "price_in",
      listed: 1,
      catalogue: 2,
      source: "constructor",
    });
    const body = issueBody({
      provider: "openai",
      kind: "drift",
      findings: [f],
      date: "27-11-2026",
      urls: {},
    });
    assert.doesNotMatch(body, /Page:/);
  });
});
