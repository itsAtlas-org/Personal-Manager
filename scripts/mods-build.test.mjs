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
// It builds tests/mod-fixtures/acme.hello (the third-party fixture) and the real mods/atlas.map,
// in place: both write only into their git-ignored dist/ and .build/.

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
    expect(result.files.get("NOTICE.txt").toString()).toContain(
      "This mod bundles no npm packages.",
    );
  });

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
