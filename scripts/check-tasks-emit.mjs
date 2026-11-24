// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "The task list changed" is announced from one place.
//
// WHY. Every open task view (the project page, the Pinboard's task lists, the Runway, the
// calendar overlay) re-reads when PM emits `tasks://changed`. Anything a task's links read can
// change it: a task, a stack, a milestone, a project's name, a calendar event. Six writers each
// emitted the constant themselves, and each new writer (a mod's handler, in PR C) is one more
// place where "emit it after the write" can be written slightly differently, or forgotten, or sent
// while the DB guard is still held. So there is one function, `tasks::announce_changed`, and this
// gate fails the build on any other emit of the constant.
//
// What it checks, over every Rust file under src-tauri/ (comments are ignored, strings are not):
//   1. the identifier `TASKS_CHANGED` appears only in src-tauri/src/tasks.rs;
//   2. the literal "tasks://changed" appears only where the constant is defined;
//   3. in tasks.rs, the constant is used exactly once besides its definition, and that use is the
//      emit inside `pub fn announce_changed`;
//   4. and it has something to look at: tasks.rs defines the constant and the function.
//
// ZERO-DEPENDENCY (INVARIANTS.md I-18): plain text matching, no parser — pr.yml's `hygiene` job runs
// with no `npm ci`.

import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

export const RUST_ROOT = "src-tauri";
export const OWNER = "src-tauri/src/tasks.rs";
const CONSTANT = "TASKS_CHANGED";
const LITERAL = "tasks://changed";

/** Every `.rs` file under `dir` (repo-relative, `/`-separated). */
function walk(root, dir) {
  const out = [];
  const visit = (abs) => {
    for (const entry of readdirSync(abs, { withFileTypes: true })) {
      const path = join(abs, entry.name);
      if (entry.isDirectory()) {
        if (entry.name !== "target" && entry.name !== "node_modules") visit(path);
      } else if (entry.name.endsWith(".rs")) {
        out.push(relative(root, path).split(sep).join("/"));
      }
    }
  };
  visit(join(root, dir));
  return out.sort();
}

/**
 * The source with its comments blanked (newlines kept, so line numbers hold) and its string
 * literals left alone: "tasks://changed" holds a `//` that must not start a comment. Handles line
 * and (nested) block comments, ordinary and raw strings, and char literals well enough for PM's
 * code; a lifetime (`'a`) is told from a char (`'x'`) by its closing quote.
 */
export function stripComments(src) {
  let out = "";
  let i = 0;
  const n = src.length;
  while (i < n) {
    const c = src[i];
    const next = src[i + 1];
    if (c === "/" && next === "/") {
      while (i < n && src[i] !== "\n") i++;
    } else if (c === "/" && next === "*") {
      let depth = 1;
      i += 2;
      while (i < n && depth > 0) {
        if (src[i] === "/" && src[i + 1] === "*") {
          depth++;
          i += 2;
        } else if (src[i] === "*" && src[i + 1] === "/") {
          depth--;
          i += 2;
        } else {
          if (src[i] === "\n") out += "\n";
          i++;
        }
      }
    } else if (c === "r" && (next === '"' || next === "#") && /^r#*"/.test(src.slice(i, i + 40))) {
      const hashes = /^r(#*)"/.exec(src.slice(i, i + 40))[1];
      const close = `"${hashes}`;
      const end = src.indexOf(close, i + 2 + hashes.length);
      const stop = end < 0 ? n : end + close.length;
      out += src.slice(i, stop);
      i = stop;
    } else if (c === '"') {
      let j = i + 1;
      while (j < n && src[j] !== '"') j += src[j] === "\\" ? 2 : 1;
      out += src.slice(i, j + 1);
      i = j + 1;
    } else if (c === "'" && /^'(\\.[^']*|[^\\'])'/.test(src.slice(i, i + 12))) {
      const m = /^'(\\.[^']*|[^\\'])'/.exec(src.slice(i, i + 12));
      out += m[0];
      i += m[0].length;
    } else {
      out += c;
      i++;
    }
  }
  return out;
}

const lineOf = (text, index) => text.slice(0, index).split("\n").length;

/** The body of `pub fn announce_changed`, or null. */
function announceBody(code) {
  const at = code.search(/\bpub\s+fn\s+announce_changed\b/);
  if (at < 0) return null;
  const open = code.indexOf("{", at);
  if (open < 0) return null;
  let depth = 0;
  for (let i = open; i < code.length; i++) {
    if (code[i] === "{") depth++;
    else if (code[i] === "}" && --depth === 0)
      return { text: code.slice(open, i + 1), start: open };
  }
  return null;
}

export function scan(root) {
  const files = walk(root, RUST_ROOT);
  const problems = [];
  let owner = null;
  for (const path of files) {
    const code = stripComments(readFileSync(join(root, path), "utf8"));
    if (path === OWNER) {
      owner = code;
      continue;
    }
    for (const m of code.matchAll(new RegExp(`\\b${CONSTANT}\\b`, "g"))) {
      problems.push(
        `${path}:${lineOf(code, m.index)} names \`${CONSTANT}\`; call \`tasks::announce_changed(app)\` ` +
          `instead, so "the task list changed" is announced from one place`,
      );
    }
    for (const m of code.matchAll(new RegExp(LITERAL.replace(/[/]/g, "\\/"), "g"))) {
      problems.push(
        `${path}:${lineOf(code, m.index)} holds the event name "${LITERAL}"; only ${OWNER} may, ` +
          `where \`${CONSTANT}\` is defined`,
      );
    }
  }

  if (owner === null) {
    problems.push(`${OWNER} is missing`);
  } else {
    const define = owner.match(
      new RegExp(`\\bpub\\s+const\\s+${CONSTANT}\\b[^;]*"${LITERAL}"\\s*;`),
    );
    if (!define)
      problems.push(`${OWNER} no longer defines \`pub const ${CONSTANT}\` as "${LITERAL}"`);
    const fn = announceBody(owner);
    if (fn === null) {
      problems.push(`${OWNER} no longer defines \`pub fn announce_changed\``);
    } else if (!new RegExp(`\\.emit\\(\\s*${CONSTANT}\\b`).test(fn.text)) {
      problems.push(`${OWNER}: \`announce_changed\` no longer emits \`${CONSTANT}\``);
    }
    // Every other use of the constant in tasks.rs is a second emit (or a way to make one).
    for (const m of owner.matchAll(new RegExp(`\\b${CONSTANT}\\b`, "g"))) {
      const inFn = fn !== null && m.index >= fn.start && m.index < fn.start + fn.text.length;
      const inDefinition =
        define !== null && m.index === define.index + define[0].indexOf(CONSTANT);
      if (!inFn && !inDefinition) {
        problems.push(
          `${OWNER}:${lineOf(owner, m.index)} uses \`${CONSTANT}\` outside \`announce_changed\`; ` +
            `that is a second emit`,
        );
      }
    }
    const literals = [...owner.matchAll(new RegExp(LITERAL.replace(/[/]/g, "\\/"), "g"))];
    if (literals.length > 1) {
      problems.push(
        `${OWNER} holds the event name "${LITERAL}" ${literals.length} times; once, in the constant`,
      );
    }
  }
  return { problems, files: files.length };
}

function main() {
  const { problems, files } = scan(repoRoot);
  if (problems.length > 0) {
    console.error("✗ tasks-changed emit:\n");
    for (const p of problems) console.error(`  • ${p}`);
    process.exit(1);
  }
  console.log(
    `✓ tasks-changed emit: ${files} Rust files checked; \`tasks::announce_changed\` is the only emit of ${CONSTANT}`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
