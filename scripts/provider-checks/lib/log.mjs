// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The ONLY place the provider checks print, and the only place they write to the job's summary and
// outputs. Public logs are permanent, so a line is built from a short whitelist of fields (never a
// body, a header or a URL), each field's value is checked against its own shape, and the finished
// line is redacted as well. A field that is not on the list is a bug in the caller, not something to
// be passed through: `log` throws. A scan test (log.test.mjs) pins that nothing else in
// scripts/provider-checks calls console.log, console.error, process.stdout or process.stderr.
//
// Pure Node built-ins, ESM, no dependencies (INVARIANTS.md I-18).

import { appendFileSync } from "node:fs";
import { redact, looksSecret } from "./redact.mjs";

const PROVIDERS = new Set(["openai", "gemini", "anthropic", "report"]);
const WORD = /^[a-z0-9_.:-]{1,64}$/;
const SAFE_TEXT = /^[\x20-\x7e]{0,200}$/;
/** A summary line may be a finding's whole sentence, so it gets more room than a log field. */
const SUMMARY_LINE = /^[\x20-\x7e]{0,500}$/;

/** The fields a log line may carry, and what each must look like. */
const FIELDS = {
  provider: (v) => typeof v === "string" && PROVIDERS.has(v),
  leg: (v) => typeof v === "string" && PROVIDERS.has(v),
  status: (v) => ["ok", "skipped", "inconclusive", "error"].includes(v),
  check: (v) => typeof v === "string" && WORD.test(v),
  step: (v) => typeof v === "string" && WORD.test(v),
  http: (v) => Number.isInteger(v) && v >= 0 && v <= 999,
  ok: (v) => typeof v === "boolean",
  count: (v) => Number.isInteger(v) && v >= 0 && v <= 1e9,
  msg: (v) => typeof v === "string" && SAFE_TEXT.test(v),
};

/** Where lines go. Replaced in tests. */
let sink = (line) => process.stdout.write(line + "\n");
/** Secret values this process holds, replaced wherever they might appear. */
const literals = [];

/** Send lines somewhere else (tests). Returns the previous sink. */
export function setSink(fn) {
  const old = sink;
  sink = fn;
  return old;
}

/** Remember a secret's value so redaction removes it even if it has no recognisable shape. */
export function registerSecret(value) {
  if (typeof value === "string" && value.length >= 6 && !literals.includes(value))
    literals.push(value);
}

/** Print one whitelisted line: `log({ provider: "openai", step: "models", http: 200, ok: true })`. */
export function log(fields) {
  const parts = [];
  for (const [key, value] of Object.entries(fields)) {
    const valid = FIELDS[key];
    if (!valid) throw new Error(`log: field not on the whitelist: ${key}`);
    if (!valid(value)) throw new Error(`log: bad value for ${key}`);
    parts.push(`${key}=${value}`);
  }
  sink(redact(parts.join(" "), literals));
}

/**
 * Tell GitHub to mask a value from now on. Its masking covers registered secrets only, so a token the
 * job obtains at run time (the OIDC token, the token it is exchanged for) is masked here the moment
 * it exists, before anything else could print it. The value is also remembered for redaction.
 */
export function mask(value) {
  // One token on one line: a value with whitespace or a control character could add `::` commands.
  if (typeof value !== "string" || value.length < 6 || !/^[\x21-\x7e]+$/.test(value))
    throw new Error("mask: nothing to mask, or not a single printable token");
  registerSecret(value);
  // The command is the one line that must carry the raw value; it is not echoed anywhere else.
  sink("::add-mask::" + value);
}

/** Append whitelisted markdown lines to the job summary (public too). A line with a secret shape is dropped. */
export function summary(lines, env = process.env) {
  const path = env.GITHUB_STEP_SUMMARY;
  const clean = [];
  for (const line of lines) {
    if (typeof line !== "string" || !SUMMARY_LINE.test(line) || looksSecret(line, literals)) {
      clean.push("(a line was withheld: it did not pass the whitelist)");
    } else clean.push(line);
  }
  if (path) appendFileSync(path, clean.join("\n") + "\n");
  else sink(clean.join("\n"));
}

/** Write one job output (a single line, no newline): the leg's sanitised result. */
export function setOutput(name, value, env = process.env) {
  if (!WORD.test(name) || typeof value !== "string" || /[\r\n]/.test(value)) {
    throw new Error("setOutput: bad name or a value with a line break");
  }
  if (looksSecret(value, literals)) throw new Error("setOutput: the value looks like a secret");
  if (env.GITHUB_OUTPUT) appendFileSync(env.GITHUB_OUTPUT, `${name}=${value}\n`);
  else sink(`(output ${name} withheld: no GITHUB_OUTPUT)`);
}
