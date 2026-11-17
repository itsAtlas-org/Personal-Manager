// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// scripts/check-mods.mjs, and D12 in scripts/check-version-lockstep.mjs.
//
// Every rule is shown a tree that breaks it and a tree that is merely close to breaking it, because a
// gate fails in the passing direction: a regex that stops matching prints the same tick. The static
// rules run in-process over a throwaway tree built from the repo's own committed manifests; the diff
// rules and D12 run the real scripts over throwaway git repos (committed and uncommitted work alike,
// since pre-commit sees the second and CI the first).
//
// THREAT MODEL. These gates catch a first-party author's mistake before merge (a mod folder that
// ships in the installer, a PM file that reaches into a mod, a mod change that skips its own bump).
// They are not the wall against a hostile mod: that is the sealed frame and the verifier.

import { execFileSync, spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it } from "vitest";

import {
  PLACEHOLDER_COMMIT,
  bumpSummary,
  bundleSources,
  changedPaths,
  changelogTop,
  checkModBumps,
  checkMods,
  isModsOnly,
  resolvesIntoMods,
  resourceReaches,
  rustConstants,
  specifiersIn,
} from "./check-mods.mjs";
import { PERMISSIONS } from "./pack-mod.mjs";
import { semverCmp } from "./semver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const realRoot = join(here, "..");
const readReal = (rel) => readFileSync(join(realRoot, rel), "utf8");

const dirs = [];
afterEach(() => {
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true });
});
const tmp = (prefix) => {
  const d = mkdtempSync(join(tmpdir(), prefix));
  dirs.push(d);
  return d;
};

function put(root, path, text) {
  mkdirSync(dirname(join(root, path)), { recursive: true });
  writeFileSync(join(root, path), text);
}

// ---- A tree that passes ---------------------------------------------------------------------------

const MAP_MANIFEST = JSON.parse(readReal("mods/atlas.map/manifest.json"));
const HELLO_MANIFEST = JSON.parse(readReal("tests/mod-fixtures/acme.hello/manifest.json"));
const json = (v) => `${JSON.stringify(v, null, 2)}\n`;

const packageRs = (perms = PERMISSIONS) =>
  `// the list\npub const PERMISSIONS: &[&str] = &[\n${perms.map((p) => `    "${p}",`).join("\n")}\n];\n`;
const policyRs = (api = "1, 0", oldest = 1) =>
  `pub const MOD_API_VERSION: (u16, u16) = (${api});\npub const MOD_API_OLDEST_MAJOR: u16 = ${oldest};\n`;

/** The smallest tree every static rule is happy with; `over` replaces files (null removes one). */
function goodTree(over = {}) {
  const files = {
    "src-tauri/src/mods/package.rs": packageRs(),
    "src-tauri/src/mods/policy.rs": policyRs(),
    "src-tauri/tauri.conf.json": json({
      bundle: {
        resources: {
          "../LICENCE.txt": "LICENCE.txt",
          "../sidecar/pm_sidecar.py": "sidecar/pm_sidecar.py",
        },
      },
    }),
    "src-tauri/tauri.windows.conf.json": json({ bundle: { resources: { python: "python" } } }),
    "src-tauri/tauri.linux.conf.json": json({ bundle: { resources: { python: "python" } } }),
    // PM's own folder called mods, imported the three ways it can be, is not the mods/ folder.
    "src/main.ts":
      'import { a } from "./lib/a";\nimport { F } from "./components/mods/ModFrame";\nexport {};\n',
    "src/lib/a.ts": 'import { F } from "../components/mods/ModFrame";\nexport const a = F;\n',
    "src/components/mods/ModFrame.tsx": 'import { x } from "../mods/other";\nexport const F = x;\n',
    "src/components/mods/other.ts": "export const x = 1;\n",
    justfile: "tsc:\n    npx tsc --noEmit\n    npx tsc -p mods --noEmit\n",
    "mods/tsconfig.json": '// comment\n{\n  "exclude": ["**/dist", "**/node_modules",],\n}\n',
    "mods/atlas.map/manifest.json": json(MAP_MANIFEST),
    "mods/atlas.map/CHANGELOG.md": "# Changelog\n\nIntro.\n\n## 1.0.0\n\n- The first.\n",
    "mods/atlas.map/src/index.html": "<html></html>\n",
    "mods/atlas.map/src/main.ts": "export {};\n",
    "tests/mod-fixtures/acme.hello/manifest.json": json(HELLO_MANIFEST),
    "tests/mod-fixtures/acme.hello/src/index.html": "<html></html>\n",
  };
  for (const [k, v] of Object.entries(over)) {
    if (v === null) delete files[k];
    else files[k] = v;
  }
  return files;
}

function build(files) {
  const root = tmp("pm-check-mods-");
  for (const [path, text] of Object.entries(files)) if (text !== null) put(root, path, text);
  return root;
}

const problemsOf = (files) => checkMods(build(files)).problems;

/** The tree with the Map's manifest edited by `edit(manifest)`. */
const withMap = (edit, over = {}) => {
  const m = structuredClone(MAP_MANIFEST);
  edit(m);
  return goodTree({ "mods/atlas.map/manifest.json": json(m), ...over });
};

function expectRefused(files, part) {
  const problems = problemsOf(files);
  expect(
    problems.some((p) => p.includes(part)),
    `expected a problem mentioning ${JSON.stringify(part)}, got:\n${problems.join("\n") || "(none)"}`,
  ).toBe(true);
}

describe("the wiring", () => {
  it("runs in check-fast, in pr.yml's hygiene job (which has no build) and in pre-commit", () => {
    const fast = /^check-fast:(.*)$/m.exec(readReal("justfile"))[1].trim().split(/\s+/);
    expect(fast).toContain("mods");
    expect(readReal("justfile")).toMatch(
      /^mods \*args:\n {4}node scripts\/check-mods\.mjs \{\{args\}\}$/m,
    );
    const pr = readReal(".github/workflows/pr.yml");
    const hygiene = pr.slice(pr.indexOf("\n  hygiene:"));
    expect(hygiene).toContain("run: just mods\n");
    expect(readReal(".pre-commit-config.yaml")).toContain("entry: just mods\n");
  });

  it("leaves the base-aware half to the version job, which has the history", () => {
    const pr = readReal(".github/workflows/pr.yml");
    const version = pr.slice(pr.indexOf("\n  version:"), pr.indexOf("\n  hygiene:"));
    expect(version).toContain("fetch-depth: 0");
    expect(version).toContain('run: just version --base "$BASE_SHA"');
  });
});

describe("the baseline tree", () => {
  it("passes, and says what it inspected", () => {
    const r = checkMods(build(goodTree()));
    expect(r.problems).toEqual([]);
    expect(r).toMatchObject({ mods: 1, fixtures: 1, configs: 3 });
    expect(r.scanned).toBeGreaterThan(3);
  });

  it("is what the real repo passes too", () => {
    const r = checkMods(realRoot);
    expect(r.problems).toEqual([]);
    expect(r.mods).toBeGreaterThanOrEqual(1);
    expect(r.fixtures).toBeGreaterThanOrEqual(1);
    expect(r.configs).toBe(3);
    expect(r.scanned).toBeGreaterThan(100);
  });
});

// ---- The manifest ---------------------------------------------------------------------------------

describe("the manifest", () => {
  it("refuses an unknown field, at any depth", () => {
    expectRefused(
      withMap((m) => (m.surprise = 1)),
      "unknown field",
    );
    expectRefused(
      withMap((m) => (m.publisher.extra = "x")),
      "unknown field",
    );
    expectRefused(
      withMap((m) => (m.entry.extra = "x")),
      "unknown field",
    );
  });

  it("refuses a bad id, and an id that is not the folder", () => {
    expectRefused(
      withMap((m) => (m.id = "atlas.Map")),
      "id",
    );
    expectRefused(
      withMap((m) => (m.id = "atlas")),
      "id",
    );
    expectRefused(
      withMap((m) => (m.id = "atlas.other")),
      "the id is atlas.other but the folder is atlas.map",
    );
  });

  it("makes a mod under mods/ an atlas id with tier first, and nothing else", () => {
    const tier = (t, id = "atlas.map") => withMap((m) => ((m.publisher.tier = t), (m.id = id)));
    expectRefused(tier("second"), "tier");
    expectRefused(tier("third"), "tier");
    // the folder moves with the id, so only the atlas rule can fire
    const tree = goodTree();
    for (const k of Object.keys(tree)) if (k.startsWith("mods/atlas.map/")) delete tree[k];
    const root = build({
      ...tree,
      "mods/acme.map/manifest.json": json({
        ...MAP_MANIFEST,
        id: "acme.map",
        publisher: { ...MAP_MANIFEST.publisher, tier: "third" },
      }),
      "mods/acme.map/CHANGELOG.md": "## 1.0.0\n\n- x\n",
      "mods/acme.map/src/index.html": "x",
    });
    const problems = checkMods(root).problems;
    expect(problems.some((p) => p.includes('its id starts "atlas."'))).toBe(true);
  });

  // PR B plan Q3 (recommended answer): a later package with another author or sourceRepo is refused
  // as an update by PM, which only uninstall-and-reinstall fixes, so the edit stops here.
  it("pins a first-party mod's author and sourceRepo", () => {
    expect(problemsOf(goodTree())).toEqual([]);
    expectRefused(
      withMap((m) => (m.publisher.author = "Someone")),
      'publisher.author is "Someone" but a first-party mod\'s is "Atlas"',
    );
    expectRefused(
      withMap((m) => (m.publisher.author = "atlas")),
      "publisher.author",
    );
    expectRefused(
      withMap((m) => (m.publisher.sourceRepo = "itsAtlas-org/PM-Mods")),
      'publisher.sourceRepo is "itsAtlas-org/PM-Mods"',
    );
    expectRefused(
      withMap((m) => (m.publisher.sourceRepo = "itsatlas-org/Personal-Manager")),
      "publisher.sourceRepo",
    );
  });

  it("does not hold a test fixture to the first-party publisher", () => {
    expect(HELLO_MANIFEST.publisher.author).not.toBe("Atlas");
    expect(problemsOf(goodTree())).toEqual([]);
  });

  it("refuses a first-tier mod whose id is not an atlas one", () => {
    const tree = goodTree();
    for (const k of Object.keys(tree)) if (k.startsWith("mods/atlas.map/")) delete tree[k];
    const root = build({
      ...tree,
      "mods/acme.map/manifest.json": json({ ...MAP_MANIFEST, id: "acme.map" }),
      "mods/acme.map/CHANGELOG.md": "## 1.0.0\n\n- x\n",
      "mods/acme.map/src/index.html": "x",
    });
    expect(checkMods(root).problems.some((p) => p.includes('its id starts "atlas."'))).toBe(true);
  });

  it("refuses a licence other than AGPL-3.0-or-later", () => {
    expectRefused(
      withMap((m) => (m.licence = "MIT")),
      "licence",
    );
  });

  it("refuses a permission outside package.rs's list, read from the Rust source", () => {
    expectRefused(
      withMap((m) => (m.permissions[0].id = "teleport")),
      "permission",
    );
    // pack-mod knows `links`; package.rs here does not: the Rust text is the truth.
    const withLinks = (m) => m.permissions.push({ id: "links", why: "Opens links." });
    const fewer = PERMISSIONS.filter((p) => p !== "links");
    expectRefused(
      withMap(withLinks, { "src-tauri/src/mods/package.rs": packageRs(fewer) }),
      "is not one of package.rs's PERMISSIONS",
    );
    // and the same drift is named on its own, since it would let pack-mod pack what PM refuses
    expectRefused(
      withMap(() => {}, { "src-tauri/src/mods/package.rs": packageRs(fewer) }),
      "pack-mod.mjs PERMISSIONS differs",
    );
    expect(
      problemsOf(withMap(withLinks, { "src-tauri/src/mods/package.rs": packageRs() })),
    ).toEqual([]);
  });

  it("holds the internet permission and internetHosts together", () => {
    expectRefused(
      withMap((m) => m.permissions.push({ id: "internet", why: "Reaches out." })),
      "internet",
    );
    expectRefused(
      withMap((m) => (m.internetHosts = ["example.com"])),
      "internet",
    );
    expect(
      problemsOf(
        withMap((m) => {
          m.permissions.push({ id: "internet", why: "Reaches out." });
          m.internetHosts = ["example.com"];
        }),
      ),
    ).toEqual([]);
  });

  it("needs its entry page to be a file of the mod's source", () => {
    expectRefused(goodTree({ "mods/atlas.map/src/index.html": null }), "entry page");
    expectRefused(
      withMap((m) => (m.entry.page = "index.html")),
      "entry page",
    );
    expectRefused(
      withMap((m) => (m.entry.page = "app/other.html")),
      "entry page",
    );
    // a nested page works when src/ has it at that place
    expect(
      problemsOf(
        withMap((m) => (m.entry.page = "app/pages/home.html"), {
          "mods/atlas.map/src/pages/home.html": "x",
        }),
      ),
    ).toEqual([]);
  });

  it("holds apiVersion to the API this PM serves", () => {
    const api = (min, target) => withMap((m) => (m.apiVersion = { min, target }));
    expectRefused(api("1.1", "1.1"), "outside what this PM serves");
    expectRefused(api("1.0", "1.1"), "outside what this PM serves");
    expectRefused(api("1.0", "2.0"), "outside what this PM serves");
    expectRefused(api("0.9", "0.9"), "outside what this PM serves");
    expectRefused(api("1.1", "1.0"), "apiVersion");
    // a PM that serves 1.2 takes 1.0 to 1.2, and still not an older major
    const served = { "src-tauri/src/mods/policy.rs": policyRs("1, 2") };
    expect(problemsOf({ ...api("1.0", "1.2"), ...served })).toEqual([]);
    expect(problemsOf({ ...api("1.0", "1.1"), ...served })).toEqual([]);
    const v2 = { "src-tauri/src/mods/policy.rs": policyRs("2, 0", 1) };
    expectRefused({ ...api("1.0", "1.0"), ...v2 }, "outside what this PM serves");
  });

  it("insists on the all-zero placeholder commit", () => {
    expectRefused(
      withMap((m) => (m.publisher.commit = "a".repeat(40))),
      "placeholder",
    );
    expectRefused(
      withMap((m) => (m.publisher.commit = "0".repeat(39))),
      "commit",
    );
    expect(PLACEHOLDER_COMMIT).toBe("0".repeat(40));
  });

  it("is read as strict JSON", () => {
    const text = json(MAP_MANIFEST);
    expectRefused(
      goodTree({
        "mods/atlas.map/manifest.json": text.replace(
          '"version": "1.0.0",',
          '"version": "1.0.0",\n  "version": "9.9.9",',
        ),
      }),
      "repeated",
    );
    expectRefused(goodTree({ "mods/atlas.map/manifest.json": `\ufeff${text}` }), "manifest.json");
    expectRefused(goodTree({ "mods/atlas.map/manifest.json": `${text}\n{}` }), "manifest.json");
    expectRefused(goodTree({ "mods/atlas.map/manifest.json": "[]" }), "must be an object");
  });

  it("refuses text that is not UTF-8, and a manifest over the cap", () => {
    const root = build(goodTree());
    writeFileSync(join(root, "mods/atlas.map/manifest.json"), Buffer.from([0x7b, 0xff, 0x7d]));
    expect(checkMods(root).problems.some((p) => p.includes("not valid UTF-8"))).toBe(true);
    expectRefused(
      goodTree({ "mods/atlas.map/manifest.json": `{"pad":"${"x".repeat(300 << 10)}"}` }),
      "too large",
    );
  });

  it("needs every folder under mods/ to be a mod", () => {
    expectRefused(goodTree({ "mods/atlas.map/manifest.json": null }), "has no manifest.json");
    expectRefused(goodTree({ "mods/stray/readme.txt": "x" }), "mods/stray: has no manifest.json");
  });

  it("finds no mod at all when mods/ is empty, and fails instead of passing", () => {
    const tree = goodTree();
    for (const k of Object.keys(tree)) if (k.startsWith("mods/atlas.map/")) delete tree[k];
    expectRefused(tree, "holds no mod folder");
  });

  it("holds a test fixture to the same manifest rules, and never to an atlas. id", () => {
    const fixture = (edit) => {
      const m = structuredClone(HELLO_MANIFEST);
      edit(m);
      return goodTree({ "tests/mod-fixtures/acme.hello/manifest.json": json(m) });
    };
    expectRefused(
      fixture((m) => (m.surprise = 1)),
      "unknown field",
    );
    expectRefused(
      fixture((m) => (m.id = "acme.other")),
      "the folder is acme.hello",
    );
    expectRefused(
      fixture((m) => (m.publisher.tier = "first")),
      "Atlas mod",
    );
    expectRefused(
      fixture((m) => (m.publisher.commit = "b".repeat(40))),
      "placeholder",
    );
    // an atlas. id under tests/ is the impersonation the rule exists for
    const root = build({
      ...goodTree({
        "tests/mod-fixtures/acme.hello/manifest.json": null,
        "tests/mod-fixtures/acme.hello/src/index.html": null,
      }),
      "tests/mod-fixtures/atlas.fake/manifest.json": json({ ...MAP_MANIFEST, id: "atlas.fake" }),
      "tests/mod-fixtures/atlas.fake/src/index.html": "x",
    });
    expect(checkMods(root).problems.some((p) => p.includes("may not take an"))).toBe(true);
  });

  it("skips a fixtures folder with no manifest, like the golden archive's", () => {
    const r = checkMods(build(goodTree({ "tests/mod-fixtures/golden/hello.pmmod.b64": "AAAA" })));
    expect(r.problems).toEqual([]);
    expect(r.fixtures).toBe(1);
  });
});

describe("rustConstants", () => {
  it("reads the three constants out of the real Rust source", () => {
    const c = rustConstants(realRoot);
    expect(c.permissions).toEqual(PERMISSIONS);
    expect(c.api).toEqual([1, 0]);
  });

  it("throws, loudly, when a constant is renamed", () => {
    const root = build(
      goodTree({ "src-tauri/src/mods/package.rs": "pub const RENAMED: &[&str] = &[];" }),
    );
    expect(() => rustConstants(root)).toThrow(/renamed or reshaped/);
    expect(checkMods(root).problems[0]).toMatch(/renamed or reshaped/);
    expect(() => rustConstants(build(goodTree({ "src-tauri/src/mods/policy.rs": "" })))).toThrow(
      /renamed/,
    );
    expect(() =>
      rustConstants(build(goodTree({ "src-tauri/src/mods/package.rs": packageRs([]) }))),
    ).toThrow(/empty/);
  });

  it("ignores a // comment inside the list", () => {
    const text = `pub const PERMISSIONS: &[&str] = &[\n  "storage", // "decoy"\n];\n`;
    expect(
      rustConstants(build(goodTree({ "src-tauri/src/mods/package.rs": text }))).permissions,
    ).toEqual(["storage"]);
  });
});

// ---- CHANGELOG ------------------------------------------------------------------------------------

describe("the CHANGELOG", () => {
  it("parses its top entry", () => {
    expect(changelogTop("# T\n\n## 1.2.3\n\n- a\n\n## 1.2.2\n\n- b\n")).toEqual({
      version: "1.2.3",
      hasBody: true,
    });
    expect(changelogTop("## 1.2.3 - 2026-11-17\r\n\r\n- a\r\n")).toEqual({
      version: "1.2.3",
      hasBody: true,
    });
    expect(changelogTop("## 1.2.3\n\n## 1.2.2\n- b\n")).toEqual({
      version: "1.2.3",
      hasBody: false,
    });
    expect(changelogTop("## \n- x")).toEqual({ version: null, hasBody: true });
    expect(changelogTop("# only a title\n")).toBeNull();
  });

  it("needs a top entry that is the manifest's version", () => {
    const cl = (text) => goodTree({ "mods/atlas.map/CHANGELOG.md": text });
    expectRefused(cl("## 1.0.1\n\n- a\n\n## 1.0.0\n\n- b\n"), 'the top entry is "1.0.1"');
    expectRefused(cl("## v1.0.0\n\n- a\n"), 'the top entry is "v1.0.0"');
    expectRefused(cl("# Changelog\n\nNo entries.\n"), "has no");
    expectRefused(cl("## 1.0.0\n\n## 0.9.0\n\n- old\n"), "says nothing");
    expectRefused(goodTree({ "mods/atlas.map/CHANGELOG.md": null }), "no CHANGELOG.md");
    expect(problemsOf(cl("## 1.0.0 (2026-11-17)\n\n- fine\n"))).toEqual([]);
    // the manifest moved on without the changelog
    expectRefused(
      withMap((m) => (m.version = "1.0.1")),
      'the top entry is "1.0.0"',
    );
  });
});

// ---- Worker folders -------------------------------------------------------------------------------

describe("a Worker folder", () => {
  const workerTsconfig =
    '{ "compilerOptions": { "lib": ["ES2020", "WebWorker"] }, "include": ["."] }';
  const importer =
    'import src from "inline-worker:./worker/layout.worker.ts";\nexport default src;\n';
  const good = (over = {}) =>
    goodTree({
      "mods/atlas.map/src/main.ts": importer,
      "mods/atlas.map/src/worker/layout.worker.ts": "export {};\n",
      "mods/atlas.map/src/worker/tsconfig.json": workerTsconfig,
      "mods/tsconfig.json": '{ "exclude": ["**/dist", "atlas.map/src/worker"] }',
      justfile:
        "tsc:\n    npx tsc -p mods --noEmit\n    npx tsc -p mods/atlas.map/src/worker --noEmit\n",
      ...over,
    });

  it("is fine with its tsconfig, its exclude and its tsc line", () => {
    expect(problemsOf(good())).toEqual([]);
  });

  it("needs a tsconfig.json of its own", () => {
    expectRefused(
      good({ "mods/atlas.map/src/worker/tsconfig.json": null }),
      "no readable tsconfig.json",
    );
    expectRefused(
      good({ "mods/atlas.map/src/worker/tsconfig.json": "{ not json" }),
      "no readable tsconfig.json",
    );
  });

  it("needs lib WebWorker and no DOM", () => {
    const lib = (l) =>
      good({ "mods/atlas.map/src/worker/tsconfig.json": json({ compilerOptions: { lib: l } }) });
    expectRefused(lib(["ES2020", "DOM"]), "not WebWorker without DOM");
    expectRefused(lib(["ES2020", "WebWorker", "DOM"]), "not WebWorker without DOM");
    expectRefused(lib(["ES2020"]), "not WebWorker without DOM");
    expectRefused(
      good({ "mods/atlas.map/src/worker/tsconfig.json": "{}" }),
      "not WebWorker without DOM",
    );
    expect(problemsOf(lib(["es2020", "webworker"]))).toEqual([]);
  });

  it("is left out of the DOM project by path", () => {
    expectRefused(good({ "mods/tsconfig.json": '{ "exclude": ["**/dist"] }' }), "is not in the");
    expectRefused(
      good({ "mods/tsconfig.json": '{ "exclude": ["atlas.map/src/**"] }' }),
      "is not in the",
    );
    expectRefused(
      good({ "mods/tsconfig.json": '{ "exclude": ["atlas.map/src/worker2"] }' }),
      "is not in the",
    );
    expectRefused(good({ "mods/tsconfig.json": "{}" }), "is not in the");
    expectRefused(good({ "mods/tsconfig.json": null }), "is not in the");
    expect(
      problemsOf(
        good({ "mods/tsconfig.json": '/* c */ { "exclude": ["atlas.map/src/worker", ], }' }),
      ),
    ).toEqual([]);
  });

  it("has a `tsc -p` line in the justfile for exactly that folder", () => {
    expectRefused(
      good({ justfile: "tsc:\n    npx tsc -p mods --noEmit\n" }),
      "no `tsc -p mods/atlas.map/src/worker` line",
    );
    expectRefused(
      good({ justfile: "tsc:\n    npx tsc -p mods/atlas.map/src/worker2 --noEmit\n" }),
      "no `tsc -p",
    );
    expectRefused(
      good({ justfile: "tsc:\n    npx tsc -p mods/atlas.map/src/workers --noEmit\n" }),
      "no `tsc -p",
    );
    expectRefused(good({ justfile: null }), "no `tsc -p");
    expect(
      problemsOf(good({ justfile: "tsc:\n    npx tsc -p mods/atlas.map/src/worker\n" })),
    ).toEqual([]);
  });

  it("is a folder of its own, never src/ itself or somewhere outside it", () => {
    const rooted = good({
      "mods/atlas.map/src/main.ts": 'import w from "inline-worker:./layout.worker.ts";\n',
    });
    expectRefused(rooted, "must be a folder of its own");
    const outside = good({
      "mods/atlas.map/src/main.ts": 'import w from "inline-worker:../../other/w.ts";\n',
    });
    expectRefused(outside, "must be a folder of its own");
  });

  it("reads a tsconfig the way TypeScript does: comments, escapes and trailing commas", () => {
    // a `//` inside a string is not a comment, and an escaped quote does not end the string
    const tricky = [
      "// the worker project",
      "{",
      '  /* block */ "compilerOptions": { "lib": ["ES2020", "WebWorker"], },',
      '  "note": "he said \\"hi\\" // not a comment",',
      "}",
    ].join("\n");
    const dom = [
      "// the DOM project",
      "{",
      '  "note": "a \\" quote // and a slash",',
      '  "exclude": ["**/dist", "atlas.map/src/worker", /* last */],',
      "}",
    ].join("\n");
    expect(
      problemsOf(
        good({
          "mods/atlas.map/src/worker/tsconfig.json": tricky,
          "mods/tsconfig.json": dom,
        }),
      ),
    ).toEqual([]);
  });

  it("is found from any source file, however it is spelled", () => {
    const nested = good({
      "mods/atlas.map/src/main.ts": "export {};\n",
      "mods/atlas.map/src/ui/panel.ts":
        "import w from 'inline-worker:../worker/layout.worker.ts';\n",
    });
    expect(problemsOf(nested)).toEqual([]);
    expectRefused(
      { ...nested, "mods/atlas.map/src/worker/tsconfig.json": null },
      "no readable tsconfig.json",
    );
  });
});

// ---- Symbolic links -------------------------------------------------------------------------------

describe("symbolic links", () => {
  it("are refused anywhere in a mod folder", () => {
    const root = build(goodTree());
    symlinkSync("../../../src/main.ts", join(root, "mods/atlas.map/src/borrowed.ts"));
    expect(checkMods(root).problems.some((p) => p.includes("borrowed.ts is a symbolic link"))).toBe(
      true,
    );
  });

  it("are refused as a mod folder, and in a fixture", () => {
    const root = build(goodTree());
    symlinkSync("../src", join(root, "mods/atlas.pinboard"));
    expect(
      checkMods(root).problems.some((p) => p.includes("mods/atlas.pinboard: is a symbolic link")),
    ).toBe(true);
    const f = build(goodTree());
    symlinkSync("../../../src/main.ts", join(f, "tests/mod-fixtures/acme.hello/src/x.ts"));
    expect(checkMods(f).problems.some((p) => p.includes("x.ts is a symbolic link"))).toBe(true);
  });

  it("are not looked for in built output, which this gate never reads", () => {
    const root = build(goodTree());
    mkdirSync(join(root, "mods/atlas.map/dist/app"), { recursive: true });
    symlinkSync("/etc/hostname", join(root, "mods/atlas.map/dist/app/x"));
    mkdirSync(join(root, "mods/atlas.map/.build"), { recursive: true });
    symlinkSync("/etc/hostname", join(root, "mods/atlas.map/.build/y"));
    expect(checkMods(root).problems).toEqual([]);
  });
});

// ---- The tauri configs ----------------------------------------------------------------------------

describe("bundle.resources", () => {
  const conf = (resources, extra = {}) =>
    goodTree({ "src-tauri/tauri.conf.json": json({ bundle: { resources }, ...extra }) });

  it("refuses a source under mods/, in the map form and the list form", () => {
    for (const source of [
      "../mods/atlas.map/dist",
      "../mods",
      "../mods/",
      "../mods/atlas.map",
      "./../mods/atlas.map/x",
      "..\\mods\\atlas.map",
      "../MODS/atlas.map",
      "../sidecar/../mods/atlas.map",
    ]) {
      expectRefused(conf({ [source]: "m" }), "under mods/");
      expectRefused(conf([source]), "under mods/");
    }
  });

  it("refuses a folder that holds the whole repository, mods/ included", () => {
    for (const source of ["..", "../", "../..", "../."]) {
      expectRefused(conf({ [source]: "all" }), "whole repository");
    }
  });

  it("refuses a glob that matches mods/, and one that starts outside the repository", () => {
    for (const glob of [
      "../mods/**",
      "../mods/*",
      "../*",
      "../**",
      "../m*",
      "../mod?",
      "../**/manifest.json",
      "../**/dist/**",
      "../**/mods",
      "../{mods,sidecar}",
      "../[m]ods/**",
      "../*/atlas.map/manifest.json",
      "../*/*/dist/app/index.html",
    ]) {
      expectRefused(conf({ [glob]: "g" }), "glob");
    }
    expectRefused(conf({ "../../*": "g" }), "starts outside the repository");
  });

  it("refuses an absolute path through mods/", () => {
    expectRefused(conf({ "/srv/build/mods/atlas.map": "m" }), "absolute path through mods/");
    expectRefused(conf({ "C:\\build\\mods\\atlas.map": "m" }), "absolute path through mods/");
  });

  // Tauri copies repo files into an installer through more keys than `resources`; each is read.
  describe("the other keys that copy files into the installer", () => {
    const withBundle = (bundle) =>
      goodTree({ "src-tauri/tauri.conf.json": json({ bundle: { resources: {}, ...bundle } }) });

    it.each([
      [
        "bundle.linux.deb.files",
        { linux: { deb: { files: { "/usr/share/pm/mods": "../mods" } } } },
      ],
      [
        "bundle.linux.rpm.files",
        { linux: { rpm: { files: { "/usr/share/pm/m": "../mods/atlas.map" } } } },
      ],
      [
        "bundle.linux.appimage.files",
        { linux: { appimage: { files: { "usr/share/m": "../mods/atlas.map/dist" } } } },
      ],
      ["bundle.macOS.files", { macOS: { files: { "Resources/mods": "../mods" } } }],
      ["bundle.macos.files", { macos: { files: { "Resources/mods": "../mods/atlas.map" } } }],
      ["bundle.externalBin", { externalBin: ["../mods/atlas.map/bin/tool"] }],
    ])("refuses a mod source under %s", (key, bundle) => {
      expectRefused(withBundle(bundle), `${key} source`);
      expectRefused(withBundle(bundle), "I-25");
    });

    it("refuses a glob, a whole-repository folder and an absolute path there too", () => {
      expectRefused(
        withBundle({ linux: { deb: { files: { "/usr/share/m": "../mods/**" } } } }),
        "glob",
      );
      expectRefused(
        withBundle({ macOS: { files: { "Resources/all": ".." } } }),
        "whole repository",
      );
      expectRefused(
        withBundle({ linux: { rpm: { files: { "/m": "/srv/build/mods/atlas.map" } } } }),
        "absolute path through mods/",
      );
    });

    it("sees one that only a platform file adds, in that platform's merged config", () => {
      const problems = problemsOf(
        goodTree({
          "src-tauri/tauri.linux.conf.json": json({
            bundle: {
              resources: { python: "python" },
              linux: { deb: { files: { "/m": "../mods" } } },
            },
          }),
        }),
      );
      expect(problems).toHaveLength(1);
      expect(problems[0]).toContain("tauri.linux.conf.json: bundle.linux.deb.files source");
    });

    it("lets unrelated sources through, and ignores a value that is not a path", () => {
      expect(
        problemsOf(
          withBundle({
            linux: {
              deb: { files: { "/usr/share/pm/licence": "../LICENCE.txt", "/x": "../modsfoo/x" } },
              rpm: { files: null },
              appimage: {},
            },
            macOS: { files: { "Resources/n": "../NOTICE.txt" }, minimumSystemVersion: "14.0" },
            externalBin: ["binaries/tool"],
          }),
        ),
      ).toEqual([]);
      expect(bundleSources({ linux: { deb: { files: { "/x": 3, "/y": ["../mods"] } } } })).toEqual(
        [],
      );
      expect(bundleSources(undefined)).toEqual([]);
      expect(bundleSources({ externalBin: "../mods" })).toEqual([]);
    });
  });

  it("lets an unrelated source through, including look-alikes", () => {
    expect(
      problemsOf(
        conf({
          "../LICENCE.txt": "LICENCE.txt",
          "../sidecar/**": "sidecar",
          "../src/components/mods/x.txt": "x",
          "../modsfoo/x": "x",
          "../mods-extra/x": "x",
          "../xmods": "x",
          "../sidecar/mods/x": "x",
          python: "python",
          "/opt/other/python": "py",
        }),
      ),
    ).toEqual([]);
  });

  it("merges a platform file over the base the way Tauri does (RFC 7386)", () => {
    const base = { "../LICENCE.txt": "LICENCE.txt" };
    // a platform file adding a bad source is caught in the merged config of that platform
    const added = goodTree({
      "src-tauri/tauri.conf.json": json({ bundle: { resources: base } }),
      "src-tauri/tauri.linux.conf.json": json({
        bundle: { resources: { "../mods/atlas.map": "m" } },
      }),
    });
    const problems = problemsOf(added);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("tauri.linux.conf.json");
    // a list in the platform file REPLACES the base's map, so the base's source is gone there
    const replaced = goodTree({
      "src-tauri/tauri.conf.json": json({ bundle: { resources: { "../mods/atlas.map": "m" } } }),
      "src-tauri/tauri.windows.conf.json": json({ bundle: { resources: ["python"] } }),
      "src-tauri/tauri.linux.conf.json": json({
        bundle: { resources: { "../mods/atlas.map": null } },
      }),
    });
    const merged = problemsOf(replaced);
    // the base config itself still names it, and that is the one that is flagged: the base alone is a
    // bundle (macOS has no platform file), so a platform file cannot launder it
    expect(merged).toHaveLength(1);
    expect(merged[0]).toContain("tauri.conf.json:");
  });

  it("sees a source whose platform file turns resources into the offending value", () => {
    const tree = goodTree({
      "src-tauri/tauri.windows.conf.json": json({ bundle: { resources: ["../mods/x"] } }),
    });
    const problems = problemsOf(tree);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("tauri.windows.conf.json");
    // a `null` deletes the whole key, which leaves nothing to object to
    expect(
      problemsOf(
        goodTree({ "src-tauri/tauri.windows.conf.json": json({ bundle: { resources: null } }) }),
      ),
    ).toEqual([]);
  });

  it("checks every tauri config it finds, not a fixed three", () => {
    const tree = goodTree({
      "src-tauri/tauri.macos.conf.json": json({
        bundle: { resources: { "../mods/atlas.map": "m" } },
      }),
    });
    const r = checkMods(build(tree));
    expect(r.configs).toBe(4);
    expect(r.problems.some((p) => p.includes("tauri.macos.conf.json"))).toBe(true);
  });

  it("fails when there is no base config to read, instead of passing on nothing", () => {
    expectRefused(goodTree({ "src-tauri/tauri.conf.json": null }), "tauri.conf.json is missing");
  });

  it("does not mind a config with no bundle at all", () => {
    expect(problemsOf(goodTree({ "src-tauri/tauri.conf.json": "{}" }))).toEqual([]);
  });

  it("is judged by resourceReaches on single sources too", () => {
    expect(resourceReaches("../mods", "/r", [])).toBe("is under mods/");
    expect(resourceReaches("../sidecar", "/r", [])).toBeNull();
    expect(resourceReaches("/r/mods/x", "/r", [])).toBe("is under mods/");
    expect(resourceReaches("/r/sidecar/x", "/r", [])).toBeNull();
    expect(resourceReaches("../*/atlas.map/manifest.json", "/r", ["atlas.map"])).toMatch(/glob/);
  });
});

// ---- src/ never imports mods/ ---------------------------------------------------------------------

describe("src/ importing mods/", () => {
  const src = (path, text) => goodTree({ [path]: text });
  const flagged = (path, text) => expectRefused(src(path, text), "top-level mods/ folder");

  it("is refused in every spelling of an import", () => {
    flagged("src/main.ts", 'import { id } from "../mods/atlas.map/src/id";\n');
    flagged("src/lib/deep/x.ts", 'import { id } from "../../../mods/atlas.map/src/id";\n');
    flagged("src/main.ts", "import { id } from '../mods/atlas.map/src/id';\n");
    flagged("src/main.ts", 'export * from "../mods/atlas.map/src/id";\n');
    flagged("src/main.ts", 'export { id } from "../mods/atlas.map/src/id";\n');
    flagged("src/main.ts", 'import "../mods/atlas.map/src/style.css";\n');
    flagged("src/main.ts", 'const m = await import("../mods/atlas.map/src/id");\n');
    flagged("src/main.ts", "const m = await import(`../mods/${name}/src/id`);\n");
    flagged("src/main.ts", 'const m = require("../mods/atlas.map/src/id");\n');
    flagged("src/main.ts", 'import {\n  id,\n  other,\n} from "../mods/atlas.map/src/id";\n');
    flagged("src/main.ts", 'import type { T } from "../mods/atlas.map/src/id";\n');
    flagged("src/main.ts", 'import raw from "../mods/atlas.map/manifest.json?raw";\n');
    flagged("src/main.ts", 'import url from "../mods/atlas.map/src/w.ts?worker&url";\n');
    flagged(
      "src/main.ts",
      'const u = new URL("../mods/atlas.map/manifest.json", import.meta.url);\n',
    );
    flagged("src/main.ts", 'const all = import.meta.glob("../mods/**/*.ts");\n');
    flagged("src/main.ts", 'const all = import.meta.globEager("../mods/*/manifest.json");\n');
    flagged(
      "src/main.ts",
      'const all = import.meta.glob(["./lib/*.ts", "../mods/**"], { eager: true });\n',
    );
    flagged("src/main.ts", '/// <reference path="../mods/atlas.map/src/types.d.ts" />\n');
  });

  it("is refused in CSS, and by a root-relative, bare, absolute or oddly cased path", () => {
    flagged("src/index.css", '@import "../mods/atlas.map/src/style.css";\n');
    flagged("src/index.css", "@import url(../mods/atlas.map/src/style.css);\n");
    flagged("src/index.css", '@import url("../mods/atlas.map/src/style.css");\n');
    flagged("src/index.css", ".a { background: url(../mods/atlas.map/x.png); }\n");
    flagged("src/main.ts", 'import { id } from "/mods/atlas.map/src/id";\n');
    flagged("src/main.ts", 'import { id } from "mods/atlas.map/src/id";\n');
    flagged("src/main.ts", 'import { id } from "../Mods/atlas.map/src/id";\n');
    flagged("src/main.ts", 'import { id } from "..\\\\mods\\\\atlas.map\\\\src\\\\id";\n');
    flagged("src/main.ts", 'import { id } from "../mods";\n');
    flagged("src/main.ts", 'import { id } from "../mods?url";\n');
    flagged("src/main.ts", 'import { id } from "../mods#frag";\n');
    flagged("src/main.ts", 'import { id } from "../mods/./atlas.map/../atlas.map/src/id";\n');
    flagged("src/main.ts", 'import { id } from "./lib/../../mods/atlas.map/src/id";\n');
  });

  it("is refused through an absolute path into this checkout", () => {
    const root = build(goodTree());
    writeFileSync(
      join(root, "src/main.ts"),
      `import { id } from "${root}/mods/atlas.map/src/id";\n`,
    );
    expect(checkMods(root).problems.some((p) => p.includes("top-level mods/ folder"))).toBe(true);
  });

  it("is refused in a test file, a .tsx file and a plain script, all of src/", () => {
    flagged("src/lib/x.test.ts", 'import { id } from "../../mods/atlas.map/src/id";\n');
    flagged("src/components/X.tsx", 'import { id } from "../../mods/atlas.map/src/id";\n');
    flagged("src/lib/x.mjs", 'import { id } from "../../mods/atlas.map/src/id";\n');
    flagged("src/lib/x.cjs", 'const m = require("../../mods/atlas.map/src/id");\n');
  });

  it("is refused in the root index.html and in a page under src/, by attribute or inline script", () => {
    const page = (body) => `<!doctype html>\n<html><body>${body}</body></html>\n`;
    flagged(
      "index.html",
      page('<script type="module" src="/mods/atlas.map/src/main.ts"></script>'),
    );
    flagged(
      "index.html",
      page('<script type="module" src="./mods/atlas.map/src/main.ts"></script>'),
    );
    flagged("index.html", page('<link rel="stylesheet" href="mods/atlas.map/src/style.css">'));
    flagged(
      "index.html",
      page('<script type="module">import "./mods/atlas.map/src/main";</script>'),
    );
    flagged("src/page.html", page('<script src="../mods/atlas.map/src/main.js"></script>'));
    // PM's own entry, and a page that only mentions the folder in prose, are fine
    for (const body of [
      '<script type="module" src="/src/main.tsx"></script>',
      '<link rel="stylesheet" href="/src/components/mods/a.css">',
      "<p>see the mods/ folder</p>",
    ]) {
      expect(problemsOf(src("index.html", page(body))), body).toEqual([]);
    }
  });

  it("is refused even inside a comment, because the gate fails closed", () => {
    flagged("src/main.ts", '// import { id } from "../mods/atlas.map/src/id";\n');
  });

  it("is not refused for PM's own src/components/mods/, or for look-alikes", () => {
    for (const [path, text] of [
      ["src/components/Foo.tsx", 'import { F } from "./mods/ModFrame";\n'],
      ["src/components/Foo.tsx", 'import { F } from "../components/mods/ModFrame";\n'],
      ["src/lib/x.ts", 'import { F } from "../components/mods/ModFrame";\n'],
      ["src/main.ts", 'import { F } from "./mods/x";\n'],
      ["src/lib/x.ts", 'import { F } from "../modsfoo/x";\n'],
      ["src/lib/x.ts", 'import { F } from "../xmods/x";\n'],
      ["src/lib/x.ts", 'import { F } from "../../src/mods/x";\n'],
      ["src/lib/x.ts", 'import { F } from "../../src-tauri/mods/x";\n'],
      ["src/lib/x.ts", 'import { F } from "some-package/mods/x";\n'],
      ["src/lib/x.ts", 'import { F } from "@scope/mods";\n'],
      ["src/lib/x.ts", 'const s = "see the mods/ folder";\n'],
      ["src/lib/x.ts", 'const all = import.meta.glob("../components/mods/*.tsx");\n'],
      ["src/index.css", '@import "./mods/x.css";\n'],
    ]) {
      expect(problemsOf(src(path, text)), `${path}: ${text}`).toEqual([]);
    }
  });

  it("does not read node_modules under src/, or files that are not source", () => {
    expect(problemsOf(src("src/node_modules/p/x.ts", 'import "../../../mods/a";\n'))).toEqual([]);
    expect(problemsOf(src("src/notes.md", 'import "../mods/a";\n'))).toEqual([]);
  });

  it("says which file and which specifier", () => {
    const problems = problemsOf(
      src("src/lib/x.ts", 'import { id } from "../../mods/atlas.map/src/id";\n'),
    );
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("src/lib/x.ts");
    expect(problems[0]).toContain("../../mods/atlas.map/src/id");
  });

  it("fails when src/ holds nothing to scan", () => {
    const tree = goodTree();
    for (const k of Object.keys(tree)) if (k.startsWith("src/")) delete tree[k];
    expectRefused(tree, "src/ held no source files");
    // the root index.html is read too, but it is not src/: it cannot stand in for it
    expectRefused({ ...tree, "index.html": "<html></html>" }, "src/ held no source files");
  });

  it("is judged by resolvesIntoMods and specifiersIn on single inputs", () => {
    expect(resolvesIntoMods("../mods/x", "src/a.ts", "/r")).toBe(true);
    expect(resolvesIntoMods("../mods/x", "src/c/a.ts", "/r")).toBe(false);
    expect(resolvesIntoMods("./x", "mods/a.ts", "/r")).toBe(true);
    expect(resolvesIntoMods("", "src/a.ts", "/r")).toBe(false);
    expect(resolvesIntoMods("!../mods/x", "src/a.ts", "/r")).toBe(true);
    expect(specifiersIn('import a from "./a";\nimport("./b");', "x.ts")).toEqual(["./a", "./b"]);
    expect(specifiersIn("a { background: url(./a.png) }", "x.css")).toEqual(["./a.png"]);
    expect(specifiersIn("const x = url(./a.png)", "x.ts")).toEqual([]);
  });
});

// ---- The gate as a command ------------------------------------------------------------------------

describe("the command", () => {
  const SCRIPTS = [
    "check-mods.mjs",
    "check-version-lockstep.mjs",
    "pack-mod.mjs",
    "semver.mjs",
    "check-npm-licenses.mjs",
    "generate-npm-notice.mjs",
  ];
  function repoWithScripts(files) {
    const root = build(files);
    mkdirSync(join(root, "scripts"));
    for (const s of SCRIPTS) copyFileSync(join(here, s), join(root, "scripts", s));
    return root;
  }
  const run = (root, ...args) => {
    const r = spawnSync(process.execPath, [join(root, "scripts", "check-mods.mjs"), ...args], {
      cwd: root,
      encoding: "utf8",
    });
    return { status: r.status, out: `${r.stdout}${r.stderr}` };
  };

  it("passes with a count and exit 0", () => {
    const r = run(repoWithScripts(goodTree()));
    expect(r.status).toBe(0);
    expect(r.out).toMatch(
      /✓ check-mods: 1 mod folder\(s\) and 1 fixture\(s\) valid, \d+ src files/,
    );
  });

  it("fails with every problem listed and exit 1", () => {
    const r = run(
      repoWithScripts(
        withMap((m) => (m.licence = "MIT"), {
          "src/main.ts": 'import "../mods/atlas.map/src/id";\n',
        }),
      ),
    );
    expect(r.status).toBe(1);
    expect(r.out).toContain("✗ check-mods");
    expect(r.out).toContain("licence");
    expect(r.out).toContain("top-level mods/ folder");
  });

  it("with --base, adds the changed-mod rule to the static ones, and counts it", () => {
    const r = gitRepo(goodTree());
    mkdirSync(join(r.root, "scripts"));
    for (const s of SCRIPTS) copyFileSync(join(here, s), join(r.root, "scripts", s));
    const clean = run(r.root, "--base", r.base);
    expect(clean.status).toBe(0);
    expect(clean.out).toContain(
      "0 changed mod folder(s) (0 new, 0 with a greater version) against",
    );
    put(r.root, "mods/atlas.map/src/main.ts", "export const changed = 1;\n");
    const stuck = run(r.root, "--base", r.base);
    expect(stuck.status).toBe(1);
    expect(stuck.out).toContain("not greater than 1.0.0");
    put(r.root, "mods/atlas.map/manifest.json", manifestAt("1.0.1"));
    put(r.root, "mods/atlas.map/CHANGELOG.md", changelogAt("1.0.1", "1.0.0"));
    const moved = run(r.root, "--base", r.base);
    expect(moved.out).toContain(
      "1 changed mod folder(s) (0 new, 1 with a greater version) against",
    );
    expect(moved.status).toBe(0);
    // a folder new at the base is reported as new, never as "with a greater version"
    put(r.root, "mods/atlas.pinboard/manifest.json", manifestAt("1.0.0", "atlas.pinboard"));
    put(r.root, "mods/atlas.pinboard/CHANGELOG.md", changelogAt("1.0.0"));
    put(r.root, "mods/atlas.pinboard/src/index.html", "x");
    const fresh = run(r.root, "--base", r.base);
    expect(fresh.out).toContain(
      "2 changed mod folder(s) (1 new, 1 with a greater version) against",
    );
    rmSync(join(r.root, "mods/atlas.pinboard"), { recursive: true });
    // the static rules still run beside it
    put(r.root, "src/main.ts", 'import "../mods/atlas.map/src/id";\n');
    expect(run(r.root, "--base", r.base).status).toBe(1);
    // and a base that is not a ref is a failure, not a pass
    const bad = run(r.root, "--base", "nope");
    expect(bad.status).toBe(1);
    expect(bad.out).toContain("could not list the changed files against nope");
  });

  it("refuses a usage error with exit 2, never a silent pass", () => {
    const root = repoWithScripts(goodTree());
    for (const args of [["--base"], ["--bogus"], ["origin/main"], ["--base", "a", "b"]]) {
      expect(run(root, ...args).status, args.join(" ")).toBe(2);
    }
  });
});

// ---- The diff -------------------------------------------------------------------------------------

const GIT_ENV = {
  ...process.env,
  GIT_CONFIG_GLOBAL: "/dev/null",
  GIT_CONFIG_SYSTEM: "/dev/null",
  GIT_AUTHOR_NAME: "t",
  GIT_AUTHOR_EMAIL: "t@example.com",
  GIT_COMMITTER_NAME: "t",
  GIT_COMMITTER_EMAIL: "t@example.com",
};

/** A git repo whose first commit holds `files`; `.git(...)` runs git in it and `.base` is that commit. */
function gitRepo(files) {
  const root = tmp("pm-check-mods-git-");
  for (const [path, text] of Object.entries(files)) put(root, path, text);
  const git = (...args) =>
    execFileSync("git", args, {
      cwd: root,
      env: GIT_ENV,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
  git("init", "--quiet", "-b", "main");
  git("add", "-A");
  git("commit", "--quiet", "-m", "base");
  return { root, git, base: git("rev-parse", "HEAD").trim() };
}

const manifestAt = (version, id = "atlas.map") => json({ ...MAP_MANIFEST, id, version });
const changelogAt = (...versions) =>
  `# Changelog\n\n${versions.map((v) => `## ${v}\n\n- ${v}\n`).join("\n")}`;

describe("changedPaths and isModsOnly", () => {
  const repo = () =>
    gitRepo({
      "mods/atlas.map/manifest.json": manifestAt("1.0.0"),
      "mods/atlas.map/src/main.ts": "export {};\n",
      "src/a.ts": "export {};\n",
      "package.json": "{}\n",
    });

  it("lists committed, staged, unstaged and untracked work, once each, sorted", () => {
    const r = repo();
    put(r.root, "mods/atlas.map/src/main.ts", "export const a = 1;\n");
    r.git("commit", "-aqm", "one");
    put(r.root, "src/a.ts", "export const a = 2;\n");
    r.git("add", "-A");
    put(r.root, "package.json", '{"a":1}\n');
    put(r.root, "scratch.txt", "x");
    const d = changedPaths(r.base, r.root);
    expect(d.ok).toBe(true);
    expect(d.paths).toEqual([
      "mods/atlas.map/src/main.ts",
      "package.json",
      "scratch.txt",
      "src/a.ts",
    ]);
  });

  it("measures from the merge-base, so what landed on the base since the fork is not this change's", () => {
    const r = repo();
    r.git("checkout", "-q", "-b", "feature");
    put(r.root, "mods/atlas.map/src/main.ts", "export const a = 1;\n");
    r.git("commit", "-aqm", "mine");
    r.git("checkout", "-q", "main");
    put(r.root, "src/a.ts", "export const a = 3;\n");
    r.git("commit", "-aqm", "theirs");
    r.git("checkout", "-q", "feature");
    const d = changedPaths("main", r.root);
    expect(d.paths).toEqual(["mods/atlas.map/src/main.ts"]);
    expect(isModsOnly(d.paths)).toBe(true);
  });

  it("lists a rename as the two paths it is", () => {
    const r = repo();
    r.git("mv", "src/a.ts", "mods/atlas.map/src/a.ts");
    r.git("add", "-A");
    const d = changedPaths(r.base, r.root);
    expect(d.paths).toEqual(["mods/atlas.map/src/a.ts", "src/a.ts"]);
    expect(isModsOnly(d.paths)).toBe(false);
  });

  it("lists a deletion", () => {
    const r = repo();
    r.git("rm", "-q", "mods/atlas.map/src/main.ts");
    expect(changedPaths(r.base, r.root).paths).toEqual(["mods/atlas.map/src/main.ts"]);
  });

  it("is not ok for a ref that is not one, one that looks like a flag, or no common history", () => {
    const r = repo();
    for (const ref of ["nope", "--output=/tmp/x", "-n", "", undefined, 3]) {
      const d = changedPaths(ref, r.root);
      expect(d.ok, String(ref)).toBe(false);
      expect(typeof d.why).toBe("string");
    }
    r.git("checkout", "-q", "--orphan", "other");
    r.git("rm", "-rqf", ".");
    put(r.root, "z.txt", "z");
    r.git("add", "-A");
    r.git("commit", "-qm", "unrelated");
    const orphan = r.git("rev-parse", "HEAD").trim();
    r.git("checkout", "-q", "main");
    expect(changedPaths(orphan, r.root).ok).toBe(false);
  });

  it("never lets a ref act as a git option", () => {
    // `git merge-base --octopus HEAD` is a valid command that prints HEAD, so without the guard this
    // flag would be taken for a base and the diff measured from HEAD itself.
    const r = repo();
    put(r.root, "src/a.ts", "export const a = 1;\n");
    expect(changedPaths("--octopus", r.root).ok).toBe(false);
  });

  it("leaves out what git ignores", () => {
    const r = gitRepo({
      ".gitignore": "*.log\ndist\n",
      "mods/atlas.map/manifest.json": manifestAt("1.0.0"),
    });
    put(r.root, "build.log", "x");
    put(r.root, "mods/atlas.map/dist/app/index.html", "x");
    expect(changedPaths(r.base, r.root).paths).toEqual([]);
  });

  it("calls a change mods-only only when it is non-empty and every path is inside a mod folder", () => {
    expect(isModsOnly(["mods/a/x", "mods/b/y", "mods/b/sub/z.ts"])).toBe(true);
    // The build tooling directly under mods/ is shared by every mod and decides each one's bytes, so
    // changing it is PM's change (no mod folder moved, so none would owe a bump).
    for (const shared of ["mods/vite.config.ts", "mods/tsconfig.json", "mods/vite-env.d.ts"]) {
      expect(isModsOnly([shared]), shared).toBe(false);
      expect(isModsOnly(["mods/a/x", shared]), shared).toBe(false);
    }
    expect(isModsOnly(["mods/a/"])).toBe(false);
    expect(isModsOnly([])).toBe(false);
    expect(isModsOnly(["mods/a/x", "src/y"])).toBe(false);
    expect(isModsOnly(["Mods/a/x"])).toBe(false);
    expect(isModsOnly(["modsx/a"])).toBe(false);
    expect(isModsOnly(["mods"])).toBe(false);
    expect(isModsOnly(["tests/mods/a"])).toBe(false);
    expect(isModsOnly(["xmods/a", "mods/b"])).toBe(false);
  });
});

describe("checkModBumps", () => {
  const files = (version = "1.0.0") => ({
    "mods/atlas.map/manifest.json": manifestAt(version),
    "mods/atlas.map/CHANGELOG.md": changelogAt(version),
    "mods/atlas.map/src/main.ts": "export {};\n",
    "mods/tsconfig.json": "{}\n",
    "src/a.ts": "export {};\n",
  });
  const check = (r, base = r.base) => checkModBumps(base, changedPaths(base, r.root), r.root);
  const touch = (r, path = "mods/atlas.map/src/main.ts") =>
    put(r.root, path, `export const t = ${Math.random()};\n`);
  const bump = (r, version, ...history) => {
    put(r.root, "mods/atlas.map/manifest.json", manifestAt(version));
    put(r.root, "mods/atlas.map/CHANGELOG.md", changelogAt(version, ...history));
  };

  it("passes a changed mod with a greater version and its entry", () => {
    const r = gitRepo(files());
    touch(r);
    bump(r, "1.0.1", "1.0.0");
    expect(check(r)).toMatchObject({ problems: [], changed: 1, bumped: 1, fresh: 0 });
    expect(bumpSummary(check(r))).toBe("1 changed mod folder(s) (0 new, 1 with a greater version)");
  });

  it("refuses a changed mod whose version did not move, or went backwards", () => {
    const r = gitRepo(files("1.2.0"));
    touch(r);
    expect(check(r).problems.join()).toContain("not greater than 1.2.0");
    bump(r, "1.1.9", "1.2.0");
    expect(check(r).problems.join()).toContain("not greater than 1.2.0");
    bump(r, "1.2.0", "1.1.0");
    expect(check(r).problems.join()).toContain("not greater than 1.2.0");
  });

  it("orders a pre-release below its release", () => {
    const up = gitRepo(files("1.0.0-beta.1"));
    touch(up);
    bump(up, "1.0.0", "1.0.0-beta.1");
    expect(check(up).problems).toEqual([]);
    const beta = gitRepo(files("1.0.0-beta.1"));
    touch(beta);
    bump(beta, "1.0.0-beta.2", "1.0.0-beta.1");
    expect(check(beta).problems).toEqual([]);
    const down = gitRepo(files("1.0.0"));
    touch(down);
    bump(down, "1.0.0-beta.1", "1.0.0");
    expect(check(down).problems.join()).toContain("not greater");
    expect(semverCmp("1.0.0-alpha", "1.0.0-alpha.1")).toBe(-1);
    expect(semverCmp("1.0.0-2", "1.0.0-10")).toBe(-1);
    expect(semverCmp("1.0.0-a", "1.0.0-1")).toBe(1);
    expect(semverCmp("2.0.0", "1.9.9")).toBe(1);
  });

  it("wants the changelog's top entry to be the new version, even when the version did move", () => {
    const r = gitRepo(files());
    touch(r);
    put(r.root, "mods/atlas.map/manifest.json", manifestAt("1.0.1"));
    expect(check(r).problems.join()).toContain('the top entry is "1.0.0"');
    put(r.root, "mods/atlas.map/CHANGELOG.md", "## 1.0.1\n\n## 1.0.0\n\n- a\n");
    expect(check(r).problems.join()).toContain("says nothing");
  });

  it("exempts a folder that is new at the base, but still wants its changelog", () => {
    const r = gitRepo(files());
    put(r.root, "mods/atlas.pinboard/manifest.json", manifestAt("1.0.0", "atlas.pinboard"));
    put(r.root, "mods/atlas.pinboard/src/main.ts", "export {};\n");
    put(r.root, "mods/atlas.pinboard/CHANGELOG.md", changelogAt("1.0.0"));
    // counted as new, not as a verified bump: nothing was compared
    expect(check(r)).toMatchObject({ problems: [], changed: 1, bumped: 0, fresh: 1 });
    expect(bumpSummary(check(r))).toBe("1 changed mod folder(s) (1 new, 0 with a greater version)");
    rmSync(join(r.root, "mods/atlas.pinboard/CHANGELOG.md"));
    expect(check(r).problems.join()).toContain("no CHANGELOG.md");
  });

  it("needs no bump for a deleted folder, and ignores files directly in mods/", () => {
    const r = gitRepo(files());
    rmSync(join(r.root, "mods/atlas.map"), { recursive: true });
    expect(check(r)).toMatchObject({ problems: [], changed: 1, bumped: 0, fresh: 0 });
    const only = gitRepo(files());
    put(only.root, "mods/tsconfig.json", '{"a":1}\n');
    expect(check(only)).toMatchObject({ problems: [], changed: 0 });
  });

  it("checks each changed folder on its own", () => {
    const r = gitRepo({
      ...files(),
      "mods/atlas.pinboard/manifest.json": manifestAt("2.0.0", "atlas.pinboard"),
      "mods/atlas.pinboard/CHANGELOG.md": changelogAt("2.0.0"),
      "mods/atlas.pinboard/src/main.ts": "export {};\n",
    });
    touch(r);
    touch(r, "mods/atlas.pinboard/src/main.ts");
    bump(r, "1.0.1", "1.0.0");
    const problems = check(r).problems;
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("mods/atlas.pinboard changed");
  });

  it("does not count work on the base branch since the fork as this change's", () => {
    const r = gitRepo(files());
    r.git("checkout", "-q", "-b", "feature");
    put(r.root, "src/a.ts", "export const mine = 1;\n");
    r.git("commit", "-aqm", "mine");
    r.git("checkout", "-q", "main");
    touch(r);
    bump(r, "1.0.1", "1.0.0");
    r.git("commit", "-aqm", "main moved the mod");
    r.git("checkout", "-q", "feature");
    expect(check(r, "main")).toMatchObject({ problems: [], changed: 0 });
  });

  it("reports a base it cannot read rather than passing", () => {
    const r = gitRepo(files());
    const problems = checkModBumps("nope", changedPaths("nope", r.root), r.root).problems;
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("could not list the changed files against nope");
  });

  it("reports a manifest it cannot parse at the base or now", () => {
    const r = gitRepo({ ...files(), "mods/atlas.map/manifest.json": "not json" });
    touch(r);
    put(r.root, "mods/atlas.map/manifest.json", manifestAt("1.0.1"));
    expect(check(r).problems.join()).toContain("not comparable");
    const now = gitRepo(files());
    touch(now);
    put(now.root, "mods/atlas.map/manifest.json", "not json");
    expect(check(now).problems.join()).toContain("manifest.json");
  });
});

// ---- D12 in the lockstep gate ---------------------------------------------------------------------

describe("D12 in check-version-lockstep", () => {
  const SCRIPTS = [
    "check-version-lockstep.mjs",
    "check-mods.mjs",
    "pack-mod.mjs",
    "semver.mjs",
    "check-npm-licenses.mjs",
    "generate-npm-notice.mjs",
  ];
  /** The seven files lockstep reads, all at `v`. */
  const pmFiles = (v) => ({
    "package.json": json({ version: v }),
    "package-lock.json": json({ version: v, packages: { "": { version: v } } }),
    "src-tauri/tauri.conf.json": json({ version: v }),
    "src-tauri/Cargo.toml": `[package]\nname = "pm"\nversion = "${v}"\n`,
    "src-tauri/Cargo.lock": `# lock\n\n[[package]]\nname = "pm"\nversion = "${v}"\n`,
    "src/lib/changelog.ts": `export const CHANGELOG = [{ version: "${v}" }];\n`,
  });
  const PM = "3.0.0-alpha";
  const base = () => ({
    ...pmFiles(PM),
    "mods/atlas.map/manifest.json": manifestAt("1.0.0"),
    "mods/atlas.map/CHANGELOG.md": changelogAt("1.0.0"),
    "mods/atlas.map/src/main.ts": "export {};\n",
    "mods/tsconfig.json": "{}\n",
    "src/a.ts": "export {};\n",
  });

  function repo() {
    const r = gitRepo(base());
    mkdirSync(join(r.root, "scripts"), { recursive: true });
    for (const s of SCRIPTS) copyFileSync(join(here, s), join(r.root, "scripts", s));
    // the scripts are tooling of the test, not part of the change under test
    r.git("add", "-A");
    r.git("commit", "-qm", "scripts");
    r.base = r.git("rev-parse", "HEAD").trim();
    return r;
  }
  const lockstep = (r, ...args) => {
    const x = spawnSync(
      process.execPath,
      [join(r.root, "scripts", "check-version-lockstep.mjs"), ...args],
      {
        cwd: r.root,
        encoding: "utf8",
        env: GIT_ENV,
      },
    );
    return { ok: x.status === 0, out: `${x.stdout}${x.stderr}` };
  };
  const writeAll = (r, files) => {
    for (const [p, t] of Object.entries(files)) put(r.root, p, t);
  };
  const touchMod = (r) =>
    put(r.root, "mods/atlas.map/src/main.ts", `export const t = ${Math.random()};\n`);
  const bumpMod = (r, v = "1.0.1") => {
    put(r.root, "mods/atlas.map/manifest.json", manifestAt(v));
    put(r.root, "mods/atlas.map/CHANGELOG.md", changelogAt(v, "1.0.0"));
  };

  // Pre-commit sees the working tree; CI sees a commit. Both must decide the same.
  for (const mode of ["uncommitted", "committed"]) {
    describe(`with the work ${mode}`, () => {
      const go = (r, ...args) => {
        if (mode === "committed") {
          r.git("add", "-A");
          r.git("commit", "-qm", "work", "--allow-empty");
        }
        return lockstep(r, ...args);
      };

      it("passes a mods-only change that bumps the mod, with PM's version unmoved", () => {
        const r = repo();
        touchMod(r);
        bumpMod(r);
        const x = go(r, "--base", r.base);
        expect(x.out).toContain("mods-only change (D12)");
        expect(x.ok).toBe(true);
      });

      it("refuses a mods-only change that does not bump the mod", () => {
        const r = repo();
        touchMod(r);
        const x = go(r, "--base", r.base);
        expect(x.ok).toBe(false);
        expect(x.out).toContain("not greater than 1.0.0");
      });

      it("refuses a mods-only change whose changelog did not follow", () => {
        const r = repo();
        touchMod(r);
        put(r.root, "mods/atlas.map/manifest.json", manifestAt("1.0.1"));
        const x = go(r, "--base", r.base);
        expect(x.ok).toBe(false);
        expect(x.out).toContain("CHANGELOG");
      });

      it("passes a mods-only change that adds a new mod folder, and says it is new", () => {
        const r = repo();
        writeAll(r, {
          "mods/atlas.pinboard/manifest.json": manifestAt("1.0.0", "atlas.pinboard"),
          "mods/atlas.pinboard/CHANGELOG.md": changelogAt("1.0.0"),
          "mods/atlas.pinboard/src/main.ts": "export {};\n",
        });
        const x = go(r, "--base", r.base);
        expect(x.ok).toBe(true);
        expect(x.out).toContain("(1 new, 0 with a greater version)");
      });

      // The build tooling directly under mods/ decides every mod's bytes, but no mod folder changed,
      // so no mod owes a bump: it must be PM's change, with PM's version moving.
      it.each(["mods/vite.config.ts", "mods/tsconfig.json", "mods/vite-env.d.ts"])(
        "treats %s as PM's change: refused without PM's bump, passed with it",
        (shared) => {
          const t = repo();
          put(t.root, shared, `// ${Math.random()}\n`);
          const x = go(t, "--base", t.base);
          expect(x.ok).toBe(false);
          expect(x.out).toContain("version was not bumped");
          expect(x.out).toContain("a shared file directly under mods/");
          writeAll(t, pmFiles("3.0.1-alpha"));
          const y = go(t, "--base", t.base);
          expect(y.ok).toBe(true);
          expect(y.out).toContain("✓ bumped");
        },
      );

      it("passes a mods-only change that deletes a mod folder", () => {
        const r = repo();
        rmSync(join(r.root, "mods/atlas.map"), { recursive: true });
        expect(go(r, "--base", r.base).ok).toBe(true);
      });

      it("still demands PM's bump when anything outside mods/ changed", () => {
        const r = repo();
        touchMod(r);
        bumpMod(r);
        put(r.root, "src/a.ts", "export const a = 1;\n");
        const x = go(r, "--base", r.base);
        expect(x.ok).toBe(false);
        expect(x.out).toContain("version was not bumped");
      });

      it("passes a mixed change that bumps PM and the mod", () => {
        const r = repo();
        touchMod(r);
        bumpMod(r);
        put(r.root, "src/a.ts", "export const a = 1;\n");
        writeAll(r, pmFiles("3.1.0-alpha"));
        const x = go(r, "--base", r.base);
        expect(x.out).toContain("✓ bumped");
        expect(x.ok).toBe(true);
      });

      it("still wants the mod's own bump in a mixed change", () => {
        const r = repo();
        touchMod(r);
        writeAll(r, pmFiles("3.1.0-alpha"));
        const x = go(r, "--base", r.base);
        expect(x.ok).toBe(false);
        expect(x.out).toContain("not greater than 1.0.0");
      });

      it("keeps the old rule for a change with no mods/ path at all", () => {
        const r = repo();
        put(r.root, "src/a.ts", "export const a = 1;\n");
        const none = go(r, "--base", r.base);
        expect(none.ok).toBe(false);
        expect(none.out).toContain("version was not bumped");
        const bumped = repo();
        put(bumped.root, "src/a.ts", "export const a = 1;\n");
        writeAll(bumped, pmFiles("3.0.1-alpha"));
        expect(go(bumped, "--base", bumped.base).ok).toBe(true);
      });

      it("keeps the old rule for an empty change: nothing touched is not mods-only", () => {
        const r = repo();
        const x = go(r, "--base", r.base);
        expect(x.ok).toBe(false);
        expect(x.out).toContain("version was not bumped");
      });

      it("counts a file moved into mods/ from outside as touching outside", () => {
        const r = repo();
        r.git("mv", "src/a.ts", "mods/atlas.map/src/a.ts");
        bumpMod(r);
        const x = go(r, "--base", r.base);
        expect(x.ok).toBe(false);
        expect(x.out).toContain("version was not bumped");
      });

      it("counts a version file touched alongside a mod as outside mods/", () => {
        const r = repo();
        touchMod(r);
        bumpMod(r);
        put(r.root, "package.json", json({ version: PM, extra: true }));
        const x = go(r, "--base", r.base);
        expect(x.ok).toBe(false);
        expect(x.out).toContain("version was not bumped");
      });
    });
  }

  it("counts an untracked scratch file outside mods/ as a touch, failing closed", () => {
    const r = repo();
    touchMod(r);
    bumpMod(r);
    put(r.root, "scratch.txt", "x");
    expect(lockstep(r, "--base", r.base).ok).toBe(false);
    rmSync(join(r.root, "scratch.txt"));
    expect(lockstep(r, "--base", r.base).ok).toBe(true);
  });

  it("does not look at mods at all without --base", () => {
    const r = repo();
    touchMod(r);
    expect(lockstep(r).ok).toBe(true);
  });

  it("fails when the base cannot be read, or shares no history", () => {
    const r = repo();
    touchMod(r);
    bumpMod(r);
    const missing = lockstep(r, "--base", "nope");
    expect(missing.ok).toBe(false);
    expect(missing.out).toContain('could not read package.json at "nope"');
    r.git("stash", "-u", "-q");
    r.git("checkout", "-q", "--orphan", "other");
    r.git("rm", "-rqf", ".");
    writeAll(r, pmFiles(PM));
    r.git("add", "-A");
    r.git("commit", "-qm", "unrelated");
    const orphan = r.git("rev-parse", "HEAD").trim();
    r.git("checkout", "-q", "-f", "main");
    const x = lockstep(r, "--base", orphan);
    expect(x.ok).toBe(false);
    expect(x.out).toContain("could not list what changed");
  });

  it("refuses --base and --tag with no value instead of skipping the check", () => {
    const r = repo();
    for (const args of [["--base"], ["--base", ""], ["--tag"], ["--tag", ""]]) {
      const x = lockstep(r, ...args);
      expect(x.ok, args.join(" ")).toBe(false);
      expect(x.out).toContain("needs a value");
    }
  });

  it("is still the plain lockstep for files that disagree", () => {
    const r = repo();
    put(r.root, "package.json", json({ version: "9.9.9" }));
    expect(lockstep(r).ok).toBe(false);
    expect(existsSync(join(r.root, "scripts", "semver.mjs"))).toBe(true);
  });
});
