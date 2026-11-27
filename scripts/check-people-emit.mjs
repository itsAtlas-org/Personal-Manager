// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "The address book changed" is announced from one place.
//
// WHY. The People tab, the pop-out Calendar's meeting card and the pickers re-read when PM emits
// `people://changed`. A person's record changes through commands, an import, a merge, the purge at
// store open and (in the Google PR) a sync pass; each new writer is one more place where "emit it
// after the write" can be written slightly differently, forgotten, or sent while the DB guard is
// still held. So there is one function, `people::announce_changed`, and this gate fails the build
// on any other emit of the constant. It is `check-tasks-emit.mjs` for the people tables, and it
// reads its Rust through that script's comment stripper and file walk so the two cannot drift.
//
// What it checks, over every Rust file under src-tauri/ (comments are ignored, strings are not):
//   1. the identifier `PEOPLE_CHANGED` appears only in src-tauri/src/people/mod.rs;
//   2. the literal "people://changed" appears only where the constant is defined;
//   3. in people/mod.rs, the constant is used exactly once besides its definition, and that use is
//      the emit inside `pub fn announce_changed`;
//   4. and it has something to look at: people/mod.rs defines the constant and the function.
//
// Not checked: that the DB guard is dropped before the emit. That is a review rule here, as it is
// for tasks; `commands/people.rs` has a test pinning the shape of its own commands.
//
// ZERO-DEPENDENCY (INVARIANTS.md I-18): plain text matching, no parser, repo-relative imports only
// — pr.yml's `hygiene` job runs with no `npm ci`.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { stripComments, walk } from "./check-tasks-emit.mjs";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

export const RUST_ROOT = "src-tauri";
export const OWNER = "src-tauri/src/people/mod.rs";
const CONSTANT = "PEOPLE_CHANGED";
const LITERAL = "people://changed";
/** Text as a RegExp source that matches it literally: every metacharacter, backslash included. */
const escapeRegExp = (text) => text.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&");
const LITERAL_RE = escapeRegExp(LITERAL);

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
        `${path}:${lineOf(code, m.index)} names \`${CONSTANT}\`; call \`people::announce_changed(app)\` ` +
          `instead, so "the address book changed" is announced from one place`,
      );
    }
    for (const m of code.matchAll(new RegExp(LITERAL_RE, "g"))) {
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
      new RegExp(`\\bpub\\s+const\\s+${CONSTANT}\\b[^;]*"${LITERAL_RE}"\\s*;`),
    );
    if (!define)
      problems.push(`${OWNER} no longer defines \`pub const ${CONSTANT}\` as "${LITERAL}"`);
    const fn = announceBody(owner);
    if (fn === null) {
      problems.push(`${OWNER} no longer defines \`pub fn announce_changed\``);
    } else if (!new RegExp(`\\.emit\\(\\s*${CONSTANT}\\b`).test(fn.text)) {
      problems.push(`${OWNER}: \`announce_changed\` no longer emits \`${CONSTANT}\``);
    }
    // Every other use of the constant in people/mod.rs is a second emit (or a way to make one).
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
    const literals = [...owner.matchAll(new RegExp(LITERAL_RE, "g"))];
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
    console.error("✗ people-changed emit:\n");
    for (const p of problems) console.error(`  • ${p}`);
    process.exit(1);
  }
  console.log(
    `✓ people-changed emit: ${files} Rust files checked; \`people::announce_changed\` is the only emit of ${CONSTANT}`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
