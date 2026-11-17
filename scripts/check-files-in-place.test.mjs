// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The files-in-place gate's rules for the mods folder. The gate reads `git ls-files` from the repo
// its own file sits in, so each case builds a throwaway git repo with a copy of the gate in it and
// stages the smallest tree that trips (or spares) one rule.

import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it } from "vitest";

import vitestConfig from "../vitest.config.ts";

const gate = join(dirname(fileURLToPath(import.meta.url)), "check-files-in-place.mjs");

const roots = [];
afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

/** Runs the gate over a repo that tracks `files` (plus the gate itself under scripts/, which is
 *  allowed at the root) and returns its exit status and output. */
function run(files) {
  const root = mkdtempSync(join(tmpdir(), "pm-files-in-place-"));
  roots.push(root);
  mkdirSync(join(root, "scripts"));
  copyFileSync(gate, join(root, "scripts", "check-files-in-place.mjs"));
  for (const [path, text] of Object.entries(files)) {
    mkdirSync(join(root, dirname(path)), { recursive: true });
    writeFileSync(join(root, path), text);
  }
  const git = (...args) => execFileSync("git", args, { cwd: root, stdio: "ignore" });
  git("init", "--quiet");
  git("add", "--force", "--all");
  const r = spawnSync(process.execPath, [join(root, "scripts", "check-files-in-place.mjs")], {
    cwd: root,
    encoding: "utf8",
  });
  return { ok: r.status === 0, out: r.stdout + r.stderr };
}

describe("files-in-place and the mods folder", () => {
  it("lets a mod's source, manifest and collected test be tracked", () => {
    const r = run({
      "mods/atlas.map/manifest.json": "{}",
      "mods/atlas.map/src/main.ts": "export {};",
      "mods/atlas.map/src/main.test.ts": "export {};",
      "mods/atlas.map/src/ui.test.tsx": "export {};",
      "mods/tsconfig.json": "{}",
    });
    expect(r.out).toContain("files-in-place");
    expect(r.ok).toBe(true);
  });

  it("refuses a packed mod, wherever it sits and in whatever case", () => {
    for (const path of [
      "mods/atlas.map.pmmod",
      "atlas.map-1.0.0.pmmod",
      "tests/fixtures/acme.hello.PMMOD",
    ]) {
      const r = run({ [path]: "zip" });
      expect(r.ok, path).toBe(false);
      expect(r.out, path).toContain("a packed mod");
    }
  });

  it("refuses a tracked stage dir of a mod, and only that one", () => {
    const stage = run({ "mods/atlas.map/dist/app/index.html": "<html></html>" });
    expect(stage.ok).toBe(false);
    expect(stage.out).toContain("built stage dir");
    // A folder that merely has `dist` in its name, or a source folder, is not the stage dir.
    const near = run({
      "mods/atlas.map/src/dist.ts": "export {};",
      "mods/atlas.map/src/distance/a.ts": "export {};",
    });
    expect(near.ok).toBe(true);
  });

  it("refuses the same build output of the toolchain's fixture mod, and its build record", () => {
    for (const path of [
      "tests/mod-fixtures/acme.hello/dist/app/index.html",
      "tests/mod-fixtures/acme.hello/dist/acme.hello.txt",
    ]) {
      const r = run({ [path]: "x" });
      expect(r.ok, path).toBe(false);
      expect(r.out, path).toContain("built stage dir");
    }
    for (const path of [
      "mods/atlas.map/.build/modules.json",
      "tests/mod-fixtures/acme.hello/.build/modules.json",
    ]) {
      const r = run({ [path]: "{}" });
      expect(r.ok, path).toBe(false);
      expect(r.out, path).toContain("build record");
    }
    // The fixture's source, manifest and golden file are tracked on purpose.
    const source = run({
      "tests/mod-fixtures/acme.hello/manifest.json": "{}",
      "tests/mod-fixtures/acme.hello/src/index.html": "<html></html>",
      "tests/mod-fixtures/golden/hello.pmmod.b64": "UEs=",
    });
    expect(source.ok).toBe(true);
  });

  it("refuses a mod test file no runner collects", () => {
    for (const path of ["mods/atlas.map/src/a.test.mjs", "mods/atlas.map/src/a.test.js"]) {
      const r = run({ [path]: "export {};" });
      expect(r.ok, path).toBe(false);
      expect(r.out, path).toContain("test file no runner collects");
      expect(r.out, path).toContain("mods/**/*.test.{ts,tsx}");
    }
  });

  it("knows the mods folder, and still refuses a stray root entry beside it", () => {
    const r = run({ "mods/tsconfig.json": "{}", "stray-notes/a.txt": "x" });
    expect(r.ok).toBe(false);
    expect(r.out).toContain("stray at repo root: stray-notes");
    expect(r.out).not.toContain("stray at repo root: mods");
  });
});

// The gate's COLLECTED list says vitest picks up `mods/**` tests, and nothing but this checks that
// vitest's own config agrees: a config that dropped the glob would leave the gate green over mod
// tests that no runner runs.
describe("vitest.config.ts and the mods folder", () => {
  it("includes the mods tests", () => {
    expect(vitestConfig.test.include).toContain("mods/**/*.test.{ts,tsx}");
  });
});
