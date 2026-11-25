// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The real build of a mod (mods/vite.config.ts) and the package made from it (scripts/pack-mod.mjs),
// end to end. scripts/pack-mod.test.mjs covers the packer on hand-made folders; this file runs Vite,
// because the properties that matter here are about what Vite emits and cannot be faked:
//  - the page is one CLASSIC script and one stylesheet, with no `crossorigin` and no `data:` URL
//    (the frame's CSP forbids all three; B0 measured that Vite writes every one of them by default);
//  - a worker is ONE string literal in that script, and the NOTICE still names the worker's packages
//    (the nested worker build is invisible to the outer one);
//  - two builds give the same bytes, wherever they run, with no absolute path in them;
//  - what pack-mod then writes passes its own verifier.
// It builds tests/mod-fixtures/acme.hello (the third-party fixture), tests/mod-fixtures/react-tailwind
// (a React page with Tailwind classes, which has no manifest: only the build is under test) and the
// real mods/atlas.map, in place: all write only into their git-ignored dist/ and .build/.

import { createHash } from "node:crypto";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { build } from "vite";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import {
  MODULES_RECORD,
  modConfig,
  onlyChunk,
  outputDigest,
  packageModulePath,
} from "../mods/vite.config.ts";
import * as pack from "./pack-mod.mjs";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const FIXTURE = join(repoRoot, "tests/mod-fixtures/acme.hello");
const MAP = join(repoRoot, "mods/atlas.map");
const REACT_FIXTURE = join(repoRoot, "tests/mod-fixtures/react-tailwind");
const COMMIT = "abcdef0123456789abcdef0123456789abcdef01";

const scratch = [];
afterAll(() => {
  for (const dir of scratch) rmSync(dir, { recursive: true, force: true });
});

/** The build the recipe runs: this mod's config, no config file, nothing printed. */
async function buildMod(modDir) {
  rmSync(join(modDir, "dist"), { recursive: true, force: true });
  rmSync(join(modDir, ".build"), { recursive: true, force: true });
  await build({ ...modConfig(modDir), configFile: false, logLevel: "silent" });
}

/** A throwaway mod outside the repo (so no node_modules is reachable): sources in, build out. */
function scratchMod(files) {
  const dir = mkdtempSync(join(tmpdir(), "mods-build-test-"));
  scratch.push(dir);
  for (const [path, text] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, "src", path)), { recursive: true });
    writeFileSync(join(dir, "src", path), text);
  }
  return dir;
}
const read = (modDir, path) => readFileSync(join(modDir, "dist", "app", path), "utf8");
const listFiles = (dir, rel = "") =>
  readdirSync(join(dir, rel), { withFileTypes: true }).flatMap((e) =>
    e.isDirectory() ? listFiles(dir, join(rel, e.name)) : [join(rel, e.name)],
  );
const snapshot = (modDir) =>
  Object.fromEntries(
    [...listFiles(join(modDir, "dist", "app")), ".record"]
      .sort()
      .map((f) => [
        f,
        f === ".record"
          ? readFileSync(join(modDir, MODULES_RECORD), "utf8")
          : readFileSync(join(modDir, "dist", "app", f)).toString("base64"),
      ]),
  );

const PAGE =
  '<!doctype html><html><head><title>t</title></head><body><script type="module" src="./main.ts"></script></body></html>';

describe("the build of the fixture mod", () => {
  beforeAll(async () => {
    await buildMod(FIXTURE);
  }, 60_000);

  it("emits one classic script, one stylesheet and the page, under fixed names", () => {
    expect(listFiles(join(FIXTURE, "dist", "app")).sort()).toEqual([
      join("assets", "app.js"),
      join("assets", "style.css"),
      "index.html",
    ]);
  });

  it("writes the page as classic script + stylesheet: no module script, no crossorigin, no preload", () => {
    const html = read(FIXTURE, "index.html");
    expect(html).toContain('<script defer src="./assets/app.js"></script>');
    expect(html).toContain('<link rel="stylesheet" href="./assets/style.css">');
    expect(html).not.toMatch(/type\s*=\s*["']?module/i);
    expect(html).not.toMatch(/crossorigin/i);
    expect(html).not.toMatch(/modulepreload/i);
    // The same independent check pack-mod --verify applies to every file of the package.
    for (const f of listFiles(join(FIXTURE, "dist", "app"))) {
      pack.checkBuiltFile(
        `app/${f.split("\\").join("/")}`,
        readFileSync(join(FIXTURE, "dist", "app", f)),
      );
    }
  });

  it("wraps the page's script in an IIFE, so nothing it declares reaches the frame's global scope", () => {
    const js = read(FIXTURE, "assets/app.js").trim();
    expect(js.startsWith("(function")).toBe(true);
    expect(js.endsWith("})();")).toBe(true);
  });

  it("bundles the worker as one string literal, with no atob and no import.meta", () => {
    const js = read(FIXTURE, "assets/app.js");
    expect(js).toContain("onmessage"); // the worker's own code is inside the page's script
    expect(js).not.toContain("atob(");
    expect(js).not.toContain("import.meta");
    expect(js).not.toContain("?worker");
    expect(js).toContain("new Worker(");
    expect(js).toContain("new Blob([");
  });

  it("records the packages of the page AND of the worker, as sorted repo-relative module paths", () => {
    const record = JSON.parse(readFileSync(join(FIXTURE, MODULES_RECORD), "utf8"));
    expect(record.format).toBe(1);
    const dirs = [...new Set(record.modules.map(pack.packageDirOf))];
    // d3-quadtree is the page's; d3-force and what only it pulls in (d3-dispatch, d3-timer) are the
    // worker's, which the outer build never sees.
    expect(dirs).toEqual([
      "node_modules/d3-dispatch",
      "node_modules/d3-force",
      "node_modules/d3-quadtree",
      "node_modules/d3-timer",
    ]);
    expect(record.modules).toEqual([...record.modules].sort());
    for (const m of record.modules) {
      expect(m.startsWith("node_modules/")).toBe(true);
      expect(m).not.toContain(repoRoot);
    }
  });

  it("the record's digest of the output is the one pack-mod recomputes, from either side", () => {
    const record = JSON.parse(readFileSync(join(FIXTURE, MODULES_RECORD), "utf8"));
    const app = join(FIXTURE, "dist", "app");
    expect(record.bundle).toBe(outputDigest(app));
    expect(record.bundle).toBe(pack.outputDigest(app));
  });

  it("the two digests agree on names that sort differently as paths and as a folder walk", () => {
    // `a-b.txt` < `a/x.txt` as whole paths ('-' is 0x2d, '/' is 0x2f), though the folder `a` is listed first.
    const dir = scratchMod({});
    const app = join(dir, "dist", "app");
    for (const name of ["a/x.txt", "a-b.txt", "z.txt", "a.txt"]) {
      mkdirSync(dirname(join(app, name)), { recursive: true });
      writeFileSync(join(app, name), `content of ${name}`);
    }
    expect(outputDigest(app)).toBe(pack.outputDigest(app));
  });

  it("packs to a NOTICE that credits every package of the page and the worker", () => {
    const files = pack.assemble({ modDir: FIXTURE, commit: COMMIT });
    const notice = files.get("NOTICE.txt").toString();
    for (const name of ["d3-dispatch", "d3-force", "d3-quadtree", "d3-timer"]) {
      expect(notice).toContain(`${name} `);
    }
    expect(notice).toContain("This mod bundles 4 npm packages.");
    expect(notice).toContain("SPDX-License-Identifier: ISC");
    expect(notice).toContain("Mike Bostock"); // a copyright line from a package's own LICENSE
  });

  it("packs --unsigned into a package its own verifier takes, with no absolute path in any file", () => {
    const bytes = pack.buildArchive(pack.assemble({ modDir: FIXTURE, commit: COMMIT }), {
      unsigned: true,
    });
    const result = pack.verifyArchive(bytes);
    expect(result.kind).toBe("unsigned");
    expect(result.manifest.id).toBe("acme.hello");
    expect(result.manifest.commit).toBe(COMMIT);
    expect([...result.files.keys()]).toContain("app/assets/app.js");
    for (const [name, data] of result.files) {
      expect(data.includes(repoRoot), `${name} holds this checkout's path`).toBe(false);
    }
  });

  it("builds the same bytes twice, even after every source file is touched", async () => {
    const first = snapshot(FIXTURE);
    const archive = pack.buildArchive(pack.assemble({ modDir: FIXTURE, commit: COMMIT }), {
      unsigned: true,
    });
    const later = new Date(Date.now() + 5_000);
    const touch = (dir) => {
      for (const f of listFiles(dir)) utimesSync(join(dir, f), later, later);
    };
    touch(join(FIXTURE, "src"));
    await buildMod(FIXTURE);
    expect(snapshot(FIXTURE)).toEqual(first);
    expect(
      pack
        .buildArchive(pack.assemble({ modDir: FIXTURE, commit: COMMIT }), { unsigned: true })
        .equals(archive),
    ).toBe(true);
  }, 60_000);

  it("refuses to pack output the build record is not for (a stale record, an edited dist)", () => {
    const edited = join(FIXTURE, "dist", "app", "assets", "style.css");
    const original = readFileSync(edited);
    try {
      writeFileSync(edited, `${original}\n/* edited after the build */`);
      expect(() => pack.assemble({ modDir: FIXTURE })).toThrow(/not for the output/);
    } finally {
      writeFileSync(edited, original);
    }
  });
});

describe("the real mods/atlas.map", () => {
  beforeAll(async () => {
    await buildMod(MAP);
  }, 60_000);

  it("builds and packs as a first-party package for signing, and its manifest stays valid", () => {
    const bytes = pack.buildArchive(pack.assemble({ modDir: MAP, commit: COMMIT }));
    const result = pack.verifyArchive(bytes);
    expect(result.kind).toBe("pre-sign");
    expect(result.manifest).toMatchObject({ id: "atlas.map", tier: "first", commit: COMMIT });
    // The Map's page bundles d3 (the by-project simulation, its timer and dispatch, and the hit-test
    // quadtree), so its NOTICE reproduces those four packages' licences (spec section 7): the names the
    // build record found in the bundle, and nothing else (the worker's own copy of d3-force is the
    // same package, listed once).
    const notice = result.files.get("NOTICE.txt").toString();
    expect(notice).toContain("This mod bundles 4 npm packages.");
    for (const name of ["d3-dispatch", "d3-force", "d3-quadtree", "d3-timer"]) {
      expect(notice).toContain(`\n${name} `);
    }
  });

  it("builds the same bytes with the React and Tailwind plugins as without them (D12)", async () => {
    // The Map has no JSX and no Tailwind import, so adding the plugins to the one mod config must
    // not move one byte of its bundle: a changed Map bundle would force a Map version. The "before"
    // is the config as it was, the three plugins of the mod build and nothing else.
    const full = modConfig(MAP);
    const own = full.plugins.flat(Infinity).filter((p) => p.name.startsWith("mod-"));
    const before = own.filter((p) => p.name !== "mod-tailwind-scan");
    // And without the two settings K6c added (production `define`, non-development JSX): the oracle
    // may not be the config under test.
    const old = { ...full };
    delete old.define;
    delete old.oxc;
    expect(before.map((p) => p.name).sort()).toEqual([
      "mod-classic-html",
      "mod-inline-worker",
      "mod-module-record",
    ]);
    expect(full.plugins.flat(Infinity).length).toBeGreaterThan(own.length);
    rmSync(join(MAP, "dist"), { recursive: true, force: true });
    await build({ ...old, plugins: before, configFile: false, logLevel: "silent" });
    const without = outputDigest(join(MAP, "dist", "app"));
    const withoutRecord = readFileSync(join(MAP, MODULES_RECORD), "utf8");
    await buildMod(MAP);
    expect(outputDigest(join(MAP, "dist", "app"))).toBe(without);
    expect(readFileSync(join(MAP, MODULES_RECORD), "utf8")).toBe(withoutRecord);
    // Pinned: the sha256 of each file of the Map's bundle as built just before K6c touched the config
    // (at 1828b037 plus C1's K1, whose three new error codes are in the kit's MOD_ERRORS, which the
    // Map bundles through src/kit/host.ts, plus K2, which adds `rateLimited` to MOD_ERRORS and
    // `ui.openTaskEditor` to PAGE_ACTIONS, the list `readReply` checks a page action against, plus K3, which
    // adds five names to MOD_EVENTS, the list `readEvent` lets through, and `flushed()` to the SDK). A comparison of two builds of one config cannot see a
    // change both share; this can. The bytes change when the Map's own folder, its build, or kit code
    // it bundles changes. Its own folder needs a Map version (D12, `check-mods --base`); a kit change
    // reaches users in the Map's next version (the published one is rebuilt from its recorded commit),
    // and the change that causes it edits these, saying why.
    const sha = (f) =>
      createHash("sha256")
        .update(readFileSync(join(MAP, "dist", "app", f)))
        .digest("hex");
    expect({
      "index.html": sha("index.html"),
      "assets/app.js": sha("assets/app.js"),
      "assets/style.css": sha("assets/style.css"),
    }).toEqual({
      "index.html": "774b8fd43cc22f9685ab10e2b3076e51ac413c9814a3225e364050ff03baa778",
      "assets/app.js": "efbac680085c2c0d16192ff66571ebaadad796fc0f2cd5c9fccf7ab28716c8c1",
      "assets/style.css": "d1175700cab84482c2f4bcaf64260fa02f18468f245a10d89b3e57fdfbfa2d20",
    });
    expect(JSON.parse(readFileSync(join(MAP, MODULES_RECORD), "utf8")).bundle).toBe(
      "f1aa0ea36f3f1a88387da31ec6dcfcdcd120c6f47e55cd859fe8f3565f8fa1c8",
    );
  }, 60_000);

  it("is refused as an unsigned file: an atlas. mod is never a developer's own", () => {
    expect(() => pack.buildArchive(pack.assemble({ modDir: MAP }), { unsigned: true })).toThrow(
      /third party/,
    );
  });

  it("holds exactly the manifest, licence, notice and app/ (what --stage writes beside it)", () => {
    const files = pack.assemble({ modDir: MAP });
    expect([...files.keys()].filter((n) => !n.startsWith("app/")).sort()).toEqual([
      "LICENCE.txt",
      "NOTICE.txt",
      "manifest.json",
    ]);
    expect(files.has("app/index.html")).toBe(true);
  });
});

describe("the build of a React and Tailwind mod", () => {
  const css = () => read(REACT_FIXTURE, "assets/style.css");
  // A class that only src/kit/ui/Button.tsx uses (the fixture's own source never writes it), so it
  // reaches the stylesheet only if the build scans the kit.
  const KIT_CLASS = "enabled:hover:bg-[color-mix(in_oklab,var(--st-due)_24%,transparent)]";

  beforeAll(async () => {
    await buildMod(REACT_FIXTURE);
  }, 60_000);

  it("emits the same shape as any mod: one script, one stylesheet, the page, no data: URL", () => {
    expect(listFiles(join(REACT_FIXTURE, "dist", "app")).sort()).toEqual([
      join("assets", "app.js"),
      join("assets", "style.css"),
      "index.html",
    ]);
    const html = read(REACT_FIXTURE, "index.html");
    expect(html).toContain('<script defer src="./assets/app.js"></script>');
    expect(html).not.toMatch(/type\s*=\s*["']?module|crossorigin/i);
    expect(css()).not.toContain("data:");
    const js = read(REACT_FIXTURE, "assets/app.js").trim();
    expect(js.startsWith("(function")).toBe(true);
    for (const f of listFiles(join(REACT_FIXTURE, "dist", "app"))) {
      pack.checkBuiltFile(
        `app/${f.split("\\").join("/")}`,
        readFileSync(join(REACT_FIXTURE, "dist", "app", f)),
      );
    }
  });

  it("compiles JSX (the page's markup is calls into React's automatic runtime, not tags)", () => {
    const js = read(REACT_FIXTURE, "assets/app.js");
    expect(js).toContain("JSX-RENDERED-MARKER");
    expect(js).not.toContain("<main");
    expect(js).not.toContain("<h1>");
    expect(js).not.toContain("React.createElement");
    const record = JSON.parse(readFileSync(join(REACT_FIXTURE, MODULES_RECORD), "utf8"));
    expect(record.modules.some((m) => m.endsWith("/react/jsx-runtime.js"))).toBe(true);
    // The production build of React, though this test runs with NODE_ENV=test: the bundle (and the
    // record) must not depend on the shell it was built in.
    expect(record.modules.filter((m) => /development|dev-runtime/.test(m))).toEqual([]);
    expect(js).not.toContain("process.env.NODE_ENV");
    expect(js).not.toContain(repoRoot);
  });

  it("writes the Tailwind classes of the page and of the kit it imports into the stylesheet", () => {
    expect(css()).toContain("13px"); // p-[13px], written in the fixture's own main.tsx
    expect(css()).toContain("c0ffee"); // text-[#c0ffee], the same
    expect(readFileSync(join(repoRoot, "src/kit/ui/Button.tsx"), "utf8")).toContain(KIT_CLASS);
    expect(css()).toContain("var(--st-due) 24%");
  });

  it("scans only the kit and the mod's own src/: a class anywhere else never reaches the stylesheet", () => {
    // decoy/Decoy.tsx sits beside src/ in the fixture folder, outside both scanned folders.
    expect(readFileSync(join(REACT_FIXTURE, "decoy", "Decoy.tsx"), "utf8")).toContain("p-[17px]");
    expect(css()).not.toContain("17px");
    expect(css()).not.toContain("decade");
  });

  it("does not scan a test file, in the mod's src/ or in the kit", async () => {
    const test = join(REACT_FIXTURE, "src", "Late.test.tsx");
    writeFileSync(test, 'export const x = "p-[19px] text-[#facade]";\n');
    try {
      await buildMod(REACT_FIXTURE);
      expect(css()).not.toContain("19px");
      expect(css()).not.toContain("facade");
      // The kit's own tests are written with classes of their own that no component uses.
      expect(readFileSync(join(repoRoot, "src/kit/ui/Button.test.tsx"), "utf8")).toMatch(
        /className=/,
      );
    } finally {
      rmSync(test, { force: true });
      await buildMod(REACT_FIXTURE);
    }
  }, 60_000);

  it("records react, react-dom and scheduler, and no Tauri API, and the NOTICE credits them", () => {
    const record = JSON.parse(readFileSync(join(REACT_FIXTURE, MODULES_RECORD), "utf8"));
    const dirs = [...new Set(record.modules.map(pack.packageDirOf))];
    for (const name of ["react", "react-dom", "scheduler"]) {
      expect(dirs, `modules.json lacks ${name}`).toContain(`node_modules/${name}`);
    }
    expect(dirs.filter((d) => d.startsWith("node_modules/@tauri-apps"))).toEqual([]);
    expect(record.bundle).toBe(outputDigest(join(REACT_FIXTURE, "dist", "app")));
    const notice = pack.noticeText(pack.bundledPackages(REACT_FIXTURE, repoRoot));
    for (const name of ["react", "react-dom", "scheduler"]) {
      expect(notice.toString()).toContain(`\n${name} `);
    }
  });

  it("refuses a stylesheet that widens the scan or imports Tailwind in another form", async () => {
    const original = readFileSync(join(REACT_FIXTURE, "src", "style.css"), "utf8");
    try {
      for (const [css, message] of [
        [`${original}\n@source "../../../../src/components";`, /@source/],
        [original.replace('"tailwindcss"', '"tailwindcss" source("../../../..")'), /one `@import/],
        [original.replace('"tailwindcss"', '"tailwindcss/utilities"'), /one `@import/],
        [`${original}\n@import "tailwindcss";`, /one `@import/],
        // Tailwind loads these files itself, so their content never reaches the plugin: refused by name.
        [`${original}\n@import "./extra.css";`, /may not use @import/],
        [`${original}\n@import url("./extra.css");`, /may not use @import/],
        [`${original}\n@config "./tailwind.config.js";`, /@config/],
        [`${original}\n@plugin "./plug.js";`, /@plugin/],
        [`${original}\n@reference "./extra.css";`, /@reference/],
        [`${original}\n@SOURCE "../../../../src/components";`, /@source/],
      ]) {
        writeFileSync(join(REACT_FIXTURE, "src", "style.css"), css);
        await expect(buildMod(REACT_FIXTURE)).rejects.toThrow(message);
      }
      // The reported hole: a nested stylesheet carrying an `@source` onto a folder outside the scan
      // (decoy/ holds a class no source of the mod writes). It is refused, and its class is not built.
      writeFileSync(join(REACT_FIXTURE, "src", "extra.css"), '@source "../decoy";\n');
      writeFileSync(join(REACT_FIXTURE, "src", "style.css"), `${original}\n@import "./extra.css";`);
      await expect(buildMod(REACT_FIXTURE)).rejects.toThrow(/may not use @import/);
      rmSync(join(REACT_FIXTURE, "src", "extra.css"), { force: true });
      // A `/*` inside a string is not a comment: a plain comment-stripping regex would hide the
      // directive between two such strings, and Tailwind would follow it.
      writeFileSync(join(REACT_FIXTURE, "src", "extra.css"), '@source "../decoy";\n');
      for (const [css, message] of [
        [
          `${original}\na{content:"/*"} @import "./extra.css"; b{content:"*/"}`,
          /may not use @import/,
        ],
        [`${original}\na{content:'/*'} @source "../decoy"; b{content:'*/'}`, /@source/],
        [`${original}\na{content:"\\"/*"} @source "../decoy"; b{content:"*/"}`, /@source/],
        // An escaped slash outside a string opens no comment either.
        [`${original}\na\\/*{} @source "../decoy"; b\\*/{}`, /@source/],
        [`${original}\na\\/*{} @import "./extra.css"; b\\*/{}`, /may not use @import/],
      ]) {
        writeFileSync(join(REACT_FIXTURE, "src", "style.css"), css);
        await expect(buildMod(REACT_FIXTURE)).rejects.toThrow(message);
      }
      rmSync(join(REACT_FIXTURE, "src", "extra.css"), { force: true });
      // A comment that only talks about the directives is not a use of them.
      writeFileSync(
        join(REACT_FIXTURE, "src", "style.css"),
        `/* @source is added by the build; @import "tailwindcss/utilities" is refused */\n${original}`,
      );
      await buildMod(REACT_FIXTURE);
    } finally {
      rmSync(join(REACT_FIXTURE, "src", "extra.css"), { force: true });
      writeFileSync(join(REACT_FIXTURE, "src", "style.css"), original);
      await buildMod(REACT_FIXTURE);
    }
  }, 60_000);
});

describe("what the build config does for a mod", () => {
  it("rewrites Vite's module script and drops its crossorigin, whatever else is in the page", async () => {
    const dir = scratchMod({
      "index.html": PAGE,
      "main.ts": 'import "./style.css"; document.title = "x";',
      "style.css": "body{margin:0}",
    });
    await buildMod(dir);
    const html = read(dir, "index.html");
    expect(html).toContain("<script defer src=");
    expect(html).not.toMatch(/crossorigin|type="module"/);
  });

  it("fails the build when a module script or crossorigin survives the rewrite", async () => {
    const dir = scratchMod({
      // Vite leaves a link to another origin alone, and `crossorigin="…"` is not the bare attribute
      // the rewrite drops: it must fail the build, not ship.
      "index.html": PAGE.replace(
        "</head>",
        '<link rel="preconnect" href="https://example.com" crossorigin="use-credentials"></head>',
      ),
      "main.ts": "document.title = 'x';",
      "style.css": "body{margin:0}",
    });
    await expect(buildMod(dir)).rejects.toThrow(/crossorigin/);
  });

  it("fails the build on a module script the rewrite does not recognise", async () => {
    // An external module script is left alone by Vite, so it has no `crossorigin` for the rewrite to
    // find: it must still fail the build, not reach the frame as a script the frame cannot run.
    const dir = scratchMod({
      "index.html": PAGE.replace(
        "</body>",
        '<script type="module" src="https://example.com/x.js"></script></body>',
      ),
      "main.ts": "document.title = 'x';",
    });
    await expect(buildMod(dir)).rejects.toThrow(/module script/);
  });

  it("never inlines an asset as a data: URL (the frame's CSP blocks one)", async () => {
    const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"></svg>';
    const dir = scratchMod({
      "index.html": PAGE,
      "main.ts": 'import "./style.css";',
      "style.css": "body{background:url(./dot.svg)}",
      "dot.svg": svg,
    });
    await buildMod(dir);
    const css = read(dir, "assets/style.css");
    expect(css).not.toContain("data:");
    // Next to the stylesheet (assets/), so the relative URL is the file's own name.
    expect(css).toContain("url(./dot.svg)");
    expect(readFileSync(join(dir, "dist", "app", "assets", "dot.svg"), "utf8")).toBe(svg);
  });

  it("copies nothing from a public/ folder and reads no .env file", async () => {
    const dir = scratchMod({
      "index.html": PAGE,
      "main.ts": "document.title = String(import.meta.env.VITE_LEAK);",
      "public/leak.txt": "should not ship",
      ".env": "VITE_LEAK=THE-LEAKED-VALUE\n",
    });
    await buildMod(dir);
    expect(listFiles(join(dir, "dist", "app"))).not.toContain("leak.txt");
    expect(read(dir, "assets/app.js")).not.toContain("THE-LEAKED-VALUE");
  });

  it("drops a bare crossorigin on a link to another origin, as it does on its own tags", async () => {
    const dir = scratchMod({
      "index.html": PAGE.replace(
        "</head>",
        '<link rel="preconnect" href="https://example.com" crossorigin></head>',
      ),
      "main.ts": "document.title = 'x';",
    });
    await buildMod(dir);
    expect(read(dir, "index.html")).not.toMatch(/crossorigin/);
  });

  it("keeps a worker that imports dynamically in ONE string literal (the build inlines it)", async () => {
    const dir = scratchMod({
      "index.html": PAGE,
      "main.ts": 'import w from "inline-worker:./w.ts"; document.title = w;',
      "w.ts": 'void import("./other.ts").then((m) => console.log(m.x));',
      "other.ts": 'export const x = "DYNAMIC-MODULE-MARKER";',
    });
    await buildMod(dir);
    expect(listFiles(join(dir, "dist", "app"))).toEqual([join("assets", "app.js"), "index.html"]);
    expect(read(dir, "assets/app.js")).toContain("DYNAMIC-MODULE-MARKER");
  });

  it("refuses a worker build that yields anything but exactly one chunk", () => {
    const chunk = { type: "chunk", code: "x", moduleIds: [] };
    expect(onlyChunk({ output: [chunk] })).toBe(chunk);
    expect(onlyChunk([{ output: [chunk] }])).toBe(chunk);
    expect(() => onlyChunk({ output: [chunk, chunk] })).toThrow(/exactly one chunk/);
    expect(() => onlyChunk({ output: [{ type: "asset" }] })).toThrow(/exactly one chunk/);
    expect(() => onlyChunk({ output: [] })).toThrow(/exactly one chunk/);
  });

  it("takes the folder from PM_MOD_DIR, and says so when it is missing", async () => {
    const previous = process.env.PM_MOD_DIR;
    delete process.env.PM_MOD_DIR;
    try {
      const mod = await import("../mods/vite.config.ts");
      expect(() => mod.default({ command: "build", mode: "production" })).toThrow(/PM_MOD_DIR/);
      process.env.PM_MOD_DIR = FIXTURE;
      const config = mod.default({ command: "build", mode: "production" });
      expect(config.root).toBe(join(FIXTURE, "src"));
      expect(config.build.outDir).toBe(join(FIXTURE, "dist", "app"));
    } finally {
      if (previous === undefined) delete process.env.PM_MOD_DIR;
      else process.env.PM_MOD_DIR = previous;
    }
  });
});

describe("packageModulePath", () => {
  it.each([
    [
      join(repoRoot, "node_modules/d3-force/dist/d3-force.mjs"),
      "node_modules/d3-force/dist/d3-force.mjs",
    ],
    [`\0${join(repoRoot, "node_modules/x/index.js")}?commonjs-module`, "node_modules/x/index.js"],
    [join(repoRoot, "node_modules/a/node_modules/b/i.js"), "node_modules/a/node_modules/b/i.js"],
    [join(repoRoot, "mods/atlas.map/src/main.ts"), null],
    ["\0inline-worker:/anywhere/w.ts", null],
    [join(repoRoot, "mods/atlas.map/src/index.html"), null],
  ])("%s", (id, expected) => {
    expect(packageModulePath(id)).toBe(expected);
  });
});
