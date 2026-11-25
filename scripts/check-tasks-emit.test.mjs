// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The tasks-changed emit gate's own rules. Each case builds the smallest tree that breaks one rule,
// on top of a baseline that passes, so the failure a test sees is its own.
//
// Importing the module does not run the gate: entry-point guard at the bottom of it.

import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { scan, stripComments } from "./check-tasks-emit.mjs";

const roots = [];
afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

const TASKS_RS = `//! Tasks.
/// Emitted after every write.
pub const TASKS_CHANGED: &str = "tasks://changed";

/// The one emit.
pub fn announce_changed(app: &tauri::AppHandle) {
    use tauri::Emitter;
    let _ = app.emit(TASKS_CHANGED, ());
}
`;

/** A tree that passes; \`extra\` adds or replaces files (null removes one). */
function fixture(extra = {}) {
  const files = {
    "src-tauri/src/tasks.rs": TASKS_RS,
    "src-tauri/src/commands/tasks.rs": `fn changed(app: &AppHandle) {\n    crate::tasks::announce_changed(app);\n}\n`,
    "src-tauri/src/commands/canon.rs": `// Emits tasks://changed through tasks::announce_changed (TASKS_CHANGED).\nfn f() {}\n`,
    ...extra,
  };
  const root = mkdtempSync(join(tmpdir(), "pm-tasks-emit-"));
  roots.push(root);
  for (const [path, text] of Object.entries(files)) {
    if (text === null) continue;
    mkdirSync(join(root, dirname(path)), { recursive: true });
    writeFileSync(join(root, path), text);
  }
  return root;
}

const problemsOf = (extra) => scan(fixture(extra)).problems;

describe("stripComments", () => {
  it("drops comments but keeps strings, even one holding two slashes", () => {
    const code = stripComments(
      `let a = "tasks://changed"; // TASKS_CHANGED\n/* TASKS_CHANGED /* nested */ TASKS_CHANGED */ let b = 1;\n/// TASKS_CHANGED\nlet c = 'x'; let d = r#"x // y"#; fn f<'a>(x: &'a str) {}`,
    );
    expect(code).toContain(`"tasks://changed"`);
    expect(code).toContain(`r#"x // y"#`);
    expect(code).toContain("let b = 1;");
    expect(code).toContain("fn f<'a>(x: &'a str)");
    expect(code).not.toContain("TASKS_CHANGED");
  });

  it("keeps line numbers", () => {
    expect(stripComments("a\n/* x\ny */\nb").split("\n")).toHaveLength(4);
  });
});

describe("the tasks-changed emit gate", () => {
  it("passes a tree with one emit, in announce_changed, and comments that name it", () => {
    const { problems, files } = scan(fixture());
    expect(problems).toEqual([]);
    expect(files).toBe(3);
  });

  it("fails on a second emit of the constant anywhere else (the seventh site)", () => {
    const problems = problemsOf({
      "src-tauri/src/commands/seventh.rs": `fn f(app: &AppHandle) {\n    let _ = app.emit(crate::tasks::TASKS_CHANGED, ());\n}\n`,
    });
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("src-tauri/src/commands/seventh.rs:2");
    expect(problems[0]).toContain("announce_changed");
  });

  it("fails on an alias, a use of the constant, or the event name as a literal", () => {
    expect(
      problemsOf({ "src-tauri/src/a.rs": `use crate::tasks::TASKS_CHANGED as CHANGED;\n` }),
    ).toHaveLength(1);
    expect(
      problemsOf({
        "src-tauri/src/a.rs": `fn f(app: &AppHandle) { let _ = app.emit("tasks://changed", ()); }\n`,
      }),
    ).toHaveLength(1);
    expect(
      problemsOf({ "src-tauri/src/a.rs": `const N: &str = r#"tasks://changed"#;\n` }),
    ).toHaveLength(1);
  });

  it("looks in every Rust file under src-tauri, tests and build scripts included", () => {
    for (const path of [
      "src-tauri/tests/x.rs",
      "src-tauri/build.rs",
      "src-tauri/src/deep/a/b/c.rs",
    ]) {
      expect(problemsOf({ [path]: `fn f() { emit(TASKS_CHANGED); }\n` })).toHaveLength(1);
    }
  });

  it("ignores a Rust file's comments, doc comments and a target directory", () => {
    expect(
      problemsOf({
        "src-tauri/src/a.rs": `/// emits TASKS_CHANGED, "tasks://changed"\n//! TASKS_CHANGED\nfn f() {} /* TASKS_CHANGED */\n`,
        "src-tauri/target/debug/gen.rs": `fn f() { emit(TASKS_CHANGED); }\n`,
      }),
    ).toEqual([]);
  });

  it("fails on a second use of the constant inside tasks.rs", () => {
    const problems = problemsOf({
      "src-tauri/src/tasks.rs":
        TASKS_RS +
        `\npub fn also(app: &tauri::AppHandle) {\n    let _ = app.emit(TASKS_CHANGED, ());\n}\n`,
    });
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("outside `announce_changed`");
  });

  it("fails when tasks.rs loses its constant, its function or the emit in it", () => {
    expect(
      problemsOf({
        "src-tauri/src/tasks.rs": TASKS_RS.replace("pub fn announce_changed", "pub fn announce"),
      }).join("\n"),
    ).toContain("no longer defines `pub fn announce_changed`");
    expect(
      problemsOf({
        "src-tauri/src/tasks.rs": TASKS_RS.replace('"tasks://changed"', '"tasks://other"'),
      }).join("\n"),
    ).toContain("no longer defines `pub const TASKS_CHANGED`");
    expect(
      problemsOf({
        "src-tauri/src/tasks.rs": TASKS_RS.replace("app.emit(TASKS_CHANGED, ())", "()"),
      }).join("\n"),
    ).toContain("no longer emits");
    expect(problemsOf({ "src-tauri/src/tasks.rs": null }).join("\n")).toContain("is missing");
  });
});
