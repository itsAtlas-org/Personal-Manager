// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// scripts/pack-mod.mjs: the one writer of `.pmmod` files and its mirror of package.rs's verifier.
//
// What this pins, and why each part is here:
//  - The CONSTANTS are read out of package.rs and compared, because the mirror is a second copy of
//    them and a copy drifts silently.
//  - Every REFUSAL the verifier mirrors (path rules, zip structure, checksums, manifest, licence) and
//    every built-output rule (no inline script or style, no module script, no `crossorigin`).
//  - DETERMINISM: the same inputs give the same bytes, in sorted order, STORED, with the fixed time.
//  - The two MODES (signed layout, unsigned developer file) and `attach`.
//  - A GOLDEN archive that package.rs verifies (src-tauri/src/mods/package.rs, `a_pack_mod_golden…`):
//    this script's bytes must equal the committed file, and the Rust verifier accepts that file, so
//    the two implementations of the format are proved to agree on at least one real archive.
// The real Vite build is in scripts/mods-build.test.mjs.

import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  renameSync,
  readFileSync,
  rmSync,
  symlinkSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { crc32, deflateRawSync } from "node:zlib";

import { afterAll, describe, expect, it, vi } from "vitest";

import * as pack from "./pack-mod.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..");
const packageRs = readFileSync(join(repoRoot, "src-tauri/src/mods/package.rs"), "utf8");

const scratch = [];
afterAll(() => {
  for (const dir of scratch) rmSync(dir, { recursive: true, force: true });
});
function tempDir() {
  const dir = mkdtempSync(join(tmpdir(), "pack-mod-test-"));
  scratch.push(dir);
  return dir;
}

const AGPL = "GNU AFFERO GENERAL PUBLIC LICENSE\nVersion 3, 19 November 2007\n";
const COMMIT = "1234567890abcdef1234567890abcdef12345678";
const sha = (b) => createHash("sha256").update(b).digest("hex");
/** `fn` throws a PackError whose message matches (toThrow returns nothing, so the two are two calls). */
function refused(fn, message) {
  expect(fn).toThrow(pack.PackError);
  expect(fn).toThrow(message);
}

// ---- Fixtures -----------------------------------------------------------------------------------

function baseManifest(over = {}) {
  return {
    id: "atlas.toy",
    name: "Toy",
    description: "A mod for the packer's tests.",
    version: "1.2.3",
    apiVersion: { min: "1.0", target: "1.0" },
    publisher: {
      tier: "first",
      author: "Atlas",
      sourceRepo: "itsAtlas-org/Personal-Manager",
      commit: "0".repeat(40),
    },
    licence: "AGPL-3.0-or-later",
    permissions: [{ id: "ui.tab", why: "Shows its tab." }],
    contributions: { "nav.tab": { label: "Toy" } },
    entry: { page: "app/index.html" },
    ...over,
  };
}
const thirdParty = (over = {}) =>
  baseManifest({
    id: "acme.toy",
    publisher: { tier: "third", author: "Acme", sourceRepo: "acme/toy", commit: "0".repeat(40) },
    ...over,
  });

const PAGE =
  '<!doctype html><html><head><script defer src="./assets/app.js"></script>' +
  '<link rel="stylesheet" href="./assets/style.css"></head><body><p>hi</p></body></html>';
const APP = {
  "index.html": PAGE,
  "assets/app.js": "(function(){var x=1;return x})();",
  "assets/style.css": "body{margin:0}",
};

/**
 * A repo root (LICENCE.txt and node_modules) and a mod folder with built output and its build
 * record, in a throwaway directory.
 */
function makeMod({ manifest = baseManifest(), app = APP, packages = [], record } = {}) {
  const root = tempDir();
  writeFileSync(join(root, "LICENCE.txt"), AGPL);
  for (const p of packages) {
    const dir = join(root, p.dir);
    mkdirSync(dir, { recursive: true });
    writeFileSync(
      join(dir, "package.json"),
      JSON.stringify({ name: p.name, version: p.version, license: p.license }),
    );
    if (p.text !== null) writeFileSync(join(dir, "LICENSE"), p.text);
  }
  const modDir = join(root, "mods", "m");
  mkdirSync(join(modDir, "dist", "app"), { recursive: true });
  writeFileSync(join(modDir, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  for (const [path, text] of Object.entries(app)) {
    mkdirSync(dirname(join(modDir, "dist", "app", path)), { recursive: true });
    writeFileSync(join(modDir, "dist", "app", path), text);
  }
  if (record !== null) {
    mkdirSync(join(modDir, ".build"), { recursive: true });
    writeFileSync(
      join(modDir, ".build", "modules.json"),
      JSON.stringify({
        format: 1,
        bundle: pack.outputDigest(join(modDir, "dist", "app")),
        modules: packages.map((p) => `${p.dir}/index.js`),
        ...record,
      }),
    );
  }
  return { root, modDir };
}
const pkg = (name, license = "ISC", extra = {}) => ({
  dir: `node_modules/${name}`,
  name,
  version: "1.0.0",
  license,
  text: `Copyright (c) ${name} authors`,
  ...extra,
});

const packed = (mod, opts = {}) =>
  pack.buildArchive(pack.assemble({ modDir: mod.modDir, root: mod.root, ...opts }), opts);

/** A zip built by hand, field by field, so a test can break exactly one thing. */
function rawZip(
  items,
  { comment = Buffer.alloc(0), gap = Buffer.alloc(0), pad = Buffer.alloc(0), eocd = {} } = {},
) {
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const item of items) {
    const name = Buffer.from(item.name);
    const lo = item.local ?? {}; // overrides for this entry's LOCAL header only
    const data = Buffer.from(item.data ?? "x");
    const method = item.method ?? 0;
    const stored = method === 8 ? deflateRawSync(data) : data;
    const crc = item.crc ?? crc32(data);
    const usize = item.usize ?? data.length;
    const csize = item.csize ?? stored.length;
    const flags = item.flags ?? 0;
    const extra = item.extra ?? Buffer.alloc(0);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(lo.sig ?? 0x04034b50, 0);
    local.writeUInt16LE(10, 4);
    local.writeUInt16LE(lo.flags ?? flags, 6);
    local.writeUInt16LE(lo.method ?? method, 8);
    local.writeUInt16LE(0, 10);
    local.writeUInt16LE(33, 12);
    local.writeUInt32LE(lo.crc ?? crc, 14);
    local.writeUInt32LE(lo.csize ?? csize, 18);
    local.writeUInt32LE(lo.usize ?? usize, 22);
    local.writeUInt16LE(name.length, 26);
    local.writeUInt16LE(lo.extraLen ?? 0, 28);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(item.madeBy ?? (3 << 8) | 20, 4);
    central.writeUInt16LE(10, 6);
    central.writeUInt16LE(flags, 8);
    central.writeUInt16LE(method, 10);
    central.writeUInt16LE(0, 12);
    central.writeUInt16LE(33, 14);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(csize, 20);
    central.writeUInt32LE(usize, 24);
    central.writeUInt16LE(name.length, 28);
    central.writeUInt16LE(extra.length, 30);
    central.writeUInt16LE(0, 32);
    central.writeUInt16LE(0, 34);
    central.writeUInt16LE(0, 36);
    central.writeUInt32LE(item.external ?? (0o100644 << 16) >>> 0, 38);
    central.writeUInt32LE(item.offset ?? offset, 42);
    locals.push(local, lo.name ?? name, stored);
    centrals.push(central, name, extra);
    offset += 30 + name.length + stored.length;
  }
  const directory = Buffer.concat([...centrals, pad]);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(eocd.disk ?? 0, 4);
  end.writeUInt16LE(eocd.directoryDisk ?? 0, 6);
  end.writeUInt16LE(items.length, 8);
  end.writeUInt16LE(items.length, 10);
  end.writeUInt32LE(directory.length, 12);
  end.writeUInt32LE(offset + gap.length + (eocd.offsetBy ?? 0), 16);
  end.writeUInt16LE(eocd.commentLength ?? comment.length, 20);
  return Buffer.concat([...locals, gap, directory, end, comment]);
}

// ---- Constants ----------------------------------------------------------------------------------

describe("constants agree with package.rs", () => {
  const rust = (re) => {
    const m = re.exec(packageRs);
    if (!m) throw new Error(`package.rs no longer matches ${re}`);
    return m[1];
  };
  const shifted = (s) =>
    s.includes("<<") ? Number(s.split("<<")[0]) * 2 ** Number(s.split("<<")[1]) : Number(s);

  it("permission ids", () => {
    const body = rust(/pub const PERMISSIONS: &\[&str\] = &\[([^\]]*)\]/);
    expect([...body.matchAll(/"([^"]+)"/g)].map((m) => m[1])).toEqual(pack.PERMISSIONS);
  });

  it("the most combining marks one letter may carry", () => {
    expect(Number(rust(/const MAX_STACKED_MARKS: usize = (\d+);/))).toBe(pack.MAX_STACKED_MARKS);
  });

  it("headers a secret may not go in", () => {
    const body = rust(/const FORBIDDEN_SECRET_HEADERS: &\[&str\] = &\[([^\]]*)\]/);
    const rustList = [...body.matchAll(/"([^"]+)"/g)].map((m) => m[1]);
    // The JS list is private; a header in the Rust list must be refused by the manifest check.
    for (const header of rustList) {
      const m = baseManifest({
        permissions: [
          { id: "ui.tab", why: "x" },
          { id: "internet", why: "x" },
        ],
        internetHosts: ["api.example.com"],
        secrets: [{ name: "k", host: "api.example.com", header }],
      });
      refused(() => pack.checkManifest(m, new Set(["app/index.html"])), "can't go in header");
    }
    expect(rustList.length).toBeGreaterThan(5);
  });

  it("size and depth caps, and the layout's file names", () => {
    expect(shifted(rust(/pub\(super\) const MANIFEST_CAP: usize = ([^;]+);/))).toBe(
      pack.MANIFEST_CAP,
    );
    expect(shifted(rust(/const CHECKSUMS_CAP: usize = ([^;]+);/))).toBe(pack.CHECKSUMS_CAP);
    expect(shifted(rust(/const SIGNATURE_CAP: usize = ([^;]+);/))).toBe(pack.SIGNATURE_CAP);
    expect(Number(rust(/const PATH_CAP: usize = (\d+);/))).toBe(pack.PATH_CAP);
    expect(Number(rust(/const COMPONENT_CAP: usize = (\d+);/))).toBe(pack.COMPONENT_CAP);
    expect(Number(rust(/const DEPTH_CAP: usize = (\d+);/))).toBe(pack.DEPTH_CAP);
    expect(rust(/pub const MANIFEST: &str = "([^"]+)"/)).toBe(pack.MANIFEST);
    expect(rust(/pub const LICENCE_FILE: &str = "([^"]+)"/)).toBe(pack.LICENCE_FILE);
    expect(rust(/pub const NOTICE_FILE: &str = "([^"]+)"/)).toBe(pack.NOTICE_FILE);
    expect(rust(/pub const CHECKSUMS: &str = "([^"]+)"/)).toBe(pack.CHECKSUMS);
    expect(rust(/pub const SIGNATURE: &str = "([^"]+)"/)).toBe(pack.SIGNATURE);
    expect(rust(/pub const LICENCE: &str = "([^"]+)"/)).toBe(pack.LICENCE);
    const headers = readFileSync(join(repoRoot, "src-tauri/src/mods/headers.rs"), "utf8");
    expect(/pub const APP_DIR: &str = "([^"]+)"/.exec(headers)[1]).toBe(pack.APP_DIR);
  });

  it("the default limits", () => {
    const body = rust(/pub const DEFAULT: Self = Self \{([^}]*)\}/);
    const field = (name) => shifted(new RegExp(`${name}: ([^,]+),`).exec(body)[1].trim());
    expect(field("file")).toBe(pack.LIMITS.file);
    expect(field("entries")).toBe(pack.LIMITS.entries);
    expect(field("entry")).toBe(pack.LIMITS.entry);
    expect(field("total")).toBe(pack.LIMITS.total);
  });
});

// ---- Paths --------------------------------------------------------------------------------------

describe("checkPath refuses what package.rs's check_path refuses", () => {
  it.each([
    ["", "empty name"],
    ["/app/x.js", "absolute path"],
    ["app\\x.js", "backslash"],
    ["C:/app/x.js", "colon"],
    ["app/x.js:stream", "colon"],
    ["app/../manifest.json", "dot segment"],
    ["app/./x.js", "dot segment"],
    ["../x.js", "dot segment"],
    ["app//x.js", "empty folder name"],
    ["app/x.js.", "trailing dot or space"],
    ["app/x.js ", "trailing dot or space"],
    ["app/x y.js", "character"],
    ["app/é.js", "character"],
    ["app/PROGRA~1.js", "character"],
    ["app/x%2e.js", "character"],
    ["app/x?.js", "character"],
    ["app/x\0.js", "character"],
    ["app/con.js", "device name"],
    ["app/NUL", "device name"],
    ["app/com1.txt", "device name"],
    ["app/lpt9.x", "device name"],
    ["app/aux/x.js", "device name"],
    ["other/x.js", "outside the package layout"],
    ["pmmod/extra.json", "outside the package layout"],
    ["README.md", "outside the package layout"],
    [`app/${"a".repeat(101)}`, "name too long"],
    [`app/${"a/".repeat(16)}x.js`, "too deep"],
    [`app/${"a".repeat(100)}/${"b".repeat(100)}`, "name too long"],
  ])("%j (%s)", (path, why) => {
    refused(() => pack.checkPath(path), why);
  });

  it.each([
    "manifest.json",
    "LICENCE.txt",
    "NOTICE.txt",
    "pmmod/checksums.json",
    "pmmod/signature.minisig",
    "app/index.html",
    "app/assets/app.js",
    "app/a-b_c+d.1/x.js",
    "app/console.js",
    "app/com10.js",
  ])("%s is allowed", (path) => {
    expect(() => pack.checkPath(path)).not.toThrow();
  });

  it("names equal after case folding, or a file and a folder, collide", () => {
    refused(() => pack.checkCaseCollisions(["app/Main.js", "app/main.JS"]), "same file");
    refused(() => pack.checkCaseCollisions(["app/x", "app/X/y.js"]), "file and a folder");
    expect(() => pack.checkCaseCollisions(["app/a.js", "app/b.js"])).not.toThrow();
  });
});

// ---- Strict JSON --------------------------------------------------------------------------------

describe("parseStrictJson", () => {
  it("reads ordinary JSON", () => {
    expect(pack.parseStrictJson('{"a":[1,2.5e1,"x\\u0041",true,null],"b":{}}')).toEqual({
      a: [1, 25, "xA", true, null],
      b: {},
    });
  });

  it.each([
    ['{"a":1,"a":2}', "repeated"],
    ['{"a":{"b":1,"b":1}}', "repeated"],
    ["\uFEFF{}", "unexpected"],
    ['{"a":1} x', "text after"],
    ['{"a":1,}', "expected a key"],
    ["{'a':1}", "expected a key"],
    ['{"a":01}', "expected a comma"],
    ['"\\ud800"', "lone surrogate"],
    ['"\\u12G4"', "bad \\u escape"],
    ['"\\q"', "bad escape"],
    ['"abc', "unterminated"],
    ['"\n"', "control character"],
    ['{"a":1', "expected a comma"],
    ["[".repeat(70) + "]".repeat(70), "too deep"],
  ])("refuses %j", (text, why) => {
    refused(() => pack.parseStrictJson(text), why);
  });

  it("keeps a __proto__ key as data", () => {
    const v = pack.parseStrictJson('{"__proto__":{"x":1}}');
    expect(Object.keys(v)).toEqual(["__proto__"]);
    expect({}.x).toBeUndefined();
  });
});

// ---- The manifest -------------------------------------------------------------------------------

describe("checkManifest mirrors package.rs's parse_manifest", () => {
  const files = new Set([
    "app/index.html",
    "app/w.js",
    "app/icon.svg",
    "app/assets/app.js",
    "icon.svg",
  ]);
  const check = (over, drop = []) => {
    const m = baseManifest(over);
    for (const k of drop) delete m[k];
    return pack.checkManifest(m, files);
  };

  it("accepts a full manifest and summarises it", () => {
    const summary = check({
      icon: "app/icon.svg",
      permissions: [
        { id: "ui.tab", why: "Shows its tab." },
        { id: "storage", why: "Keeps settings." },
        { id: "internet", why: "Talks to a service." },
      ],
      internetHosts: ["api.example.com"],
      secrets: [{ name: "api_key", host: "api.example.com", header: "X-Api-Key" }],
      contributions: {
        "nav.tab": { label: "Toy" },
        "calendar.overlay": { label: "Toy events" },
        help: [{ id: "intro", title: "Intro", body: "Line one.\nLine two." }],
      },
      data: {
        keys: [{ key: "settings", class: "content" }],
        projectRefs: [{ key: "settings", path: "$.project" }],
      },
      entry: { page: "app/index.html", workers: ["app/w.js"] },
      requires: ["calendar", "localai"],
      engines: { frame: true },
    });
    expect(summary).toEqual({
      id: "atlas.toy",
      version: "1.2.3",
      tier: "first",
      author: "Atlas",
      sourceRepo: "itsAtlas-org/Personal-Manager",
      commit: "0".repeat(40),
      api: "1.0-1.0",
      entryPage: "app/index.html",
    });
  });

  it.each([
    ["an unknown top-level field", { extra: 1 }, [], "unknown field"],
    ["a missing required field", {}, ["name"], "has no name"],
    ["a null icon", { icon: null }, [], "icon must be text"],
    ["an icon that is not an image", { icon: "app/index.html" }, [], "icon must be"],
    [
      "an icon that is not a package file",
      { icon: "app/missing.svg" },
      [],
      "isn't one of its app/",
    ],
    ["an id outside the grammar", { id: "Atlas.toy" }, [], "dot-separated"],
    ["an id with one part", { id: "toy" }, [], "two to five"],
    ["an id that names a device", { id: "atlas.con" }, [], "device"],
    ["a name that is empty", { name: "" }, [], "name is empty"],
    ["a name with a hidden character", { name: "To\u200Fy" }, [], "hidden or control"],
    ["a name that is too long", { name: "x".repeat(41) }, [], "longer than 40"],
    ["a name with a leading space", { name: " Toy" }, [], "starts or ends"],
    ["a description over the cap", { description: "x".repeat(281) }, [], "longer than 280"],
    ["a version that is not plain semver", { version: "1.2" }, [], "plain semver"],
    ["a version with build metadata", { version: "1.2.3+b" }, [], "plain semver"],
    ["a version with a leading zero", { version: "01.2.3" }, [], "plain semver"],
    [
      "an API range that is reversed",
      { apiVersion: { min: "2.0", target: "1.0" } },
      [],
      "min <= target",
    ],
    [
      "an API version with a leading zero",
      { apiVersion: { min: "01.0", target: "1.0" } },
      [],
      "min <= target",
    ],
    [
      "an unknown publisher field",
      { publisher: { ...baseManifest().publisher, x: 1 } },
      [],
      "unknown field",
    ],
    [
      "a tier that does not exist",
      { publisher: { ...baseManifest().publisher, tier: "fourth" } },
      [],
      "publisher.tier",
    ],
    [
      "a source repo that is not owner/repo",
      { publisher: { ...baseManifest().publisher, sourceRepo: "x" } },
      [],
      "owner/repo",
    ],
    [
      "a commit that is not 40 hex",
      { publisher: { ...baseManifest().publisher, commit: "abc" } },
      [],
      "40-digit",
    ],
    [
      "an upper-case commit",
      { publisher: { ...baseManifest().publisher, commit: "A".repeat(40) } },
      [],
      "40-digit",
    ],
    ["another licence", { licence: "MIT" }, [], "licence must be"],
    [
      "an unknown permission",
      { permissions: [{ id: "root", why: "x" }] },
      [],
      "unknown permission",
    ],
    [
      "a permission listed twice",
      {
        permissions: [
          { id: "ui.tab", why: "x" },
          { id: "ui.tab", why: "y" },
        ],
      },
      [],
      "listed twice",
    ],
    ["a permission without a reason", { permissions: [{ id: "ui.tab" }] }, [], "has no why"],
    [
      "internet without hosts",
      {
        permissions: [
          { id: "ui.tab", why: "x" },
          { id: "internet", why: "x" },
        ],
      },
      [],
      "go together",
    ],
    ["hosts without internet", { internetHosts: ["api.example.com"] }, [], "go together"],
    [
      "a host that is an IP literal",
      {
        permissions: [
          { id: "internet", why: "x" },
          { id: "ui.tab", why: "x" },
        ],
        internetHosts: ["10.0.0.1"],
      },
      [],
      "isn't an internet host",
    ],
    [
      "a localhost host",
      {
        permissions: [
          { id: "internet", why: "x" },
          { id: "ui.tab", why: "x" },
        ],
        internetHosts: ["localhost"],
      },
      [],
      "isn't an internet host",
    ],
    [
      "a host with a port",
      {
        permissions: [
          { id: "internet", why: "x" },
          { id: "ui.tab", why: "x" },
        ],
        internetHosts: ["a.example.com:8080"],
      },
      [],
      "isn't an internet host",
    ],
    [
      "a secret going to an unlisted host",
      {
        permissions: [
          { id: "internet", why: "x" },
          { id: "ui.tab", why: "x" },
        ],
        internetHosts: ["a.example.com"],
        secrets: [{ name: "k", host: "b.example.com", header: "X-K" }],
      },
      [],
      "doesn't list",
    ],
    [
      "a secret in a forbidden header",
      {
        permissions: [
          { id: "internet", why: "x" },
          { id: "ui.tab", why: "x" },
        ],
        internetHosts: ["a.example.com"],
        secrets: [{ name: "k", host: "a.example.com", header: "Cookie" }],
      },
      [],
      "can't go in header",
    ],
    ["a nav.tab without ui.tab", { permissions: [] }, [], "needs the ui.tab"],
    [
      "a nav.tab label that is too long",
      { contributions: { "nav.tab": { label: "x".repeat(25) } } },
      [],
      "longer than 24",
    ],
    ["a null nav.tab", { contributions: { "nav.tab": null } }, [], "must be an object"],
    [
      "a help id with capitals",
      { contributions: { help: [{ id: "Intro", title: "t", body: "b" }] } },
      [],
      "help id",
    ],
    [
      "an empty help body",
      { contributions: { help: [{ id: "a", title: "t", body: "" }] } },
      [],
      "is empty",
    ],
    [
      "data keys without storage",
      { data: { keys: [{ key: "a", class: "cache" }] } },
      [],
      "storage permission",
    ],
    [
      "a data key with a bad class",
      {
        permissions: [
          { id: "storage", why: "x" },
          { id: "ui.tab", why: "x" },
        ],
        data: { keys: [{ key: "a", class: "secret" }] },
      },
      [],
      "class",
    ],
    [
      "a data key with capitals",
      {
        permissions: [
          { id: "storage", why: "x" },
          { id: "ui.tab", why: "x" },
        ],
        data: { keys: [{ key: "Aa", class: "cache" }] },
      },
      [],
      "data key",
    ],
    [
      "a projectRef naming an undeclared key",
      {
        permissions: [
          { id: "storage", why: "x" },
          { id: "ui.tab", why: "x" },
        ],
        data: { projectRefs: [{ key: "a", path: "$.x" }] },
      },
      [],
      "doesn't declare",
    ],
    ["an entry that is not html", { entry: { page: "app/w.js" } }, [], "must be an .html"],
    ["an entry outside app/", { entry: { page: "manifest.json" } }, [], "isn't one of its app/"],
    ["a missing entry file", { entry: { page: "app/nope.html" } }, [], "isn't one of its app/"],
    [
      "a worker that is not a script",
      { entry: { page: "app/index.html", workers: ["app/index.html"] } },
      [],
      "isn't a script",
    ],
    [
      "a worker listed twice",
      { entry: { page: "app/index.html", workers: ["app/w.js", "app/w.js"] } },
      [],
      "listed twice",
    ],
    ["a required room that is unknown", { requires: ["mail"] }, [], "required room"],
    ["a required room listed twice", { requires: ["calendar", "calendar"] }, [], "listed twice"],
    ["engines that are not frame:true", { engines: { frame: false } }, [], "frame"],
    ["a null engines", { engines: null }, [], "must be an object"],
    ["a permissions value that is not a list", { permissions: "ui.tab" }, [], "must be a list"],
    ["internetHosts that is not a list", { internetHosts: {} }, [], "must be a list"],
    ["a name with a control character", { name: "To\u0007y" }, [], "hidden or control"],
    ["a name with a no-break space", { name: "To y" }, [], "hidden or control"],
    [
      "an author that is empty",
      { publisher: { ...baseManifest().publisher, author: "" } },
      [],
      "author is empty",
    ],
    [
      "an author with a hidden character",
      { publisher: { ...baseManifest().publisher, author: "A‮b" } },
      [],
      "hidden or control",
    ],
    [
      "a permission reason that is empty",
      { permissions: [{ id: "ui.tab", why: "" }] },
      [],
      "is empty",
    ],
    [
      "a permission reason over the cap",
      { permissions: [{ id: "ui.tab", why: "x".repeat(161) }] },
      [],
      "longer than 160",
    ],
    [
      "more than 32 internet hosts",
      {
        permissions: [
          { id: "internet", why: "x" },
          { id: "ui.tab", why: "x" },
        ],
        internetHosts: Array.from({ length: 33 }, (_, i) => `h${i}.example.com`),
      },
      [],
      "too many internet hosts",
    ],
    [
      "a host listed twice",
      {
        permissions: [
          { id: "internet", why: "x" },
          { id: "ui.tab", why: "x" },
        ],
        internetHosts: ["a.example.com", "a.example.com"],
      },
      [],
      "listed twice",
    ],
    [
      "a *.localhost host",
      {
        permissions: [
          { id: "internet", why: "x" },
          { id: "ui.tab", why: "x" },
        ],
        internetHosts: ["app.localhost"],
      },
      [],
      "isn't an internet host",
    ],
    [
      "a secret name that starts with a capital",
      {
        permissions: [
          { id: "internet", why: "x" },
          { id: "ui.tab", why: "x" },
        ],
        internetHosts: ["a.example.com"],
        secrets: [{ name: "Key", host: "a.example.com", header: "X-K" }],
      },
      [],
      "secret name",
    ],
    [
      "a secret name listed twice",
      {
        permissions: [
          { id: "internet", why: "x" },
          { id: "ui.tab", why: "x" },
        ],
        internetHosts: ["a.example.com"],
        secrets: [
          { name: "k", host: "a.example.com", header: "X-K" },
          { name: "k", host: "a.example.com", header: "X-L" },
        ],
      },
      [],
      "listed twice",
    ],
    [
      "more than 16 secrets",
      {
        permissions: [
          { id: "internet", why: "x" },
          { id: "ui.tab", why: "x" },
        ],
        internetHosts: ["a.example.com"],
        secrets: Array.from({ length: 17 }, (_, i) => ({
          name: `k${i}`,
          host: "a.example.com",
          header: "X-K",
        })),
      },
      [],
      "too many secrets",
    ],
    [
      "a secret header that starts with a digit",
      {
        permissions: [
          { id: "internet", why: "x" },
          { id: "ui.tab", why: "x" },
        ],
        internetHosts: ["a.example.com"],
        secrets: [{ name: "k", host: "a.example.com", header: "1-K" }],
      },
      [],
      "can't go in header",
    ],
    [
      "more than 64 help entries",
      {
        contributions: {
          help: Array.from({ length: 65 }, (_, i) => ({ id: `h${i}`, title: "t", body: "b" })),
        },
      },
      [],
      "too many help entries",
    ],
    [
      "more than 64 data keys",
      {
        permissions: [
          { id: "storage", why: "x" },
          { id: "ui.tab", why: "x" },
        ],
        data: { keys: Array.from({ length: 65 }, (_, i) => ({ key: `k${i}`, class: "cache" })) },
      },
      [],
      "too many data keys",
    ],
    [
      "a projectRef path with a space",
      {
        permissions: [
          { id: "storage", why: "x" },
          { id: "ui.tab", why: "x" },
        ],
        data: { keys: [{ key: "a", class: "cache" }], projectRefs: [{ key: "a", path: "$. x" }] },
      },
      [],
      "projectRef path",
    ],
    [
      "more than 8 workers",
      { entry: { page: "app/index.html", workers: Array.from({ length: 9 }, () => "app/w.js") } },
      [],
      "too many workers",
    ],
    ["an icon that is not under app/", { icon: "icon.svg" }, [], "isn't one of its app/"],
  ])("refuses %s", (_what, over, drop, why) => {
    refused(() => check(over, drop), why);
  });

  it("holds first party to atlas. ids and everyone else to other ids", () => {
    expect(() => pack.checkTier("atlas.map", "first")).not.toThrow();
    expect(() => pack.checkTier("acme.map", "second")).not.toThrow();
    expect(() => pack.checkTier("acme.map", "third")).not.toThrow();
    refused(() => pack.checkTier("atlas.map", "second"), "atlas.");
    refused(() => pack.checkTier("Atlas.map", "third"), "atlas.");
    refused(() => pack.checkTier("acme.map", "first"), "must start");
  });
});

// ---- The zip: writing ---------------------------------------------------------------------------

describe("writeZip", () => {
  const entries = new Map([
    ["manifest.json", Buffer.from("bb")],
    ["app/z.js", Buffer.from("zz")],
    ["LICENCE.txt", Buffer.from("l")],
    ["app/a.js", Buffer.from("aa")],
  ]);

  it("sorts entries by path whatever order they arrive in", () => {
    const read = pack.readZip(pack.writeZip(entries));
    // Byte order: upper-case before lower-case, so LICENCE.txt comes before app/.
    expect([...read.keys()]).toEqual(["LICENCE.txt", "app/a.js", "app/z.js", "manifest.json"]);
  });

  it("is a pure function of its entries (same bytes every time)", () => {
    const shuffled = new Map([...entries].reverse());
    expect(pack.writeZip(shuffled).equals(pack.writeZip(entries))).toBe(true);
  });

  it("stores, never deflates, with the fixed 1980 time and Unix mode 0644", () => {
    const bytes = pack.writeZip(new Map([["LICENCE.txt", Buffer.from("a".repeat(1000))]]));
    const at = bytes.indexOf(Buffer.from([0x50, 0x4b, 0x01, 0x02]));
    expect(bytes.readUInt16LE(at + 4) >> 8).toBe(3); // creator: Unix
    expect(bytes.readUInt16LE(at + 10)).toBe(0); // stored
    expect(bytes.readUInt16LE(at + 12)).toBe(0); // 00:00:00
    expect(bytes.readUInt16LE(at + 14)).toBe((1 << 5) | 1); // 1980-01-01
    expect(bytes.readUInt32LE(at + 20)).toBe(1000); // compressed == uncompressed
    expect(bytes.readUInt32LE(at + 38) >>> 16).toBe(0o100644);
    expect(bytes.readUInt16LE(at + 30)).toBe(0); // no extra field
    expect(bytes.readUInt16LE(at + 32)).toBe(0); // no comment
  });

  it("writes no folder entries and nothing else after the directory", () => {
    const bytes = pack.writeZip(new Map([["app/sub/x.js", Buffer.from("x")]]));
    expect([...pack.readZip(bytes).keys()]).toEqual(["app/sub/x.js"]);
    expect(bytes.readUInt32LE(bytes.length - 22)).toBe(0x06054b50);
  });
});

// ---- The zip: reading ---------------------------------------------------------------------------

describe("readZip refuses a broken archive", () => {
  const ok = [{ name: "app/a.js", data: "aaa" }];

  it("reads the hand-built baseline, stored and deflated", () => {
    const read = pack.readZip(
      rawZip([
        { name: "app/a.js", data: "aaa" },
        { name: "app/b.js", data: "b".repeat(500), method: 8 },
      ]),
    );
    expect(read.get("app/a.js").toString()).toBe("aaa");
    expect(read.get("app/b.js").length).toBe(500);
  });

  const cases = [
    ["folder entries", [{ name: "app/sub/", data: "" }], "folder entry"],
    ["a DOS folder bit", [{ name: "app/d", external: 0x10 }], "folder entry"],
    ["a symlink", [{ name: "app/l.js", external: (0o120777 << 16) >>> 0 }], "isn't a plain file"],
    ["a FIFO", [{ name: "app/f", external: (0o010644 << 16) >>> 0 }], "isn't a plain file"],
    ["a name outside the layout", [{ name: "other/x.js" }], "outside the package layout"],
    ["a name with a colon", [{ name: "app/x:y" }], "colon"],
    ["a name twice", [{ name: "app/a.js" }, { name: "app/a.js", data: "y" }], "one file twice"],
    [
      "names equal after case folding",
      [{ name: "app/A.js" }, { name: "app/a.js", data: "y" }],
      "same file",
    ],
    ["an encrypted entry", [{ name: "app/a.js", flags: 1 }], "encrypted"],
    ["a data descriptor", [{ name: "app/a.js", flags: 8 }], "flag that isn't allowed"],
    ["another compression method", [{ name: "app/a.js", method: 12 }], "stored or deflated"],
    ["an extra field", [{ name: "app/a.js", extra: Buffer.from([1, 2, 3, 4]) }], "extra fields"],
    ["a wrong checksum", [{ name: "app/a.js", crc: 1 }], "damaged"],
    ["a stored entry whose sizes disagree", [{ name: "app/a.js", csize: 2 }], "sizes disagree"],
    [
      "a size that lies about the content",
      [{ name: "app/a.js", method: 8, data: "x".repeat(100), usize: 3 }],
      "couldn't be read",
    ],
    [
      "an entry that starts in the wrong place",
      [{ name: "app/a.js", offset: 5 }],
      "overlap or leave a gap",
    ],
    ["a name that is not UTF-8", [{ name: Buffer.from([0x61, 0xff]) }], "isn't a plain file name"],
  ];
  it.each(cases)("%s", (_what, items, why) => {
    refused(() => pack.readZip(rawZip(items)), why);
  });

  it.each([
    [
      "a local header with a damaged signature",
      [{ name: "app/a.js", local: { sig: 0 } }],
      {},
      "local header is damaged",
    ],
    [
      "a local header naming another file",
      [{ name: "app/a.js", local: { name: Buffer.from("app/b.js") } }],
      {},
      "headers don't agree",
    ],
    [
      "a local header with an extra field",
      [{ name: "app/a.js", local: { extraLen: 1 } }],
      {},
      "headers don't agree",
    ],
    [
      "a local header with other flags",
      [{ name: "app/a.js", local: { flags: 0x800 } }],
      {},
      "headers don't agree",
    ],
    [
      "a local header with another method",
      [{ name: "app/a.js", local: { method: 8 } }],
      {},
      "headers don't agree",
    ],
    [
      "a local header with another checksum",
      [{ name: "app/a.js", local: { crc: 7 } }],
      {},
      "headers don't agree",
    ],
    [
      "a local header with another stored size",
      [{ name: "app/a.js", local: { csize: 2 } }],
      {},
      "headers don't agree",
    ],
    [
      "a local header with another size",
      [{ name: "app/a.js", local: { usize: 2 } }],
      {},
      "headers don't agree",
    ],
    [
      "an entry longer than the file holds",
      [{ name: "app/a.js", csize: 100, usize: 100 }],
      {},
      "runs into the directory",
    ],
    [
      "a deflated entry shorter than its size says",
      [{ name: "app/a.js", method: 8, data: "x".repeat(10), usize: 50 }],
      {},
      "damaged",
    ],
    [
      "an end record with a disk number",
      [{ name: "app/a.js" }],
      { eocd: { disk: 1 } },
      "multi-disk",
    ],
    [
      "an end record with a directory disk number",
      [{ name: "app/a.js" }],
      { eocd: { directoryDisk: 1 } },
      "multi-disk",
    ],
    [
      "an end record that claims a comment",
      [{ name: "app/a.js" }],
      { eocd: { commentLength: 1 } },
      "commented",
    ],
    [
      "a directory that does not end at the end record",
      [{ name: "app/a.js" }],
      { eocd: { offsetBy: 1 } },
      "doesn't end where",
    ],
    [
      "bytes after the last directory entry",
      [{ name: "app/a.js" }],
      { pad: Buffer.from("junk") },
      "trailing bytes",
    ],
    [
      "bytes between the entries and the directory",
      [{ name: "app/a.js" }],
      { gap: Buffer.from("junk") },
      "bytes between",
    ],
  ])("%s", (_what, items, opts, why) => {
    refused(() => pack.readZip(rawZip(items, opts)), why);
  });

  it("a damaged directory entry signature", () => {
    const bytes = Buffer.from(rawZip(ok));
    bytes[bytes.indexOf(Buffer.from([0x50, 0x4b, 0x01, 0x02])) + 2] = 0;
    refused(() => pack.readZip(bytes), "directory entry is damaged");
  });

  it("a comment, bytes in front, bytes behind, and a missing end record", () => {
    refused(() => pack.readZip(rawZip(ok, { comment: Buffer.from("hi") })), "end record");
    refused(() => pack.readZip(Buffer.concat([Buffer.from("junk"), rawZip(ok)])), "zip");
    refused(() => pack.readZip(Buffer.concat([rawZip(ok), Buffer.from("junk")])), "end record");
    refused(() => pack.readZip(Buffer.from("PK")), "end record");
    refused(() => pack.readZip(Buffer.alloc(0)), "end record");
  });

  it("flipping one byte of the data fails its checksum", () => {
    const bytes = Buffer.from(rawZip(ok));
    bytes[30 + "app/a.js".length] ^= 0xff;
    refused(() => pack.readZip(bytes), "damaged");
  });

  it("a deflated bomb is cut at its declared size, not inflated", () => {
    const bomb = rawZip([{ name: "app/a.js", method: 8, data: Buffer.alloc(5 << 20), usize: 100 }]);
    // Stopped while inflating (not after it: "damaged" is the message for a finished, wrong-size entry).
    refused(() => pack.readZip(bomb), /couldn't be read$/);
  });

  it("the caps: entries, one file, all files, the whole file", () => {
    const three = rawZip([{ name: "app/a" }, { name: "app/b" }, { name: "app/c" }]);
    refused(() => pack.readZip(three, { ...pack.LIMITS, entries: 2 }), "too many");
    refused(
      () => pack.readZip(rawZip([{ name: "app/a", data: "xxxx" }]), { ...pack.LIMITS, entry: 3 }),
      "too large",
    );
    refused(
      () =>
        pack.readZip(
          rawZip([
            { name: "app/a", data: "xxx" },
            { name: "app/b", data: "xxx" },
          ]),
          { ...pack.LIMITS, total: 5 },
        ),
      "too large together",
    );
    refused(() => pack.readZip(three, { ...pack.LIMITS, file: 10 }), "too large");
  });
});

// ---- Bundled packages and NOTICE ----------------------------------------------------------------

describe("which npm package a bundled module belongs to", () => {
  it.each([
    ["node_modules/d3-force/dist/d3-force.mjs", "node_modules/d3-force"],
    ["node_modules/d3-dispatch/src/dispatch.js", "node_modules/d3-dispatch"],
    ["node_modules/@scope/pkg/lib/x.js", "node_modules/@scope/pkg"],
    ["node_modules/a/node_modules/b/index.js", "node_modules/a/node_modules/b"],
    ["node_modules/a/node_modules/@s/b/x/y.js", "node_modules/a/node_modules/@s/b"],
    ["packages/x/node_modules/y/z.js", "packages/x/node_modules/y"],
  ])("%s", (path, dir) => {
    expect(pack.packageDirOf(path)).toBe(dir);
  });

  it.each([
    ["../node_modules/a/x.js", "leaves the repository"],
    ["/abs/node_modules/a/x.js", "leaves the repository"],
    ["node_modules/../a/x.js", "leaves the repository"],
    ["node_modules//a/x.js", "leaves the repository"],
    ["src/x.js", "isn't inside a package"],
    ["node_modules/a", "isn't inside a package"],
    ["node_modules/@s/a", "isn't inside a package"],
    ["node_modules", "isn't inside a package"],
  ])("refuses %s", (path, why) => {
    refused(() => pack.packageDirOf(path), why);
  });
});

describe("bundledPackages and the NOTICE", () => {
  it("reads each package's version, licence and text, once per package, in name order", () => {
    const mod = makeMod({
      packages: [pkg("zeta"), pkg("alpha", "MIT OR Apache-2.0")],
      record: {
        modules: ["node_modules/zeta/a.js", "node_modules/alpha/b.js", "node_modules/alpha/c.js"],
      },
    });
    const list = pack.bundledPackages(mod.modDir, mod.root);
    expect(list.map((p) => p.name)).toEqual(["alpha", "zeta"]);
    const text = pack.noticeText(list).toString();
    expect(text).toContain("This mod bundles 2 npm packages.");
    expect(text.indexOf("alpha 1.0.0")).toBeLessThan(text.indexOf("zeta 1.0.0"));
    expect(text).toContain("SPDX-License-Identifier: MIT OR Apache-2.0");
    expect(text).toContain("Copyright (c) zeta authors");
    expect(text).not.toContain("\r");
  });

  it("orders the NOTICE by name and then version whatever order the packages arrive in", () => {
    const p = (name, version) => ({ name, version, license: "MIT", text: `text of ${name}` });
    const text = pack
      .noticeText([p("zeta", "1.0.0"), p("alpha", "2.0.0"), p("alpha", "1.0.0")])
      .toString();
    const at = (s) => text.indexOf(s);
    expect(at("alpha 1.0.0")).toBeLessThan(at("alpha 2.0.0"));
    expect(at("alpha 2.0.0")).toBeLessThan(at("zeta 1.0.0"));
    expect(text).toContain("This mod bundles 3 npm packages.");
    expect(pack.noticeText([p("solo", "1.0.0")]).toString()).toContain("bundles 1 npm package.");
  });

  it("refuses a build record of the wrong shape, one field at a time", () => {
    for (const record of [
      { bundle: 5 },
      { modules: "node_modules/x/i.js" },
      { modules: [1] },
      { format: "1" },
    ]) {
      const mod = makeMod({ record });
      refused(() => pack.bundledPackages(mod.modDir, mod.root), "isn't a module record");
    }
  });

  it("says plainly when a mod bundles nothing", () => {
    const text = pack.noticeText([]).toString();
    expect(text).toContain("This mod bundles no npm packages.");
  });

  it("refuses a package whose licence is not on the allow-list", () => {
    const mod = makeMod({ packages: [pkg("copyleft", "GPL-3.0-only")] });
    refused(() => pack.bundledPackages(mod.modDir, mod.root), "not in the accepted set");
  });

  it("refuses a package that declares no licence, or a licence object", () => {
    const none = makeMod({ packages: [pkg("nolicence", undefined, { license: undefined })] });
    refused(() => pack.bundledPackages(none.modDir, none.root), "not in the accepted set");
    const legacy = makeMod({ packages: [pkg("legacy", { type: "MIT" })] });
    refused(() => pack.bundledPackages(legacy.modDir, legacy.root), "not in the accepted set");
  });

  it("refuses a package that ships no licence text", () => {
    const mod = makeMod({ packages: [pkg("silent", "MIT", { text: null })] });
    refused(() => pack.bundledPackages(mod.modDir, mod.root), "no licence text");
  });

  it("refuses a package that isn't installed, or isn't the one its folder says", () => {
    const missing = makeMod({ packages: [pkg("gone")] });
    rmSync(join(missing.root, "node_modules/gone/package.json"));
    refused(() => pack.bundledPackages(missing.modDir, missing.root), "has no package.json");
    const liar = makeMod({ packages: [pkg("liar", "MIT", { name: "someone-else" })] });
    refused(() => pack.bundledPackages(liar.modDir, liar.root), "names");
  });

  it("refuses a module path that leaves the repository", () => {
    const mod = makeMod({ record: { modules: ["../outside/node_modules/x/index.js"] } });
    refused(() => pack.bundledPackages(mod.modDir, mod.root), "leaves the repository");
  });

  it("refuses without a build record, with a malformed one, and with one for other output", () => {
    const none = makeMod({ record: null });
    refused(() => pack.bundledPackages(none.modDir, none.root), "build the mod first");
    const odd = makeMod({ record: { format: 2 } });
    refused(() => pack.bundledPackages(odd.modDir, odd.root), "isn't a module record");
    const stale = makeMod({ packages: [pkg("p")] });
    writeFileSync(
      join(stale.modDir, "dist", "app", "assets", "app.js"),
      "(function(){})();//changed",
    );
    refused(() => pack.bundledPackages(stale.modDir, stale.root), "not for the output");
  });

  it("a folder's digest is sha256 over sorted `path NUL sha256 LF` lines, whatever order the files were made in", () => {
    // `a-b.txt` sorts before `a/x.txt` as a whole path ('-' is 0x2d, '/' is 0x2f), but a walk that
    // lists each folder in order visits the folder `a` first: only a final sort gets this right.
    const names = ["a.txt", "a-b.txt", "a/x.txt", "m/b.txt", "m/z.txt", "z.txt", "m/n/c.txt"];
    const expected = createHash("sha256");
    for (const name of [...names].sort()) {
      expected.update(`${name}\0${sha(Buffer.from(`content of ${name}`))}\n`);
    }
    // One folder made in ascending order and one in descending order: whichever order a filesystem
    // lists a folder in (creation order, reverse, hash order), at least one of them is out of order.
    for (const order of [names, [...names].reverse()]) {
      const dir = tempDir();
      for (const name of order) {
        mkdirSync(dirname(join(dir, name)), { recursive: true });
        writeFileSync(join(dir, name), `content of ${name}`);
      }
      expect(pack.outputDigest(dir)).toBe(expected.copy().digest("hex"));
    }
  });

  it("a folder's digest changes with a file added, removed, renamed or edited", () => {
    const mod = makeMod();
    const dir = join(mod.modDir, "dist", "app");
    const seen = new Set([pack.outputDigest(dir)]);
    writeFileSync(join(dir, "extra.txt"), "x");
    seen.add(pack.outputDigest(dir));
    writeFileSync(join(dir, "extra.txt"), "y");
    seen.add(pack.outputDigest(dir));
    renameSync(join(dir, "extra.txt"), join(dir, "other.txt"));
    seen.add(pack.outputDigest(dir));
    rmSync(join(dir, "other.txt"));
    expect(seen.size).toBe(4);
    expect(pack.outputDigest(dir)).toBe([...seen][0]);
  });
});

// ---- Packing ------------------------------------------------------------------------------------

describe("packing", () => {
  it("the same inputs give the same bytes, in sorted order, with the layout the Rust verifier expects", () => {
    const a = packed(makeMod({ packages: [pkg("p1")] }), { commit: COMMIT });
    const b = packed(makeMod({ packages: [pkg("p1")] }), { commit: COMMIT });
    expect(a.equals(b)).toBe(true);
    const read = pack.readZip(a);
    const names = [...read.keys()];
    expect(names).toEqual([...names].sort());
    expect(names).toEqual([
      "LICENCE.txt",
      "NOTICE.txt",
      "app/assets/app.js",
      "app/assets/style.css",
      "app/index.html",
      "manifest.json",
      "pmmod/checksums.json",
    ]);
  });

  it("files are in the archive byte for byte, LICENCE.txt verbatim, and checksums list all but pmmod/", () => {
    const mod = makeMod();
    const read = pack.readZip(packed(mod));
    expect(read.get("LICENCE.txt").toString()).toBe(AGPL);
    expect(read.get("app/index.html").toString()).toBe(PAGE);
    const sums = JSON.parse(read.get("pmmod/checksums.json"));
    expect(sums.format).toBe(1);
    expect(sums.files.map((f) => f.path)).toEqual(
      [...read.keys()].filter((n) => !n.startsWith("pmmod/")),
    );
    for (const f of sums.files) expect(f.sha256).toBe(sha(read.get(f.path)));
  });

  it("refuses an oversize manifest, a manifest without a publisher object (with --commit), a missing LICENCE.txt", () => {
    const big = makeMod();
    writeFileSync(join(big.modDir, "manifest.json"), " ".repeat(pack.MANIFEST_CAP + 1));
    refused(() => pack.assemble({ modDir: big.modDir, root: big.root }), "manifest is too large");
    const bare = makeMod({ manifest: baseManifest({ publisher: "someone" }) });
    refused(
      () => pack.assemble({ modDir: bare.modDir, root: bare.root, commit: COMMIT }),
      "publisher must be an object",
    );
    const none = makeMod();
    rmSync(join(none.root, "LICENCE.txt"));
    refused(
      () => pack.assemble({ modDir: none.modDir, root: none.root }),
      "LICENCE.txt is missing",
    );
  });

  it("says --unsigned can't be used for an atlas. id in its own words, before the verifier gets there", () => {
    const mod = makeMod({ manifest: baseManifest({ publisher: { ...thirdParty().publisher } }) });
    refused(() => packed(mod, { unsigned: true }), "an unsigned file can't use one");
  });

  it("stages a third-party mod too (the stage follows the manifest's tier)", () => {
    const mod = makeMod({ manifest: thirdParty() });
    const log = vi.spyOn(console, "log").mockImplementation(() => {});
    try {
      pack.run([mod.modDir, "--stage"], mod.root);
    } finally {
      log.mockRestore();
    }
    expect(JSON.parse(readFileSync(join(mod.modDir, "dist", "manifest.json"))).id).toBe("acme.toy");
  });

  it("does not depend on file mtimes or on where the repo is", () => {
    const first = packed(makeMod());
    const second = makeMod();
    // A different place (another temp folder) and different times: only content is packed.
    const then = new Date("2001-02-03T04:05:06Z");
    for (const f of ["manifest.json", "dist/app/index.html", "dist/app/assets/app.js"]) {
      utimesSync(join(second.modDir, f), then, then);
    }
    expect(packed(second).equals(first)).toBe(true);
  });

  it("stamps --commit into the packaged manifest only, and refuses a bad one", () => {
    const mod = makeMod();
    const stamped = pack.readZip(packed(mod, { commit: COMMIT }));
    expect(JSON.parse(stamped.get("manifest.json")).publisher.commit).toBe(COMMIT);
    expect(JSON.parse(readFileSync(join(mod.modDir, "manifest.json"))).publisher.commit).toBe(
      "0".repeat(40),
    );
    const plain = pack.readZip(packed(mod));
    expect(plain.get("manifest.json").equals(readFileSync(join(mod.modDir, "manifest.json")))).toBe(
      true,
    );
    for (const bad of ["abc", COMMIT.toUpperCase(), `${COMMIT}0`, ""]) {
      refused(
        () => pack.assemble({ modDir: mod.modDir, root: mod.root, commit: bad }),
        "40 lowercase hex",
      );
    }
  });

  it("the signed layout needs first or second party; --unsigned needs third party and a non-atlas id", () => {
    const first = makeMod();
    expect(pack.verifyArchive(packed(first)).kind).toBe("pre-sign");
    refused(() => packed(first, { unsigned: true }), "must say it is third party");
    const third = makeMod({ manifest: thirdParty() });
    expect(pack.verifyArchive(packed(third, { unsigned: true })).kind).toBe("unsigned");
    refused(() => packed(third), "packed with --unsigned");
    const atlasThird = makeMod({
      manifest: baseManifest({ publisher: { ...thirdParty().publisher } }),
    });
    refused(() => packed(atlasThird, { unsigned: true }), "atlas.");
    const wrongTier = makeMod({ manifest: baseManifest({ id: "acme.toy" }) });
    refused(() => packed(wrongTier), "must start");
  });

  it.skipIf(process.platform === "win32")("refuses a symlink in the build output", () => {
    const mod = makeMod();
    symlinkSync("/etc/passwd", join(mod.modDir, "dist", "app", "link.js"));
    refused(
      () => pack.assemble({ modDir: mod.modDir, root: mod.root }),
      "isn't a plain file or folder",
    );
  });

  it("refuses a missing build, an empty one and a missing manifest", () => {
    const mod = makeMod();
    rmSync(join(mod.modDir, "dist"), { recursive: true });
    refused(() => pack.assemble({ modDir: mod.modDir, root: mod.root }), "build the mod first");
    const empty = makeMod({ app: {} });
    refused(() => pack.assemble({ modDir: empty.modDir, root: empty.root }), "holds no files");
    const noManifest = makeMod();
    rmSync(join(noManifest.modDir, "manifest.json"));
    refused(
      () => pack.assemble({ modDir: noManifest.modDir, root: noManifest.root }),
      "manifest.json is missing",
    );
  });

  it("refuses an output file whose name breaks the path rules", () => {
    const mod = makeMod({ app: { ...APP, "assets/bad name.js": "1" } });
    refused(() => packed(mod), "character");
  });

  it("refuses a manifest that names a file the build did not produce", () => {
    const mod = makeMod({ manifest: baseManifest({ entry: { page: "app/other.html" } }) });
    refused(() => packed(mod), "isn't one of its app/");
  });

  it("refuses a manifest that repeats a key", () => {
    const mod = makeMod();
    const text = readFileSync(join(mod.modDir, "manifest.json"), "utf8");
    writeFileSync(
      join(mod.modDir, "manifest.json"),
      text.replace('"version"', '"name": "Dup",\n  "version"'),
    );
    refused(() => packed(mod), "repeated");
    // And with --commit, which reads the manifest itself.
    refused(
      () => pack.assemble({ modDir: mod.modDir, root: mod.root, commit: COMMIT }),
      "repeated",
    );
  });
});

// ---- attach -------------------------------------------------------------------------------------

describe("attach", () => {
  const b64 = (n) => Buffer.alloc(n, 7).toString("base64");
  const comment = (over = {}) => {
    const f = {
      pmmod: "1",
      id: "atlas.toy",
      version: "1.2.3",
      tier: "first",
      author: "Atlas",
      sourceRepo: "itsAtlas-org/Personal-Manager",
      commit: COMMIT,
      api: "1.0-1.0",
      ...over,
    };
    return Object.entries(f)
      .map(([k, v]) => `${k}:${v}`)
      .join("\t");
  };
  const signature = (over, lines = {}) =>
    Buffer.from(
      `${lines.first ?? "untrusted comment: signature from a test"}\n${lines.sig ?? b64(74)}\n` +
        `trusted comment: ${comment(over)}\n${lines.global ?? b64(64)}\n`,
    );
  const archive = () => packed(makeMod(), { commit: COMMIT });

  it("adds the signature with the same writer: the result is sorted, verified, and still reproducible", () => {
    const sig = signature();
    const signed = pack.attach(archive(), sig);
    const read = pack.readZip(signed);
    expect(read.get("pmmod/signature.minisig").equals(sig)).toBe(true);
    const names = [...read.keys()];
    expect(names).toEqual([...names].sort());
    expect(pack.verifyArchive(signed).kind).toBe("signed");
    expect(pack.attach(archive(), sig).equals(signed)).toBe(true);
    // Every other entry is untouched.
    const before = pack.readZip(archive());
    for (const [name, data] of before) expect(read.get(name).equals(data)).toBe(true);
  });

  it("is the same file packing with a signature would have written", () => {
    const mod = makeMod();
    const files = pack.assemble({ modDir: mod.modDir, root: mod.root, commit: COMMIT });
    const direct = pack.buildArchive(files, { signature: signature() });
    expect(pack.attach(pack.buildArchive(files), signature()).equals(direct)).toBe(true);
  });

  it("refuses an archive packed without --commit, even with a signature that names the same placeholder", () => {
    const placeholder = "0".repeat(40);
    const bare = packed(makeMod());
    expect(pack.verifyArchive(bare).kind).toBe("pre-sign"); // a valid pre-sign file, which is the trap
    refused(() => pack.attach(bare, signature({ commit: placeholder })), "all-zero placeholder");
    refused(() => pack.attach(bare, signature()), "all-zero placeholder");
  });

  it("refuses an archive that is already signed, and a third-party file", () => {
    const signed = pack.attach(archive(), signature());
    refused(() => pack.attach(signed, signature()), "already signed");
    const third = packed(makeMod({ manifest: thirdParty() }), { unsigned: true });
    refused(
      () => pack.attach(third, signature({ id: "acme.toy", tier: "third" })),
      "can't be signed",
    );
  });

  it.each([
    ["an id that is not the manifest's", { id: "atlas.other" }, "isn't the one that was signed"],
    ["another version", { version: "9.9.9" }, "version"],
    ["another commit", { commit: "f".repeat(40) }, "commit"],
    ["another author", { author: "Someone" }, "author"],
    ["another source repo", { sourceRepo: "x/y" }, "source repo"],
    ["another API range", { api: "1.0-1.1" }, "API range"],
    ["a tier that is third", { tier: "third" }, "malformed (tier)"],
    ["another party's tier", { tier: "second" }, "manifest's tier isn't"],
    ["a control character in a field", { author: "A\u0001b" }, "control character"],
    ["not a mod's signature", { pmmod: "2" }, "not a mod's signature"],
    ["a commit that is not hex", { commit: "z".repeat(40) }, "40 lowercase hex"],
    ["an empty field", { author: "" }, "empty"],
  ])("refuses a trusted comment with %s", (_what, over, why) => {
    refused(() => pack.attach(archive(), signature(over)), why);
  });

  it.each([
    ["no trusted comment line", Buffer.from(`untrusted comment: x\n${b64(74)}\n${b64(64)}\n`)],
    ["a fifth line", Buffer.from(`${signature()}extra\n`)],
    ["a signature line of the wrong length", signature({}, { sig: b64(70) })],
    ["a global signature of the wrong length", signature({}, { global: b64(60) })],
    ["a signature line that is not base64", signature({}, { sig: "!!!!" })],
    [
      "a signature line with stray characters round valid base64",
      signature({}, { sig: `!${b64(74)}` }),
    ],
    ["a first line that is not the untrusted comment", signature({}, { first: "hello" })],
    [
      "a third line that is not the trusted comment",
      Buffer.from(
        signature()
          .toString()
          .replace(/^trusted comment: /m, "trusted remark: "),
      ),
    ],
    ["a file that is not UTF-8", Buffer.from([0xff, 0xfe])],
    [
      "a byte that is not UTF-8 inside an otherwise valid file",
      Buffer.concat([
        Buffer.from("untrusted comment: x"),
        Buffer.from([0xff]),
        signature().subarray("untrusted comment: ".length + 1),
      ]),
    ],
  ])("refuses a signature file with %s", (_what, sig) => {
    refused(() => pack.attach(archive(), sig), "its signature is malformed");
  });

  it("refuses a signature file over 4 KiB that is otherwise well formed", () => {
    const big = signature({}, { first: `untrusted comment: ${"x".repeat(5000)}` });
    refused(() => pack.attach(archive(), big), "its signature is too large");
  });

  it("takes a signature file with Windows line endings, as minisign's own reader does", () => {
    const crlf = Buffer.from(signature().toString().replaceAll("\n", "\r\n"));
    expect(pack.verifyArchive(pack.attach(archive(), crlf)).kind).toBe("signed");
  });

  it.each([
    ["an extra field", (c) => `${c}\textra:1`, "wrong number of fields"],
    ["a field short", (c) => c.split("\t").slice(0, 7).join("\t"), "wrong number of fields"],
    ["fields out of order", (c) => c.replace("\tid:", "\tname:"), "expected id"],
  ])("refuses a trusted comment with %s", (_what, edit, why) => {
    const sig = Buffer.from(
      signature()
        .toString()
        .replace(/^trusted comment: (.*)$/m, (_m, c) => `trusted comment: ${edit(c)}`),
    );
    refused(() => pack.attach(archive(), sig), why);
  });

  it("verify also catches a signed file whose comment disagrees with its manifest", () => {
    const mod = makeMod();
    const files = pack.assemble({ modDir: mod.modDir, root: mod.root, commit: COMMIT });
    refused(
      () => pack.buildArchive(files, { signature: signature({ version: "0.0.1" }) }),
      "isn't the one that was signed",
    );
  });
});

// ---- Verifying ----------------------------------------------------------------------------------

describe("verifyArchive", () => {
  const filesFor = (over = {}) => {
    const mod = makeMod(over);
    return pack.assemble({ modDir: mod.modDir, root: mod.root, commit: COMMIT });
  };
  const rebuilt = (files, tweak) => {
    const entries = new Map(files);
    tweak(entries);
    entries.set("pmmod/checksums.json", pack.checksumsFor(entries));
    return pack.writeZip(entries);
  };

  it("accepts what pack wrote and reports its kind and contents", () => {
    const result = pack.verifyArchive(pack.buildArchive(filesFor()));
    expect(result.kind).toBe("pre-sign");
    expect(result.manifest.id).toBe("atlas.toy");
    expect(result.files.size).toBe(6);
  });

  it("refuses a file that lists a checksum for something missing, or holds something unlisted", () => {
    const files = filesFor();
    const withExtra = new Map(files);
    const bytes = pack.writeZip(
      new Map([
        ...withExtra,
        ["pmmod/checksums.json", pack.checksumsFor(files)],
        ["app/extra.js", Buffer.from("1")],
      ]),
    );
    refused(() => pack.verifyArchive(bytes), "checksums don't list");
    const half = new Map(files);
    const sums = pack.checksumsFor(files);
    half.delete("app/assets/style.css");
    refused(
      () => pack.verifyArchive(pack.writeZip(new Map([...half, ["pmmod/checksums.json", sums]]))),
      "which it doesn't hold",
    );
  });

  it("refuses a file whose content is not what its checksum says", () => {
    const files = filesFor();
    const entries = new Map([...files, ["pmmod/checksums.json", pack.checksumsFor(files)]]);
    entries.set("app/assets/app.js", Buffer.from("(function(){var evil=1})();"));
    refused(() => pack.verifyArchive(pack.writeZip(entries)), "isn't the file its checksums list");
  });

  it("refuses no checksums, malformed checksums, unsorted ones, ones that list pmmod/, and a missing licence", () => {
    const files = filesFor();
    const noSums = pack.writeZip(new Map(files));
    refused(() => pack.verifyArchive(noSums), "no pmmod/checksums.json");
    const sumsOf = (mutate) => {
      const sums = JSON.parse(pack.checksumsFor(files));
      mutate(sums);
      return pack.writeZip(
        new Map([...files, ["pmmod/checksums.json", Buffer.from(JSON.stringify(sums))]]),
      );
    };
    refused(() => pack.verifyArchive(sumsOf((s) => (s.format = 2))), "unknown format");
    refused(() => pack.verifyArchive(sumsOf((s) => (s.x = 1))), "unexpected fields");
    refused(() => pack.verifyArchive(sumsOf((s) => s.files.reverse())), "not sorted");
    refused(() => pack.verifyArchive(sumsOf((s) => s.files.push(s.files[0]))), "not sorted");
    refused(
      () =>
        pack.verifyArchive(sumsOf((s) => (s.files[0].sha256 = s.files[0].sha256.toUpperCase()))),
      "sha256",
    );
    refused(
      () =>
        pack.verifyArchive(
          sumsOf((s) => s.files.push({ path: "pmmod/signature.minisig", sha256: "0".repeat(64) })),
        ),
      "can't be listed",
    );
    refused(
      () =>
        pack.verifyArchive(
          sumsOf((s) => (s.files = s.files.filter((f) => f.path !== "LICENCE.txt"))),
        ),
      "no LICENCE.txt",
    );
  });

  it("refuses a licence file that is not the AGPL", () => {
    const files = filesFor();
    refused(
      () =>
        pack.verifyArchive(rebuilt(files, (e) => e.set("LICENCE.txt", Buffer.from("MIT License")))),
      "isn't the GNU AGPL",
    );
    refused(
      () =>
        pack.verifyArchive(
          rebuilt(files, (e) =>
            e.set("LICENCE.txt", Buffer.from("GNU AFFERO GENERAL PUBLIC LICENSE\nVersion 2")),
          ),
        ),
      "isn't the GNU AGPL",
    );
  });

  it("refuses a manifest that is not UTF-8 or not JSON, or a signature that came without its checksums", () => {
    const files = filesFor();
    refused(
      () => pack.verifyArchive(rebuilt(files, (e) => e.set("manifest.json", Buffer.from([0xff])))),
      "not UTF-8",
    );
    refused(
      () => pack.verifyArchive(rebuilt(files, (e) => e.set("manifest.json", Buffer.from("{")))),
      "not valid JSON",
    );
    refused(
      () =>
        pack.verifyArchive(rebuilt(files, (e) => e.set("manifest.json", Buffer.from("\uFEFF{}")))),
      "not valid JSON",
    );
  });

  it("refuses an oversize manifest", () => {
    const files = filesFor();
    refused(
      () =>
        pack.verifyArchive(
          rebuilt(files, (e) => e.set("manifest.json", Buffer.alloc(pack.MANIFEST_CAP + 1, 0x20))),
        ),
      "too large",
    );
  });

  it("refuses oversize checksums", () => {
    const files = filesFor();
    const big = pack.writeZip(
      new Map([...files, ["pmmod/checksums.json", Buffer.alloc(pack.CHECKSUMS_CAP + 1, 0x20)]]),
    );
    refused(() => pack.verifyArchive(big), "too large");
  });

  it("refuses checksums of the wrong shape, one way at a time", () => {
    const files = filesFor();
    const withSums = (edit) => {
      const sums = JSON.parse(pack.checksumsFor(files));
      edit(sums);
      return pack.writeZip(
        new Map([...files, ["pmmod/checksums.json", Buffer.from(JSON.stringify(sums))]]),
      );
    };
    const cases = [
      ["files that isn't a list", (s) => (s.files = "x"), "files must be a list"],
      ["an item with an extra field", (s) => (s.files[0].extra = 1), "unexpected fields"],
      ["an item that isn't an object", (s) => (s.files[0] = "x"), "unexpected fields"],
      ["a path that isn't text", (s) => (s.files[0].path = 5), "not text"],
      ["a hash that isn't text", (s) => (s.files[0].sha256 = 5), "not text"],
      [
        "a path that breaks the path rules",
        (s) => (s.files[0].path = "app/../x"),
        "isn't allowed in a mod",
      ],
      ["a path listed twice in a row", (s) => s.files.splice(1, 0, s.files[0]), "not sorted"],
      ["not JSON", null, "malformed"],
    ];
    for (const [what, edit, why] of cases) {
      const bytes =
        edit === null
          ? pack.writeZip(new Map([...files, ["pmmod/checksums.json", Buffer.from("{")]]))
          : withSums(edit);
      expect(() => pack.verifyArchive(bytes), what).toThrow(why);
    }
  });

  it("refuses a licence file that has the title but not the version", () => {
    const files = filesFor();
    refused(
      () =>
        pack.verifyArchive(
          rebuilt(files, (e) =>
            e.set("LICENCE.txt", Buffer.from("GNU AFFERO GENERAL PUBLIC LICENSE\nVersion 2")),
          ),
        ),
      "isn't the GNU AGPL",
    );
    refused(
      () =>
        pack.verifyArchive(rebuilt(files, (e) => e.set("LICENCE.txt", Buffer.from("Version 3")))),
      "isn't the GNU AGPL",
    );
  });

  it("checksums list every file but the two pmmod/ ones, in path order", () => {
    const listed = JSON.parse(
      pack.checksumsFor(
        new Map([
          ["manifest.json", Buffer.from("m")],
          ["pmmod/signature.minisig", Buffer.from("s")],
          ["LICENCE.txt", Buffer.from("l")],
          ["pmmod/checksums.json", Buffer.from("c")],
          ["app/a.js", Buffer.from("a")],
        ]),
      ),
    ).files.map((f) => f.path);
    expect(listed).toEqual(["LICENCE.txt", "app/a.js", "manifest.json"]);
  });
});

// package.rs `a_run_of_more_than_four_combining_marks_is_refused`, the same cases: --verify must
// never accept a manifest Rust refuses.
describe("combining marks stacked on one letter", () => {
  const stacked = (base, marks) => base + marks.join("");
  const vietnamese = stacked("a", ["̣", "̂", "́"]);
  const hebrew = stacked("ב", ["ּ", "ָ", "֥", "֮"]);
  const thai = stacked("ก", ["ุ", "้", "์"]);
  const five = ["́", "̂", "̃", "̄", "̅"];
  const fault = (s) => pack.plainTextFault(s, 40, false, false);
  const files = new Set(["app/index.html"]);
  const check = (over) => pack.checkManifest(baseManifest(over), files);

  it("passes the real scripts that stack three or four marks", () => {
    for (const s of [vietnamese, hebrew, thai]) {
      expect(fault(s), s).toBeNull();
      expect(() => check({ name: s }), s).not.toThrow();
    }
    // the run is of marks, not of marked letters
    const many = `${hebrew}${hebrew}${vietnamese}${thai} ${hebrew}`;
    expect(fault(many)).toBeNull();
    expect(() => check({ name: many })).not.toThrow();
    expect(() =>
      check({
        description: `${hebrew}\n${hebrew}`,
        publisher: { ...baseManifest().publisher, author: hebrew },
      }),
    ).not.toThrow();
  });

  it("refuses five or more in a row, whatever the kind of mark and wherever they sit", () => {
    const mixedKinds = ["́", "⃝", "ः", "้", "ּ"];
    for (const [what, text] of [
      ["five accents", stacked("a", five)],
      ["mixed kinds (Mn, Me, Mc)", stacked("a", mixedKinds)],
      ["thirty-nine", stacked("M", Array(39).fill("́"))],
      ["after a good letter", hebrew + stacked("e", five)],
      ["at the end", `Map ${stacked("e", five)}`],
      ["on a mark-first name", five.join("")],
    ]) {
      expect(fault(text), what).toBe("has too many accent marks stacked on one letter");
      refused(() => check({ name: text }), "too many accent marks");
      refused(() => check({ description: `Line one.\n${text}` }), "too many accent marks");
      refused(
        () => check({ publisher: { ...baseManifest().publisher, author: text } }),
        "too many accent marks",
      );
    }
    refused(
      () => check({ permissions: [{ id: "ui.tab", why: stacked("a", five) }] }),
      "too many accent marks",
    );
  });

  it("counts a run, not the total: a letter between two runs of four passes", () => {
    const split = stacked("a", five.slice(0, 4)) + stacked("b", five.slice(0, 4));
    expect(fault(split)).toBeNull();
  });
});

describe("built-output rules", () => {
  const page = (inner, head = "") =>
    `<!doctype html><html><head>${head}</head><body>${inner}</body></html>`;
  const html = (text) => pack.checkBuiltFile("app/index.html", Buffer.from(text));

  it("accepts a page that is one external classic script and one stylesheet", () => {
    expect(() => html(PAGE)).not.toThrow();
    // A comment that names no tag, like the SPDX header a mod's index.html starts with, is fine.
    expect(() => html(`<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->${PAGE}`)).not.toThrow();
    expect(() => html(`${PAGE.replace("<script", "<SCRIPT")}`)).not.toThrow();
    expect(() => html(page("", '<script src="a.js">\n  </script>'))).not.toThrow();
    expect(() => html(page("<p>text with <b>markup</b></p>"))).not.toThrow();
    expect(() =>
      html(page("", '<script src="a.js" type="text/javascript"></script>')),
    ).not.toThrow();
  });

  it.each([
    ["an inline script", page("<script>alert(1)</script>"), "inline script"],
    ["an inline script with a src", page('<script src="a.js">alert(1)</script>'), "inline script"],
    ["a script with no src and no body", page("<script></script>"), "inline script"],
    ["a module script", page('<script type="module" src="a.js"></script>'), "type"],
    [
      "a module script, other case and quotes",
      page("<script TYPE='Module' src=a.js></script>"),
      "type",
    ],
    ["an import map", page('<script type="importmap" src="a.js"></script>'), "type"],
    ["an unclosed script", page('<script src="a.js">'), "unclosed script"],
    ["crossorigin on a script", page('<script src="a.js" crossorigin></script>'), "crossorigin"],
    [
      "crossorigin on a link",
      page("", '<link rel="stylesheet" href="a.css" crossorigin="anonymous">'),
      "crossorigin",
    ],
    ["an inline style element", page("<style>p{color:red}</style>"), "inline style element"],
    ["an inline style attribute", page('<p style="color:red">x</p>'), "inline style attribute"],
    ["an inline event handler", page('<img src="a.png" onerror="x()">'), "event handler"],
    [
      "a modulepreload link",
      page("", '<link rel="modulepreload" href="a.js">'),
      "preloads a module",
    ],
    ["a javascript: URL", page('<a href="javascript:alert(1)">x</a>'), "javascript: URL"],
  ])("refuses %s", (_what, text, why) => {
    refused(() => html(text), why);
  });

  // The frame's CSP is the control that holds; these are slip-catchers. They still must not be
  // fooled by markup a browser reads differently from a `<[^>]*>` pattern.
  it.each([
    ["a script from another host", page('<script src="https://evil.example/x.js"></script>')],
    ["a script from a protocol-relative URL", page('<script src="//evil.example/x.js"></script>')],
    ["a script from a backslash URL", page('<script src="\\\\evil.example/x.js"></script>')],
    ["a script from a data: URL", page('<script src="data:text/javascript,alert(1)"></script>')],
    ["a script from a blob: URL", page('<script src="blob:null/x"></script>')],
    ["a scheme in capitals", page('<script src="HTTPS://evil.example/x.js"></script>')],
    ["a scheme with a tab in it", page('<script src="ht\ttps://evil.example/x.js"></script>')],
    [
      "a scheme spelled with character references",
      page('<script src="https&#58;//evil.example"></script>'),
    ],
    ["a scheme with a named colon", page('<script src="https&colon;//evil.example"></script>')],
    ["a scheme in hex references", page('<script src="&#x68;ttps://evil.example"></script>')],
    [
      "a second src that is remote",
      page('<script src="a.js" src="https://evil.example/x.js"></script>'),
    ],
    ["an unquoted remote src", page("<script src=https://evil.example/x.js></script>")],
    [
      "a stylesheet from another host",
      page("", '<link rel="stylesheet" href="https://evil.example/a.css">'),
    ],
    ["an icon from a data: URL", page("", '<link rel="icon" href="data:image/png;base64,AA">')],
  ])("refuses %s", (_what, text) => {
    refused(() => html(text), "isn't one of the mod's own files");
  });

  it.each([
    [
      "an event handler after a > inside a quoted value",
      '<div title=">" onclick="x()">x</div>',
      "event handler",
    ],
    [
      "an event handler right after a closing quote",
      '<div class="a"onclick="x()">x</div>',
      "event handler",
    ],
    ["an event handler after a slash", '<div/onclick="x()">x</div>', "event handler"],
    ["an unquoted event handler", "<svg onload=x()></svg>", "event handler"],
    ["an event handler in capitals", '<div ONCLICK="x()">x</div>', "event handler"],
    [
      "an inline style after a > inside a quoted value",
      "<p title='>' style='color:red'>x</p>",
      "inline style attribute",
    ],
    ["crossorigin after a quoted >", '<link title=">" crossorigin>', "crossorigin"],
    [
      "a javascript: URL spelled with a character reference",
      '<a href="java&#115;cript:x()">x</a>',
      "javascript: URL",
    ],
    [
      "a javascript: URL with a newline in it",
      '<a href="java\nscript:x()">x</a>',
      "javascript: URL",
    ],
    [
      "a javascript: URL after leading spaces",
      '<a href="  javascript:x()">x</a>',
      "javascript: URL",
    ],
    [
      "a modulepreload among other rel words",
      '<link rel="preload modulepreload" href="a.js">',
      "preloads a module",
    ],
    ["an inline script after a quoted >", '<script data-x=">">alert(1)</script>', "inline script"],
    ["an inline script after a slash", "<script/data-x=1>alert(1)</script>", "inline script"],
  ])("refuses %s", (_what, inner, why) => {
    refused(() => html(page(inner)), why);
  });

  // There is NO comment handling: a stripper is what three review rounds kept finding holes in, and
  // the CSP (not this lint) is the guard. Whatever fake comment opener precedes it, an inline
  // script is seen.
  it.each([
    ["a comment-looking prefix `<!-->`", "<!--><script>alert(1)</script><!-- end -->"],
    ["a comment-looking prefix `<!--->`", "<!---><script>alert(1)</script><!-- end -->"],
    ["a comment closed by `--!>`", "<!-- x --!><script>alert(1)</script><!-- end -->"],
    ["an end tag with an attribute", '<p>x</p x="<!--"><script>alert(1)</script><!-- -->'],
    ["a bogus comment `<!x`", "<!x <!-- ><script>alert(1)</script> -->"],
    ["a bogus comment `<?x`", "<?x <!-- ><script>alert(1)</script> -->"],
    [
      "a DOCTYPE with a quoted identifier",
      '<!DOCTYPE html PUBLIC "<!--"><script>alert(1)</script><!-- -->',
    ],
    ["a bogus comment `</ `", "</ <!-- ><script>alert(1)</script> -->"],
    ["a title holding a comment opener", "<title><!--</title><script>alert(1)</script>-->"],
    [
      "a textarea holding a comment opener",
      "<textarea><!--</textarea><script>alert(1)</script>-->",
    ],
    [
      "a noscript holding a comment opener",
      "<NoScript><!--</noscript><script>alert(1)</script>-->",
    ],
    ["an unclosed title", "<title><!--<script>alert(1)</script>"],
    [
      "a quoted attribute value holding a comment opener",
      '<a title="<!--"><script>alert(1)</script><a title="-->">',
    ],
  ])("refuses an inline script behind %s", (_what, text) => {
    refused(() => html(text), "inline script");
    refused(() => html(page(text)), "inline script");
  });

  it("refuses a tag named only inside a comment or a quoted value, on purpose", () => {
    // The rules read the raw text: a comment that mentions the markup is refused too, and the
    // message says to remove the comment. Vite's built output carries no such comment.
    for (const [text, why] of [
      ["<!-- <script>x</script> --><p>ok</p>", "inline script"],
      ["<!-- the <script> tag goes last --><p>ok</p>", "remove a comment"],
      ["<!-- <style>p{}</style> --><p>ok</p>", "inline style element"],
      ["<!-- crossorigin --><p>ok</p>", "crossorigin"],
      ["<!-- type=module --><p>ok</p>", "type=module"],
      ["<title>a</title><!-- <script>x</script> -->", "inline script"],
      ['<a title="<script>">x</a>', "remove a comment"],
    ]) {
      refused(() => html(page(text)), why);
    }
    expect(() => html(page("<!-- <title> --><p>ok</p>"))).not.toThrow();
  });

  it("refuses a script however the tag is spelled", () => {
    for (const [text, why] of [
      ["<SCRIPT>alert(1)</SCRIPT>", "inline script"],
      ["<ScRiPt\n>alert(1)</sCrIpT>", "inline script"],
      ['<script src="a.js">alert(1)</script>', "inline script"],
      ['<script src="a.js"> x </script>', "inline script"],
      ['<script src="a.js"></script x>', "inline script"],
      ['<script src="a.js"></ script>', "unclosed script"],
      ["<script/src=a.js>x</script>", "inline script"],
      ["<scripts>x</scripts>", "isn't a script tag"],
      ["<script-x></script-x>", "isn't a script tag"],
      ['<script src="a.js"><script src="b.js"></script></script>', "inline script"],
      ['<script src="a.js"></script><script>alert(1)</script>', "inline script"],
      ["<script src=a.js type = 'module'></script>", "type"],
      ['<script type="MODULE" src=a.js></script>', "type"],
      ['<p data-x="type = module"></p>', "type=module"],
      ['<link rel="stylesheet" href="a.css" CrossOrigin>', "crossorigin"],
      ["<STYLE>p{}</STYLE>", "inline style element"],
      ["<style\n>p{}</style>", "inline style element"],
    ]) {
      refused(() => html(page(text)), why);
    }
    refused(() => html("<p>ok</p><script"), "never ends");
    refused(() => html("<p>ok</p><script src=a.js"), "never ends");
  });

  // HTML whitespace is tab, LF, FF, CR and space. A character JS's `\s` also matches but a browser
  // does not (no-break space, VT, U+2028, U+FEFF) before a quote makes the VALUE unquoted for the
  // browser, so a tokenizer that skips it reads a different end of the tag.
  it.each([
    ["no-break space", " "],
    ["vertical tab", "\u000b"],
    ["U+2028", " "],
    ["U+FEFF", "﻿"],
  ])("refuses markup hidden behind a %s before a quote", (_name, ch) => {
    for (const [text, why] of [
      [`<script a=${ch}'>alert(1)//' src=x></script>`, "inline script"],
      [`<script src=x a=${ch}'></script><script>alert(1)</script>' ></script>`, "inline script"],
      [`<div a=${ch}' style=x>'>`, "inline style attribute"],
      [`<div a=${ch}' onclick=alert(1)>'>`, "event handler"],
      [`<script${ch}src=x></script>`, "isn't a script tag"],
    ]) {
      refused(() => html(page(text)), why);
    }
  });

  it("still accepts a no-break space where it is only text or inside a quoted value", () => {
    expect(() => html(page('<p title="a b">x y</p>'))).not.toThrow();
    expect(() => html(page('<script src="a.js" data-x=" "></script>'))).not.toThrow();
  });

  it("refuses markup a character reference hides from the raw-text rules", () => {
    refused(
      () => html(page('<iframe srcdoc="&lt;script&gt;alert(1)&lt;/script&gt;"></iframe>')),
      "srcdoc",
    );
    refused(() => html(page("<IFRAME SrcDoc=x></IFRAME>")), "srcdoc");
    refused(
      () => html(page("", '<link rel="module&#112;reload" href="a.js">')),
      "preloads a module",
    );
    refused(
      () => html(page("", '<link rel="module&#x70;reload" href="a.js">')),
      "preloads a module",
    );
    // a lookalike in plain text is not an attribute
    expect(() => html(page("<p>srcdoc is not an attribute here</p>"))).not.toThrow();
  });

  it("refuses a tag the text ends inside", () => {
    refused(() => html("<p>ok</p><div onclick="), "never ends");
    refused(() => html("<img src='a.png"), "never ends");
  });

  it("reads every candidate tag from its own start (a fake tag in a comment, title or textarea hides nothing)", () => {
    // `<a b='` inside the first comment opens a quote that only the second comment closes; a
    // tokenizer that skips to each tag's end never examines the real tag in between.
    const hide = (real) => page(`<!-- <a b=' -->${real}<!-- ' -->`);
    refused(() => html(hide("<img src=x style=x>")), "inline style attribute");
    refused(() => html(hide("<img src=x onerror=alert(1)>")), "inline event handler");
    refused(() => html(hide("<img src=x style=x onerror=alert(1)>")), "inline style attribute");
    refused(() => html(hide("<a href='javascript:alert(1)'>x</a>")), "javascript: URL");
    refused(() => html(hide("<iframe srcdoc=x></iframe>")), "srcdoc");
    refused(() => html(hide("<link rel=modulepreload href=a.js>")), "preloads a module");
    refused(
      () => html(hide("<script src=https://evil.example/x.js></script>")),
      "isn't one of the mod's own files",
    );
    refused(
      () => html(page("<textarea><a b='</textarea><img style=x><textarea>'</textarea>")),
      "inline style attribute",
    );
    refused(
      () => html(page("<title><a b='</title><img style=x onerror=1><title>'</title>")),
      "inline style attribute",
    );
    // control: the same shape with nothing hidden in it passes
    expect(() => html(hide("<img src=x alt=y>"))).not.toThrow();
  });

  it("refuses markup whose overlapping tag reads would run long, and quickly", () => {
    // Every read runs on to the far end of the file, so the reads would cost quadratic time.
    for (const body of ["<a b='".repeat(20000) + "'>", "<a ".repeat(20000) + ">"]) {
      const started = performance.now();
      refused(() => html(page(body)), "too tangled");
      expect(performance.now() - started).toBeLessThan(2000);
    }
    // A long, ordinary page (each tag ends at its own >) is nowhere near the budget.
    expect(() => html(page("<p class=a>x</p>".repeat(20000)))).not.toThrow();
  });

  it("still accepts local references, navigation links and a > inside a quoted value", () => {
    for (const src of ["./a.js", "a.js", "/a.js", "../a.js", "assets/a.js?x=1&y=2", "a.js#top"]) {
      expect(() => html(page("", `<script src="${src}"></script>`)), src).not.toThrow();
    }
    expect(() => html(page("", '<link rel="stylesheet" href="./a.css">'))).not.toThrow();
    // an anchor is navigation, not something the page loads
    expect(() => html(page('<a href="https://example.com/docs">docs</a>'))).not.toThrow();
    expect(() => html(page('<p title=">" class="x">a &lt; b and a < b</p>'))).not.toThrow();
    expect(() => html(page('<img alt="a>b" src="./a.png">'))).not.toThrow();
  });

  it("turns text that isn't UTF-8 into a refusal, not a TypeError", () => {
    const bad = Buffer.from([0xff, 0xfe, 0x41]);
    for (const path of ["app/index.html", "app/a.css", "app/a.js", "app/b.htm"]) {
      refused(() => pack.checkBuiltFile(path, bad), "isn't UTF-8 text");
    }
    // and through the verifier, which is where a packed file meets it
    const mod = makeMod();
    const files = pack.assemble({ modDir: mod.modDir, root: mod.root });
    files.set("app/other.html", bad);
    refused(() => pack.buildArchive(files), "isn't UTF-8 text");
  });

  it("refuses a LICENCE.txt that isn't UTF-8 as the Licence refusal, like package.rs", () => {
    const mod = makeMod();
    const files = pack.assemble({ modDir: mod.modDir, root: mod.root });
    files.set("LICENCE.txt", Buffer.from([0xff, 0xfe, 0x41]));
    refused(() => pack.buildArchive(files), "isn't the GNU AGPL, version 3");
  });

  it("refuses a stylesheet with a data: URL", () => {
    refused(
      () =>
        pack.checkBuiltFile(
          "app/a.css",
          Buffer.from("a{background:url(data:image/png;base64,AA)}"),
        ),
      "data: URL",
    );
    refused(
      () => pack.checkBuiltFile("app/a.css", Buffer.from('a{background:url( "data:x" )}')),
      "data: URL",
    );
    expect(() =>
      pack.checkBuiltFile("app/a.css", Buffer.from("a{background:url(./a.png)}")),
    ).not.toThrow();
  });

  it("refuses a script file that uses module syntax, and accepts a classic one", () => {
    refused(
      () => pack.checkBuiltFile("app/a.js", Buffer.from("import x from './y.js'; x()")),
      "classic script",
    );
    refused(
      () => pack.checkBuiltFile("app/a.js", Buffer.from("export const a = 1")),
      "classic script",
    );
    refused(() => pack.checkBuiltFile("app/a.js", Buffer.from("(function(){")), "classic script");
    expect(() =>
      pack.checkBuiltFile("app/a.js", Buffer.from("(function(){ var a = 1; return a })()")),
    ).not.toThrow();
  });

  it("is applied to every file under app/ when a package is verified", () => {
    const mod = makeMod({ app: { ...APP, "other.html": page("<script>1</script>") } });
    refused(() => packed(mod), "inline script");
    const css = makeMod({ app: { ...APP, "assets/style.css": "a{background:url(data:x)}" } });
    refused(() => packed(css), "data: URL");
    const js = makeMod({ app: { ...APP, "assets/app.js": "export {}" } });
    refused(() => packed(js), "classic script");
  });
});

// ---- The command line ---------------------------------------------------------------------------

describe("run (the command line)", () => {
  const logs = () => {
    const log = vi.spyOn(console, "log").mockImplementation(() => {});
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    return {
      log,
      err,
      restore: () => {
        log.mockRestore();
        err.mockRestore();
      },
    };
  };

  it("packs to dist/<id>-<version>.pmmod, prints `sha256  file`, and --verify reads it back", () => {
    const mod = makeMod();
    const out = logs();
    try {
      pack.run([mod.modDir, "--commit", COMMIT], mod.root);
      const file = join(mod.modDir, "dist", "atlas.toy-1.2.3.pmmod");
      expect(out.log).toHaveBeenCalledWith(`${sha(readFileSync(file))}  ${file}`);
      pack.run(["--verify", file]);
      expect(out.log.mock.calls.at(-1)[0]).toMatch(
        /well-formed pre-sign package of atlas\.toy 1\.2\.3 \(6 files\)/,
      );
    } finally {
      out.restore();
    }
  });

  // The signature vouches for the commit in the trusted comment, so a package for signing that still
  // holds the all-zero placeholder would be signed as built from a commit that does not exist.
  it("refuses to pack for signing while the commit is the placeholder, and writes nothing", () => {
    const mod = makeMod();
    const out = logs();
    try {
      refused(() => pack.run([mod.modDir], mod.root), "all-zero placeholder");
      const file = join(mod.modDir, "dist", "atlas.toy-1.2.3.pmmod");
      expect(existsSync(file)).toBe(false);
      pack.run([mod.modDir, "--commit", COMMIT], mod.root);
      expect(existsSync(file)).toBe(true);
    } finally {
      out.restore();
    }
  });

  it("lets a developer's --unsigned file and a --stage keep the placeholder", () => {
    const third = makeMod({ manifest: thirdParty() });
    const out = logs();
    try {
      pack.run([third.modDir, "--unsigned"], third.root);
      expect(existsSync(join(third.modDir, "dist", "acme.toy-1.2.3.pmmod"))).toBe(true);
      const first = makeMod();
      pack.run([first.modDir, "--stage"], first.root);
      expect(existsSync(join(first.modDir, "dist", "manifest.json"))).toBe(true);
    } finally {
      out.restore();
    }
  });

  it("--stage writes the three package files beside app/, without an archive", () => {
    const mod = makeMod();
    const out = logs();
    try {
      pack.run([mod.modDir, "--stage", "--commit", COMMIT], mod.root);
    } finally {
      out.restore();
    }
    const dist = join(mod.modDir, "dist");
    expect(JSON.parse(readFileSync(join(dist, "manifest.json"))).publisher.commit).toBe(COMMIT);
    expect(readFileSync(join(dist, "LICENCE.txt"), "utf8")).toBe(AGPL);
    expect(readFileSync(join(dist, "NOTICE.txt"), "utf8")).toContain("bundles no npm packages");
    expect(existsSync(join(dist, "atlas.toy-1.2.3.pmmod"))).toBe(false);
  });

  it("--out, attach and its round trip", () => {
    const mod = makeMod();
    const out = logs();
    try {
      const file = join(mod.root, "out", "a.pmmod");
      pack.run([mod.modDir, "--commit", COMMIT, "--out", file], mod.root);
      const sigFile = join(mod.root, "a.minisig");
      const sig = [
        "untrusted comment: t",
        Buffer.alloc(74, 7).toString("base64"),
        `trusted comment: pmmod:1\tid:atlas.toy\tversion:1.2.3\ttier:first\tauthor:Atlas\tsourceRepo:itsAtlas-org/Personal-Manager\tcommit:${COMMIT}\tapi:1.0-1.0`,
        Buffer.alloc(64, 7).toString("base64"),
        "",
      ].join("\n");
      writeFileSync(sigFile, sig);
      const signed = join(mod.root, "out", "signed.pmmod");
      pack.run(["attach", file, sigFile, "--out", signed]);
      pack.run(["--verify", signed]);
      expect(out.log.mock.calls.at(-1)[0]).toContain("well-formed signed package");
    } finally {
      out.restore();
    }
  });

  it.each([
    [[], "usage"],
    [["--nope"], "unknown option"],
    [["a", "b"], "usage"],
    [["--verify"], "usage"],
    [["--commit"], "needs a value"],
    [["mod", "--verify", "x"], "usage"],
    [["--verify", "x", "--stage"], "doesn't go with this command"],
    [["mod", "--stage", "--unsigned"], "doesn't go with this command"],
    [["attach", "a", "b"], "usage"],
    [["attach", "a", "b", "--out", "c", "--commit", COMMIT], "doesn't go with this command"],
  ])("refuses the arguments %j", (argv, why) => {
    refused(() => pack.run(argv), why);
  });
});

// ---- Golden file --------------------------------------------------------------------------------

describe("the golden archive package.rs verifies", () => {
  const GOLDEN = join(repoRoot, "tests/mod-fixtures/golden/hello.pmmod.b64");

  /** Fixed inputs. Nothing here may change without regenerating the golden (and re-running cargo test). */
  function goldenBytes() {
    const manifest = thirdParty({
      id: "acme.golden",
      name: "Golden",
      description: "A tiny package whose bytes the Rust verifier is tested against.",
      version: "1.0.0",
      permissions: [{ id: "ui.tab", why: "Shows its tab." }],
      contributions: { "nav.tab": { label: "Golden" } },
    });
    const mod = makeMod({ manifest, packages: [pkg("golden-dep", "MIT")] });
    return packed(mod, { unsigned: true, commit: COMMIT });
  }

  it("matches the committed file byte for byte (PM_UPDATE_GOLDEN=1 rewrites it)", () => {
    const text = `${goldenBytes()
      .toString("base64")
      .replace(/(.{76})/g, "$1\n")}\n`;
    if (process.env.PM_UPDATE_GOLDEN === "1") {
      mkdirSync(dirname(GOLDEN), { recursive: true });
      writeFileSync(GOLDEN, text);
    }
    expect(readFileSync(GOLDEN, "utf8")).toBe(text);
  });

  it("is an unsigned third-party package with the stamped commit", () => {
    const result = pack.verifyArchive(
      Buffer.from(readFileSync(GOLDEN, "utf8").replace(/\s/g, ""), "base64"),
    );
    expect(result.kind).toBe("unsigned");
    expect(result.manifest.commit).toBe(COMMIT);
    expect(result.manifest.id).toBe("acme.golden");
  });
});
