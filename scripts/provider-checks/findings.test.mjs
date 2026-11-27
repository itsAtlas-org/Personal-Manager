// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  FINDINGS,
  NOTICES,
  describe as describeFinding,
  finding,
  fingerprint,
  makeResult,
  notice,
  parseResult,
} from "./lib/findings.mjs";

const x = (n) => "a1B2c3D4e5".repeat(Math.ceil(n / 10)).slice(0, n);
const ok = (extra = {}) =>
  JSON.stringify({ status: "ok", findings: [], notices: [], note: "", ...extra });

describe("finding", () => {
  it("builds a finding from an id and exactly the fields the id allows", () => {
    const f = finding("model-missing", "openai", { model: "gpt-6-luna" });
    assert.equal(
      describeFinding(f).detail,
      "A model in direct_models.json is no longer listed by the provider: gpt-6-luna.",
    );
    assert.throws(() => finding("nope", "openai", {}), /unknown finding id/);
    assert.throws(() => finding("model-missing", "mistral", { model: "m" }), /unknown provider/);
    assert.throws(() => finding("model-missing", "openai", {}), /missing field/);
    assert.throws(
      () => finding("model-missing", "openai", { model: "m", extra: "x" }),
      /not allowed/,
    );
  });

  it("refuses a field that is long, unprintable or shaped like a key", () => {
    assert.throws(
      () => finding("model-missing", "openai", { model: "m".repeat(121) }),
      /plain word/,
    );
    assert.throws(() => finding("model-missing", "openai", { model: "a\nb" }), /plain word/);
    assert.throws(() => finding("model-missing", "openai", { model: "sk-ant-" + x(24) }), /secret/);
    assert.throws(() => finding("model-missing", "openai", { model: { a: 1 } }), /neither/);
  });

  it("refuses markup, a mention or a link in any vendor-derived field", () => {
    for (const bad of [
      "@BBYU26",
      "[click](http://evil.example)",
      "</details>![x](http://e.x/a.png)",
      "a b",
      "`code`",
      "<b>",
    ]) {
      assert.throws(
        () => finding("new-token-type", "openai", { step: "chat", field: bad }),
        /plain word/,
        bad,
      );
      assert.throws(
        () => finding("stop-reason-unknown", "openai", { step: "chat", value: bad }),
        /plain word/,
        bad,
      );
    }
    assert.throws(
      () => finding("stream-shape-changed", "openai", { step: "chat", what: "see @BBYU26" }),
      /plain word/,
    );
    assert.throws(
      () => finding("stream-shape-changed", "openai", { step: "chat", what: "a [x](http://e.x)" }),
      /plain word/,
    );
    // the fixed sentences the legs use still pass
    finding("stream-shape-changed", "openai", { step: "chat", what: "no [DONE] at the end" });
  });

  it("every template renders within a summary line and carries no secret shape", () => {
    for (const [id, spec] of Object.entries(FINDINGS)) {
      const fields = Object.fromEntries(spec.keys.map((k) => [k, "x".repeat(60)]));
      const f = finding(id, "gemini", fields);
      const d = describeFinding(f);
      assert.ok(d.detail.length <= 500 && d.action.length <= 500, id);
      assert.ok(["high", "medium"].includes(d.severity), id);
    }
    for (const id of Object.keys(NOTICES))
      assert.deepEqual(notice(id, "openai"), { id, provider: "openai" });
  });
});

describe("parseResult", () => {
  it("accepts a conforming result and keeps only what it validated", () => {
    const f = finding("model-missing", "openai", { model: "gpt-6-luna" });
    const r = parseResult(
      JSON.stringify(
        makeResult("ok", { findings: [f], notices: [notice("fallback-key-used", "openai")] }),
      ),
      "openai",
    );
    assert.equal(r.ok, true);
    assert.equal(r.value.findings.length, 1);
  });

  it("rejects a missing, oversized, non-JSON, extra-key or wrong-provider result", () => {
    assert.equal(parseResult(undefined, "openai").ok, false);
    assert.equal(parseResult("", "openai").ok, false);
    assert.equal(parseResult("{not json", "openai").ok, false);
    assert.equal(parseResult("[]", "openai").ok, false);
    assert.equal(parseResult(ok({ raw: "a vendor body" }), "openai").ok, false);
    assert.equal(parseResult(ok({ status: "great" }), "openai").ok, false);
    assert.equal(parseResult(ok({ note: "x".repeat(70000) }), "openai").ok, false);
    const other = finding("model-missing", "gemini", { model: "m" });
    assert.equal(parseResult(ok({ findings: [other] }), "openai").ok, false);
  });

  it("rejects findings on a leg that did not finish ok, a made-up id, or a key-shaped note", () => {
    const f = finding("model-missing", "openai", { model: "m" });
    assert.equal(parseResult(ok({ status: "error", findings: [f] }), "openai").ok, false);
    assert.equal(
      parseResult(ok({ findings: [{ id: "free-text", provider: "openai", fields: {} }] }), "openai")
        .ok,
      false,
    );
    assert.equal(
      parseResult(
        ok({
          findings: [
            { id: "model-missing", provider: "openai", fields: { model: "m" }, detail: "ignored" },
          ],
        }),
        "openai",
      ).ok,
      true,
      "an extra property is dropped, never trusted",
    );
    assert.equal(parseResult(ok({ note: "token sk-ant-" + x(24) }), "openai").ok, false);
    assert.equal(parseResult(ok({ findings: Array(51).fill(f) }), "openai").ok, false);
  });
});

describe("fingerprint", () => {
  it("is eight hex over the sorted findings: order and counts do not matter, a different problem does", () => {
    const a = finding("model-missing", "openai", { model: "a" });
    const b = finding("cache-never-hit", "openai", { step: "cache" });
    const c = finding("model-missing", "openai", { model: "other" });
    assert.match(fingerprint([a, b]), /^[0-9a-f]{8}$/);
    assert.equal(fingerprint([a, b]), fingerprint([b, a]));
    assert.notEqual(fingerprint([a]), fingerprint([c]), "a different model is a different problem");
    assert.notEqual(fingerprint([a]), fingerprint([a, b]));
    assert.notEqual(fingerprint([a]), fingerprint([a, c]), "a second model missing changes it");
    const n1 = finding("new-token-type", "openai", { step: "chat", field: "x_tokens" });
    const n2 = finding("new-token-type", "openai", { step: "chat", field: "y_tokens" });
    assert.notEqual(fingerprint([n1]), fingerprint([n1, n2]));
    const u1 = finding("unpriced-token-seen", "openai", { step: "chat", field: "k", value: 3 });
    const u2 = finding("unpriced-token-seen", "openai", { step: "chat", field: "k", value: 9 });
    assert.equal(fingerprint([u1]), fingerprint([u2]), "a changing count never flaps the issue");
    const r1 = finding("request-refused", "openai", { step: "chat", http: 400 });
    const r2 = finding("request-refused", "openai", { step: "chat", http: 500 });
    assert.equal(fingerprint([r1]), fingerprint([r2]));
  });
});
