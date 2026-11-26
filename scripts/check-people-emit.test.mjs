// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The people-changed emit gate's own rules. Each case builds the smallest tree that breaks one rule,
// on top of a baseline that passes, so the failure a test sees is its own. The comment stripper
// and the file walk are the tasks gate's and are tested there; here they are only used.
//
// Importing the module does not run the gate: entry-point guard at the bottom of it.

import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { scan } from "./check-people-emit.mjs";

const roots = [];
afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

const PEOPLE_RS = `//! People.
/// Emitted after every people write.
pub const PEOPLE_CHANGED: &str = "people://changed";

/// The one emit.
pub fn announce_changed(app: &tauri::AppHandle) {
    use tauri::Emitter;
    let _ = app.emit(PEOPLE_CHANGED, ());
}
`;

/** A tree that passes; \`extra\` adds or replaces files (null removes one). */
function fixture(extra = {}) {
  const files = {
    "src-tauri/src/people/mod.rs": PEOPLE_RS,
    "src-tauri/src/commands/people.rs": `fn f(app: &AppHandle) {\n    crate::people::announce_changed(app);\n}\n`,
    "src-tauri/src/commands/canon.rs": `// Emits people://changed through people::announce_changed (PEOPLE_CHANGED).\nfn f() {}\n`,
    ...extra,
  };
  const root = mkdtempSync(join(tmpdir(), "pm-people-emit-"));
  roots.push(root);
  for (const [path, text] of Object.entries(files)) {
    if (text === null) continue;
    mkdirSync(join(root, dirname(path)), { recursive: true });
    writeFileSync(join(root, path), text);
  }
  return root;
}

const problemsOf = (extra) => scan(fixture(extra)).problems;

describe("the people-changed emit gate", () => {
  it("passes a tree with one emit, in announce_changed, and comments that name it", () => {
    const { problems, files } = scan(fixture());
    expect(problems).toEqual([]);
    expect(files).toBe(3);
  });

  it("fails on a second emit of the constant anywhere else (an import's or a sync's own)", () => {
    const problems = problemsOf({
      "src-tauri/src/commands/seventh.rs": `fn f(app: &AppHandle) {\n    let _ = app.emit(crate::people::PEOPLE_CHANGED, ());\n}\n`,
    });
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("src-tauri/src/commands/seventh.rs:2");
    expect(problems[0]).toContain("announce_changed");
  });

  it("fails on the people tests naming it too: only people/mod.rs may", () => {
    expect(
      problemsOf({ "src-tauri/src/people/tests.rs": `fn f() { emit(PEOPLE_CHANGED); }\n` }),
    ).toHaveLength(1);
  });

  it("fails on an alias, a use of the constant, or the event name as a literal", () => {
    expect(
      problemsOf({ "src-tauri/src/a.rs": `use crate::people::PEOPLE_CHANGED as CHANGED;\n` }),
    ).toHaveLength(1);
    expect(
      problemsOf({
        "src-tauri/src/a.rs": `fn f(app: &AppHandle) { let _ = app.emit("people://changed", ()); }\n`,
      }),
    ).toHaveLength(1);
    expect(
      problemsOf({ "src-tauri/src/a.rs": `const N: &str = r#"people://changed"#;\n` }),
    ).toHaveLength(1);
  });

  it("does not mistake the tasks event for the people one", () => {
    expect(
      problemsOf({
        "src-tauri/src/a.rs": `const N: &str = "tasks://changed";\nfn f() { emit(TASKS_CHANGED); }\n`,
      }),
    ).toEqual([]);
  });

  it("looks in every Rust file under src-tauri, tests and build scripts included", () => {
    for (const path of [
      "src-tauri/tests/x.rs",
      "src-tauri/build.rs",
      "src-tauri/src/deep/a/b/c.rs",
    ]) {
      expect(problemsOf({ [path]: `fn f() { emit(PEOPLE_CHANGED); }\n` })).toHaveLength(1);
    }
  });

  it("ignores a Rust file's comments, doc comments and a target directory", () => {
    expect(
      problemsOf({
        "src-tauri/src/a.rs": `/// emits PEOPLE_CHANGED, "people://changed"\n//! PEOPLE_CHANGED\nfn f() {} /* PEOPLE_CHANGED */\n`,
        "src-tauri/target/debug/gen.rs": `fn f() { emit(PEOPLE_CHANGED); }\n`,
      }),
    ).toEqual([]);
  });

  it("fails on a second use of the constant inside people/mod.rs", () => {
    const problems = problemsOf({
      "src-tauri/src/people/mod.rs":
        PEOPLE_RS +
        `\npub fn also(app: &tauri::AppHandle) {\n    let _ = app.emit(PEOPLE_CHANGED, ());\n}\n`,
    });
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("outside `announce_changed`");
  });

  it("fails on the event name written a second time inside people/mod.rs", () => {
    const problems = problemsOf({
      "src-tauri/src/people/mod.rs": PEOPLE_RS + `\nconst OTHER: &str = "people://changed";\n`,
    });
    expect(problems.join("\n")).toContain("2 times");
  });

  it("fails when people/mod.rs loses its constant, its function or the emit in it", () => {
    expect(
      problemsOf({
        "src-tauri/src/people/mod.rs": PEOPLE_RS.replace(
          "pub fn announce_changed",
          "pub fn announce",
        ),
      }).join("\n"),
    ).toContain("no longer defines `pub fn announce_changed`");
    expect(
      problemsOf({
        "src-tauri/src/people/mod.rs": PEOPLE_RS.replace('"people://changed"', '"people://other"'),
      }).join("\n"),
    ).toContain("no longer defines `pub const PEOPLE_CHANGED`");
    expect(
      problemsOf({
        "src-tauri/src/people/mod.rs": PEOPLE_RS.replace("app.emit(PEOPLE_CHANGED, ())", "()"),
      }).join("\n"),
    ).toContain("no longer emits");
    expect(problemsOf({ "src-tauri/src/people/mod.rs": null }).join("\n")).toContain("is missing");
  });
});
