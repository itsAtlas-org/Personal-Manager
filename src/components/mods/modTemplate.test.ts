// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The starter mod (`mod-template/`, Mods spec 1.15) is what a mod author copies, so it must stay in
// step with PM: its SDK file is generated from `src/kit/host.ts`, its licence is the repo's, its page
// obeys the frame's policy and its script asks for nothing its manifest does not. Rust's
// `the_mod_template_loads_in_a_dev_folder` (src-tauri/src/mods/package.rs) holds the other half: that
// PM's own folder loader accepts the folder.
//
// `app/pm-host.js` is the SDK built as one classic script that defines `PmHost` (a frame may load
// only classic scripts from its own folder, and the template has no build step). A change to
// `src/kit/host.ts` (or to `modBridge.ts`, which it bundles) changes what that build says, and this
// test fails until the file is regenerated:
//
//     npx vitest run src/components/mods/modTemplate.test.ts -u
//
// A Dependabot batch that moves vite or rolldown can change the output too: regenerate it the same
// way and say so in the PR.

import { build } from "vite";
import { describe, expect, it } from "vitest";
import licence from "../../../LICENCE.txt?raw";
import templateLicence from "../../../mod-template/LICENCE.txt?raw";
import indexHtml from "../../../mod-template/app/index.html?raw";
import mainJs from "../../../mod-template/app/main.js?raw";
import manifestText from "../../../mod-template/manifest.json?raw";
import callTable from "../../../src-tauri/src/mods/api/table.rs?raw";

const HEADER =
  "// SPDX-FileCopyrightText: 2026 Bobby Yu\n// SPDX-License-Identifier: AGPL-3.0-or-later\n";

/** The SDK as the template ships it: `src/kit/host.ts` as one unminified IIFE named `PmHost`. */
async function builtSdk(): Promise<string> {
  const out = await build({
    configFile: false,
    logLevel: "silent",
    envDir: false,
    build: {
      write: false,
      minify: false,
      target: "es2022",
      lib: { entry: "src/kit/host.ts", formats: ["iife"], name: "PmHost", fileName: "pm-host" },
      rolldownOptions: { output: { banner: HEADER } },
    },
  });
  const outputs = Array.isArray(out) ? out : [out];
  const chunks = outputs
    .flatMap((o) => ("output" in o ? o.output : []))
    .filter((o) => o.type === "chunk");
  // One file: a build that split would leave the template's page without its other half.
  if (chunks.length !== 1) throw new Error("the SDK must build to exactly one chunk");
  return chunks[0].code;
}

describe("the mod template's SDK file", { timeout: 60_000 }, () => {
  it("is src/kit/host.ts as one classic script", async () => {
    const code = await builtSdk();
    await expect(code).toMatchFileSnapshot("../../../mod-template/app/pm-host.js");
  });

  it("is a classic script that defines PmHost.connectHost, under the two-line header", async () => {
    const code = await builtSdk();
    expect(code.startsWith(HEADER)).toBe(true);
    expect(code).toMatch(/^var PmHost = /m);
    expect(code).toMatch(/exports\.connectHost = connectHost/);
    // Nothing a classic script cannot carry.
    expect(code).not.toMatch(/^\s*(import|export)\s/m);
    expect(code).not.toMatch(/import\.meta/);
  });
});

describe("the mod template's files", () => {
  it("carries the repo's licence text, byte for byte", () => {
    expect(templateLicence).toBe(licence);
    expect(templateLicence).toContain("GNU AFFERO GENERAL PUBLIC LICENSE");
  });

  it("has a page with no inline code and no module script, only its own two scripts", () => {
    // The page is read as it is, comments included: pack-mod's lint (scripts/pack-mod.mjs
    // checkBuiltHtml) reads raw text and refuses a comment that mentions `<script`, so a template
    // that passed here with one in a comment would still be refused by the packer.
    const page = indexHtml;
    expect(page.match(/<script/gi)).toHaveLength(2);
    const scripts = [...page.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script\s*>/gi)];
    expect(scripts.map((m) => /src="([^"]+)"/.exec(m[1])?.[1])).toEqual([
      "./pm-host.js",
      "./main.js",
    ]);
    for (const [, attrs, body] of scripts) {
      expect(body.trim(), "inline script").toBe("");
      expect(attrs).not.toMatch(/type\s*=/i);
    }
    expect(page).not.toMatch(/<style\b/i);
    expect(page).not.toMatch(/\sstyle\s*=/i);
    // No inline handler (onclick=...), which the frame's policy refuses like an inline script.
    expect(page).not.toMatch(/\son[a-z]+\s*=/i);
    // The stylesheet is the template's own file.
    expect(page).toContain('href="./style.css"');
    // The frame loads only the mod's own app/ files: every href and src is relative (no scheme, no
    // `//` host), and the two attributes the packer refuses outright are absent.
    const urls = [...page.matchAll(/\s(?:href|src)\s*=\s*"([^"]*)"/gi)].map((m) => m[1]);
    expect(urls.length).toBeGreaterThan(0);
    for (const url of urls)
      expect(url, "a url outside the mod's own files").toMatch(/^\.\/[\w.-]+$/);
    expect(page).not.toMatch(/crossorigin/i);
    expect(page).not.toMatch(/srcdoc/i);
    expect(page).not.toMatch(/javascript:/i);
  });

  it("asks PM only for what its manifest declares", () => {
    const manifest = JSON.parse(manifestText) as {
      permissions: { id: string }[];
      data: { keys: { key: string }[] };
    };
    const declared = manifest.permissions.map((p) => p.id).sort();
    expect(declared).toEqual(["storage", "ui.tab"]);

    // The permission each call or event the script names needs, read from the call table itself
    // (src-tauri/src/mods/api/table.rs), so a hand-kept copy cannot drift from what Rust enforces.
    const NEEDS = new Map<string, string>();
    for (const m of callTable.matchAll(/^\s*call\("([^"]+)",\s*"([^"]+)"/gm)) NEEDS.set(m[1], m[2]);
    for (const m of callTable.matchAll(/^\s*event\("([^"]+)",\s*Some\("([^"]+)"\)/gm)) {
      NEEDS.set(m[1], m[2]);
    }
    expect(NEEDS.get("theme.get"), "the table was not read").toBe("storage");
    const used = new Set<string>();
    for (const m of mainJs.matchAll(/\.call\(\s*"([^"]+)"/g)) used.add(m[1]);
    for (const m of mainJs.matchAll(/event\.name\s*===\s*"([^"]+)"/g)) used.add(m[1]);
    expect(used.size).toBeGreaterThan(0);
    for (const name of used) {
      const need = NEEDS.get(name);
      expect(need, `${name} is not a call or event with a permission in the table`).toBeDefined();
      expect(declared, `${name} needs ${need}, which the manifest does not declare`).toContain(
        need,
      );
    }

    // And the key it saves under is one the manifest declares.
    const key = /var KEY = "([^"]+)"/.exec(mainJs)?.[1];
    expect(manifest.data.keys.map((k) => k.key)).toContain(key);
  });

  it("calls the host only through the SDK's global, in a classic script", () => {
    expect(mainJs).toContain("PmHost.connectHost()");
    expect(mainJs).not.toMatch(/^\s*(import|export)\s/m);
  });
});
