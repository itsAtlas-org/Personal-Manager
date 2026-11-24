// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The lint fence around the mod kit (eslint.config.js, the `src/kit/**` block; Mods spec D14). The kit
// is compiled into a mod's page, so it may not reach the IPC layer or a Tauri API (a frame has
// neither), and it carries the mods' own bans on HTML sinks, frame makers, dynamic import() and
// import.meta.glob. It is also CLOSED under its own imports: a kit file imports only src/kit/**,
// packages and `import type` from src/lib/types.ts, so a core module that itself imports `ipc` cannot
// be dragged into a mod's bundle through the kit (K6b). Linted through ESLint's own API with the
// repo's config, without type information (the fence needs none), like modsFence.test.ts. It guards a
// first-party author's mistakes.

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
    .filter((m) =>
      ["no-restricted-imports", "no-restricted-syntax", "kit/only-kit-imports"].includes(
        m.ruleId ?? "",
      ),
    )
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

  // The closure rule (K6b): planted imports of core files. `linkFailure` imports `openUrl` from
  // `lib/ipc`, so a kit file importing it would ship `ipc` and Tauri in a mod's bundle with no `ipc`
  // import of its own in the kit: only the closure rule sees it.
  it.each([
    'import { openLink } from "../lib/linkFailure";',
    'import { openLink } from "../../lib/linkFailure";',
    'import { openLink } from "src/lib/linkFailure";',
    'import { CopyLinkButton } from "../components/ui/CopyLinkButton";',
    'import { cn } from "../components/ui/cn";',
    'import { cn } from "../components/ui";',
    'import { formatClock } from "../lib/format";',
    'import { todayIso } from "../../../src/lib/dateField";',
    'import { x } from "../../tests/sealed-frame/x";',
    'import { x } from "/src/lib/format";',
    'export { openLink } from "../lib/linkFailure";',
    'export * from "../lib/linkFailure";',
    "const m = (): Promise<unknown> => import('../lib/linkFailure');",
    // Only `import type` from lib/types is allowed: a value import of it, or a type from another file.
    'import { listTasks } from "../lib/types";',
    'import { type Task } from "../lib/types";',
    'import type { Task } from "../lib/tasks";',
    'import type { Task } from "../lib/ipc";',
    'export { type Task } from "../lib/types";',
    // A path that only looks like the kit.
    'import { x } from "../kit/../lib/linkFailure";',
    'import { x } from "../kitten/x";',
  ])("refuses a core import: %s", async (code) => {
    expect((await refusals(code, FILE)).length, code).toBeGreaterThan(0);
  });

  // A worker or asset URL is a file reference the import rule does not read: the bundler builds the
  // named file into the mod's bundle, a core module and its `ipc` with it.
  it.each([
    'new Worker(new URL("../lib/ipc.ts", import.meta.url), { type: "module" });',
    'new Worker(new URL("../../lib/linkFailure.ts", import.meta.url));',
    'const u = new URL("../lib/format.ts", import.meta.url);',
  ])("refuses a file URL from import.meta.url: %s", async (code) => {
    expect((await refusals(code, FILE)).length, code).toBeGreaterThan(0);
  });

  it("still allows a URL that is not a file reference", async () => {
    expect(await refusals("const u = new URL(s, 'https://example.org/');", FILE)).toEqual([]);
    expect(await refusals("const b = new Worker(s);", FILE)).toEqual([]);
  });

  it("names the closure in its message", async () => {
    const [message] = await refusals('import { openLink } from "../lib/linkFailure";', FILE);
    expect(message).toContain("The kit imports only the kit");
  });

  it.each([
    'import type { Task } from "../lib/types";',
    'import type { Task } from "../lib/types.ts";',
    'import type { Task } from "src/lib/types";',
    'export type { Task } from "../lib/types";',
    'import { x } from "./host";',
    'import { x } from "./ui/cn";',
    'import { x } from "../kit/lib/format";',
    'import { x } from "./lib/format.ts";',
    'import { useState } from "react";',
    'import { ipcLike } from "./ipcLike";',
  ])("allows %s", async (code) => {
    expect(await refusals(code, FILE)).toEqual([]);
  });

  it("allows what is inside the kit from a nested kit file", async () => {
    for (const code of [
      'import { cn } from "./cn";',
      'import { useTheme } from "../theme";',
      'import { formatClock } from "../lib/format";',
      'import type { Task } from "../../lib/types";',
    ]) {
      expect(await refusals(code, "src/kit/ui/Button.tsx"), code).toEqual([]);
    }
    // A kit file one level down reaches core by `../../`, not `../`.
    expect(
      (await refusals('import { cn } from "../../components/ui/cn";', "src/kit/ui/Button.tsx"))
        .length,
    ).toBeGreaterThan(0);
  });

  // The two named exceptions, until their constants move into the kit (and host.test.ts, the test of
  // host.ts, reads the same bridge and the Rust source whose limits it pins). Nothing else gets one.
  it("lets host.ts, fakePm.ts and host.test.ts import the modBridge, and no other kit file", async () => {
    const bridge = 'import { readVectors } from "../components/mods/modBridge";';
    for (const file of ["src/kit/host.ts", "src/kit/fakePm.ts", "src/kit/host.test.ts"]) {
      expect(await refusals(bridge, file), file).toEqual([]);
    }
    expect((await refusals(bridge, "src/kit/spawnWorker.ts")).length).toBeGreaterThan(0);
    expect(
      (await refusals('import { x } from "../components/mods/other";', "src/kit/host.ts")).length,
    ).toBeGreaterThan(0);
    expect(
      await refusals(
        'import limits from "../../src-tauri/src/mods/api/limits.rs?raw";',
        "src/kit/host.test.ts",
      ),
    ).toEqual([]);
    expect(
      (
        await refusals(
          'import limits from "../../src-tauri/src/mods/api/limits.rs?raw";',
          "src/kit/spawnWorker.test.ts",
        )
      ).length,
    ).toBeGreaterThan(0);
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
