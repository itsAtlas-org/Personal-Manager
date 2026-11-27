// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { afterEach, beforeEach, describe, it } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, readdirSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { log, mask, registerSecret, setOutput, setSink, summary } from "./lib/log.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const x = (n) => "a1B2c3D4e5".repeat(Math.ceil(n / 10)).slice(0, n);

let lines;
let old;
let dir;
beforeEach(() => {
  lines = [];
  old = setSink((l) => lines.push(l));
  dir = mkdtempSync(join(tmpdir(), "pc-log-"));
});
afterEach(() => {
  setSink(old);
  rmSync(dir, { recursive: true, force: true });
});

describe("log", () => {
  it("prints whitelisted fields as one line", () => {
    log({ leg: "openai", step: "models", http: 200, ok: true });
    assert.deepEqual(lines, ["leg=openai step=models http=200 ok=true"]);
  });

  it("refuses a field that is not on the whitelist, and a value of the wrong shape", () => {
    assert.throws(() => log({ body: "x" }), /whitelist/);
    assert.throws(() => log({ headers: {} }), /whitelist/);
    assert.throws(() => log({ http: "200" }), /bad value/);
    assert.throws(() => log({ step: "has space" }), /bad value/);
    assert.throws(() => log({ msg: "multi\nline" }), /bad value/);
    assert.deepEqual(lines, []);
  });

  it("redacts a secret that slipped into an allowed field, including a registered literal", () => {
    log({ msg: "key sk-ant-" + x(24) });
    assert.match(lines[0], /\[redacted\]/);
    registerSecret("plainsecretvalue");
    log({ msg: "oops plainsecretvalue here" });
    assert.ok(!lines[1].includes("plainsecretvalue"));
  });

  it("mask() issues the add-mask command once and remembers the value for redaction", () => {
    const token = "tok" + x(30);
    mask(token);
    assert.equal(lines[0], "::add-mask::" + token);
    log({ msg: "later " + token });
    assert.ok(!lines[1].includes(token));
    assert.throws(() => mask("x"), /nothing to mask/);
    // a value that could carry a second workflow command is refused outright
    assert.throws(() => mask("abcdef\n::set-output name=x::y"), /single printable token/);
    assert.throws(() => mask("abc def ghi"), /single printable token/);
    assert.throws(() => mask("abcdef\u0000ghi"), /single printable token/);
  });

  it("summary() appends to the summary file and withholds a line with a secret shape", () => {
    const file = join(dir, "summary.md");
    summary(["fine line", "bad sk-ant-" + x(24)], { GITHUB_STEP_SUMMARY: file });
    const text = readFileSync(file, "utf8");
    assert.match(text, /fine line/);
    assert.match(text, /withheld/);
    assert.ok(!text.includes("sk-ant-"));
  });

  it("setOutput() writes one name=value line and refuses a line break or a secret", () => {
    const file = join(dir, "out");
    setOutput("result", '{"status":"ok"}', { GITHUB_OUTPUT: file });
    assert.equal(readFileSync(file, "utf8"), 'result={"status":"ok"}\n');
    assert.throws(() => setOutput("result", "a\nb", { GITHUB_OUTPUT: file }), /line break/);
    assert.throws(() => setOutput("result", "sk-ant-" + x(24), { GITHUB_OUTPUT: file }), /secret/);
    assert.throws(() => setOutput("bad name", "v", { GITHUB_OUTPUT: file }));
  });
});

/** Every .mjs file under scripts/provider-checks, tests and fixtures included. */
function sources(d = here) {
  return readdirSync(d).flatMap((n) => {
    const p = join(d, n);
    if (statSync(p).isDirectory()) return n === "fixtures" ? [] : sources(p);
    return n.endsWith(".mjs") ? [p] : [];
  });
}

describe("nothing prints except lib/log.mjs", () => {
  it("no other file calls console.*, process.stdout or process.stderr", () => {
    const found = [];
    for (const file of sources()) {
      const rel = relative(here, file);
      if (rel === join("lib", "log.mjs") || rel === "log.test.mjs") continue;
      const text = readFileSync(file, "utf8");
      for (const re of [
        /\bconsole\s*\./,
        /\bprocess\s*\.\s*(stdout|stderr)\b/,
        /\bprocess\s*\.\s*binding\b/,
      ]) {
        if (re.test(text)) found.push(`${rel}: ${re}`);
      }
    }
    assert.deepEqual(found, []);
    assert.ok(sources().length >= 12, "the scan saw the files it is meant to guard");
  });
});
