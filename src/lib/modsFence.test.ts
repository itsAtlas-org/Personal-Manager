// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The lint fence around first-party mods (eslint.config.js's MODS_* constants; Mods spec §7). A mod
// is a sealed frame, so its source may import nothing from PM's src/ except src/kit/**, no Tauri API,
// no dynamic import(), no import.meta.glob, and none of the page's HTML sinks or frame makers. Linted
// through ESLint's own API with the repo's config and without type information (the fence needs
// none), like frameFence.test.ts. The paths are the repo's, read from where vitest runs (its root).
//
// These guard a first-party author's mistakes, not a mod written to evade them (that is the frame's
// sandbox); the cases are the shapes a mistake takes.

import { ESLint } from "eslint";
import { describe, expect, it } from "vitest";

const eslint = new ESLint({
  overrideConfig: [
    {
      files: ["mods/**/*.{ts,tsx}"],
      languageOptions: { parserOptions: { projectService: false, project: null } },
      rules: {
        "@typescript-eslint/no-floating-promises": "off",
        "@typescript-eslint/no-misused-promises": "off",
      },
    },
  ],
});

const MAIN = "mods/atlas.map/src/main.ts";
const WORKER = "mods/atlas.map/src/worker/layout.worker.ts";
const TEST = "mods/atlas.map/src/main.test.ts";
const DECLARED = "declare const el: HTMLElement; declare const s: string;\n";

/** The fence's messages (import fences and syntax bans) for `code` linted as the file at `filePath`. */
async function refusals(code: string, filePath: string): Promise<string[]> {
  const [result] = await eslint.lintText(DECLARED + code, { filePath });
  expect(result.messages.filter((m) => m.fatal)).toEqual([]);
  return result.messages
    .filter((m) =>
      ["mods/no-core-imports", "no-restricted-imports", "no-restricted-syntax"].includes(
        m.ruleId ?? "",
      ),
    )
    .map((m) => m.message);
}

describe("the mods fence: imports", { timeout: 60_000 }, () => {
  it.each([
    'import { x } from "../../../src/lib/ipc";',
    'import { x } from "../../../src/components/mods/modBridge";',
    'import x from "../../../src/lib/ipc";',
    'import "../../../src/index.css";',
    'export { x } from "../../../src/lib/ipc";',
    'export * from "../../../src/lib/ipc";',
    'import { x } from "../../../src/lib/ipc.ts";',
    'import css from "../../../src/index.css?raw";',
    // `kit/..` climbs back out of the one allowed folder.
    'import { x } from "../../../src/kit/../lib/ipc";',
    // A bare or absolute path into src/ names no package.
    'import { x } from "src/lib/ipc";',
    'import { x } from "src/kit/../lib/ipc";',
    'import { x } from "/src/lib/ipc";',
    'import { x } from "C:/repo/src/lib/ipc";',
  ])("refuses %s", async (code) => {
    const found = await refusals(code, MAIN);
    expect(found.join("\n"), code).toContain("only through the bridge");
  });

  it("holds in a worker file and in a test too", async () => {
    const code = 'import { x } from "../../../../src/lib/ipc";';
    expect(await refusals(code, WORKER)).not.toEqual([]);
    expect(await refusals('import { x } from "../../../src/lib/ipc";', TEST)).not.toEqual([]);
  });

  it.each([
    'import { x } from "../../../src/kit/host";',
    'import { x } from "../../../src/kit";',
    'import { x } from "src/kit/host";',
    // The mod's OWN src/ folder is not PM's src/: these resolve under mods/atlas.map/src.
    'import { x } from "./id";',
    'import { x } from "./worker/layout.worker";',
    'import { x } from "../src/id";',
    'import { x } from "../manifest.json";',
    // A folder that only looks like it (PM's own components/mods folder, a src-prefixed name).
    'import { x } from "../../../src-tauri/anything";',
    'import { x } from "d3-force";',
    'import { x } from "src-map";',
  ])("allows %s", async (code) => {
    expect(await refusals(code, MAIN)).toEqual([]);
  });

  it("allows a file in mods/ outside atlas.map's tree to reach the same places", async () => {
    // Resolved against the importing file: one level up from mods/x/ is mods/, not src/.
    expect(await refusals('import { x } from "../tsconfig-shared";', "mods/x/main.ts")).toEqual([]);
    expect(await refusals('import { x } from "../../src/lib/ipc";', "mods/x/main.ts")).not.toEqual(
      [],
    );
  });
});

describe("the mods fence: no Tauri API", { timeout: 60_000 }, () => {
  it.each([
    'import { invoke } from "@tauri-apps/api/core";',
    'import { listen } from "@tauri-apps/api/event";',
    'import { getCurrentWindow } from "@tauri-apps/api/window";',
    'import { core } from "@tauri-apps/api";',
    'import { open } from "@tauri-apps/plugin-dialog";',
    'import * as t from "@tauri-apps/api";',
  ])("refuses %s", async (code) => {
    const found = await refusals(code, MAIN);
    expect(found.join("\n"), code).toContain("no Tauri API");
  });
});

describe("the mods fence: one classic script, no bulk reads", { timeout: 60_000 }, () => {
  it.each([
    'void import("./id");',
    'void import("../../../src/kit/host");',
    "void import(s);",
    'void import.meta.glob("./*.ts");',
    'void import.meta.glob("../../../src/**/*.ts");',
  ])("refuses %s", async (code) => {
    expect(await refusals(code, MAIN), code).not.toEqual([]);
  });
});

describe("the mods fence: the page's sinks and frames", { timeout: 60_000 }, () => {
  it.each([
    "el.innerHTML = s;",
    'el["outerHTML"] = s;',
    "new DOMParser();",
    'el.insertAdjacentHTML("beforeend", s);',
    "document.write(s);",
    'document.createElement("iframe");',
    'document.createElement("object");',
    "el.setAttribute('srcdoc', s);",
    'const Tag = "iframe";',
    "document.createElement(s);",
  ])("refuses %s", async (code) => {
    expect(await refusals(code, MAIN), code).not.toEqual([]);
    expect(await refusals(code, WORKER), code).not.toEqual([]);
  });

  it("lets a mod's test drive a sink, as src/ tests do", async () => {
    expect(await refusals("el.innerHTML = s;", TEST)).toEqual([]);
    expect(await refusals('document.createElement("iframe");', TEST)).toEqual([]);
  });

  it("leaves a mod's built stage dir alone, and nothing else", async () => {
    // mods/<id>/dist is Vite's bundle, not source: linting it would audit minified output.
    expect(await eslint.isPathIgnored("mods/atlas.map/dist/assets/app.js")).toBe(true);
    expect(await eslint.isPathIgnored("mods/atlas.map/src/main.ts")).toBe(false);
    expect(await eslint.isPathIgnored("mods/atlas.map/src/dist/main.ts")).toBe(false);
  });

  it("lints the real skeleton clean", async () => {
    const results = await eslint.lintFiles(["mods"]);
    expect(results.length).toBeGreaterThan(0);
    expect(results.flatMap((r) => r.messages)).toEqual([]);
  });
});
