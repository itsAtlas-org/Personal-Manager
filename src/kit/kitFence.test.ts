// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The lint fence around the mod kit (eslint.config.js, the `src/kit/**` block; Mods spec D14). The kit
// is compiled into a mod's page, so it may not reach the IPC layer or a Tauri API (a frame has
// neither), and it carries the mods' own bans on HTML sinks, frame makers, dynamic import() and
// import.meta.glob. Linted through ESLint's own API with the repo's config, without type information
// (the fence needs none), like modsFence.test.ts. It guards a first-party author's mistakes.

import { ESLint } from "eslint";
import { describe, expect, it } from "vitest";

const eslint = new ESLint({
  overrideConfig: [
    {
      files: ["src/kit/**/*.{ts,tsx}"],
      languageOptions: { parserOptions: { projectService: false, project: null } },
      rules: {
        "@typescript-eslint/no-floating-promises": "off",
        "@typescript-eslint/no-misused-promises": "off",
      },
    },
  ],
});

const FILE = "src/kit/host.ts";
const TEST = "src/kit/host.test.ts";
const DECLARED = "declare const el: HTMLElement; declare const s: string;\n";

async function refusals(code: string, filePath: string): Promise<string[]> {
  const [result] = await eslint.lintText(DECLARED + code, { filePath });
  expect(result.messages.filter((m) => m.fatal)).toEqual([]);
  return result.messages
    .filter((m) => ["no-restricted-imports", "no-restricted-syntax"].includes(m.ruleId ?? ""))
    .map((m) => m.message);
}

describe("the kit fence", { timeout: 60_000 }, () => {
  it.each([
    'import { modCall } from "../lib/ipc";',
    'import { modCall } from "../lib/ipc.ts";',
    'import { modCall } from "src/lib/ipc";',
    'export { modCall } from "../lib/ipc";',
    'import { invoke } from "@tauri-apps/api/core";',
    'import { listen } from "@tauri-apps/api/event";',
    'import { open } from "@tauri-apps/plugin-shell";',
    'import * as tauri from "@tauri-apps/api";',
  ])("refuses %s", async (code) => {
    expect((await refusals(code, FILE)).length, code).toBeGreaterThan(0);
  });

  it.each([
    "el.innerHTML = s;",
    "el.insertAdjacentHTML('beforeend', s);",
    "new DOMParser();",
    "document.createElement(s);",
    "document.createElement('iframe');",
    "const m = () => import('./other');",
    "const f = import.meta.glob('./*.ts');",
  ])("carries the mods' own bans: %s", async (code) => {
    expect((await refusals(code, FILE)).length, code).toBeGreaterThan(0);
  });

  it.each([
    'import { readVectors } from "../components/mods/modBridge";',
    'import { x } from "./host";',
    'import { useState } from "react";',
    'import { cn } from "../components/ui/cn";',
    'import { ipcLike } from "./ipcLike";',
  ])("allows %s", async (code) => {
    expect(await refusals(code, FILE)).toEqual([]);
  });

  it("lets the kit's tests drive a sink and a dynamic import, as src/ tests do", async () => {
    expect(await refusals("el.innerHTML = s; const m = () => import('./other');", TEST)).toEqual(
      [],
    );
  });

  it("leaves the rest of src/ as it was: ipc is still the app's own door there", async () => {
    const [result] = await eslint.lintText('import { modCall } from "../lib/ipc";\nvoid modCall;', {
      filePath: "src/components/mods/ModFrame.tsx",
    });
    expect(result.messages.filter((m) => m.ruleId === "no-restricted-imports")).toEqual([]);
  });

  it("lints the real kit clean", async () => {
    const results = await eslint.lintFiles(["src/kit"]);
    const found = results.flatMap((r) =>
      r.messages.map((m) => `${r.filePath}:${m.line} ${m.ruleId}: ${m.message}`),
    );
    expect(found).toEqual([]);
  });
});
