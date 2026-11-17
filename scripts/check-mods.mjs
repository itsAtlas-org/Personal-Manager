// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The mods gate (Mods spec section 7; INVARIANTS I-25). First-party mod source lives in this repo
// under mods/<id>/, is built into its own signed `.pmmod`, and reaches a user only through the Mods
// tab. This is what keeps the folder that way, and it reads SOURCE ONLY: the hygiene job that runs
// it has no build and no node_modules, so what a built bundle may contain is `pack-mod --verify`'s
// job (it runs inside `just build-mods`).
//
//   For every folder under mods/ (and every tests/mod-fixtures/<id>/ that has a manifest):
//     * manifest.json passes the rules src-tauri/src/mods/package.rs applies (unknown fields, the id
//       grammar, licence, permission ids, internet iff internetHosts, caps, every named file there),
//       by the one mirror of them that exists, pack-mod.mjs `checkManifest`; and each permission id
//       is also checked against the Rust list itself, which is read from package.rs by pattern;
//     * the id equals the folder name; under mods/ it starts `atlas.` with tier `first`, and a
//       fixture is never an `atlas.` id (a test mod must not look first-party);
//     * `apiVersion` is one this PM serves (policy.rs MOD_API_VERSION);
//     * `publisher.commit` is the all-zero placeholder: a manifest cannot hold the hash of the
//       commit that contains it, so `pack-mod --commit` stamps the real one into the package;
//     * CHANGELOG.md's top entry is the manifest's version and says something (D12);
//     * a folder with a Worker has its own tsconfig (lib WebWorker, no DOM), is excluded from the DOM
//       project in mods/tsconfig.json and has a `tsc -p` line in the justfile, so a Worker cannot
//       drop out of every type-check without a sound (three places edited by hand until now);
//     * nothing under mods/ is a symbolic link (the D12 rule trusts "inside mods/" to mean "not PM").
//     * under mods/, the publisher is pinned to what Q3 of the PR B plan recommends: author "Atlas"
//       and sourceRepo "itsAtlas-org/Personal-Manager". A later package whose tier, author or
//       sourceRepo differs is refused as an update by PM (policy.rs), which can only be fixed by
//       every user uninstalling and reinstalling, so the edit is refused here first.
//   Across the repo:
//     * no source that a tauri config copies into the installer, merged the way Tauri merges them
//       (RFC 7386 JSON Merge Patch), is under mods/, is a glob that matches it, or is a folder that
//       contains it: `bundle.resources`, `bundle.linux.{deb,rpm,appimage}.files`,
//       `bundle.macOS.files`, `bundle.externalBin` and the path-valued keys (icon, licenseFile,
//       frameworks, NSIS/WiX templates, hooks and fragments, deb/rpm scripts: `bundleSources`). A mod inside the installer
//       would skip the verifier, the ledger and the blacklist, and ship to everyone;
//     * nothing under src/ (and not the root index.html) imports from the top-level mods/ folder.
//       Each specifier is RESOLVED against the importing file: a substring test would refuse src/components/mods/, which is
//       PM's own.
//     * no Rust file under src-tauri/ (src/, build.rs, any other crate) embeds a file of the top-level
//       mods/ folder with `include_str!`, `include_bytes!` or `include!`: a string literal (resolved
//       from the folder of the file holding the macro, as rustc does) or `concat!` of literals led by
//       `env!("CARGO_MANIFEST_DIR")` (resolved from the crate's folder). A form it cannot resolve that
//       mentions mods/ fails closed. Test code is exempt: `tests.rs`, `*_tests.rs`, a `tests/` folder,
//       and an item behind `#[cfg(test)]` exactly (any other test-only use goes in
//       RUST_INCLUDE_ALLOWED by file path). A build.rs naming mods/ and using OUT_DIR is refused too.
//
//   --base <ref>: each mods/<id>/ that differs from the merge-base with <ref> (working tree
//       included) needs a manifest version greater than the one at <ref>, and the matching top
//       CHANGELOG entry. A folder new at <ref> is exempt. check-version-lockstep.mjs runs this same
//       check whenever it is given --base, and uses `changedPaths` / `isModsOnly` to decide whether
//       PM's own version must move (D12: only when something outside the mod folders changed; a
//       shared file directly under mods/ such as vite.config.ts counts as outside, because it
//       decides the bytes of every mod and no single mod folder would owe a bump for it).
//
// NOT repeated here, because the Rust tests already hold them and a second copy could only drift:
//     * "the two mod key ids differ" and "neither is the updater's": keys.rs
//       `the_embedded_keys_decode_to_their_recorded_ids` and `the_mod_keys_are_not_the_updater_key`;
//     * "every dispatch-table method and event has a permission row": api/table.rs
//       `every_handler_and_event_has_a_row_and_every_row_its_handler`.
//
// Prints what it inspected; a count of zero is a failure, never a pass. Pure Node built-ins plus the
// repo's own scripts, no dependencies.
//
// Usage:
//   node scripts/check-mods.mjs
//   node scripts/check-mods.mjs --base origin/main

import { execFileSync } from "node:child_process";
import { existsSync, lstatSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, posix } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  MANIFEST_CAP,
  PERMISSIONS as PACK_MOD_PERMISSIONS,
  PackError,
  checkManifest,
  checkTier,
  isReservedId,
  parseStrictJson,
} from "./pack-mod.mjs";
import { semverCmp } from "./semver.mjs";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

export const MODS_DIR = "mods";
export const FIXTURES_DIR = "tests/mod-fixtures";
export const PLACEHOLDER_COMMIT = "0".repeat(40);
// The publisher pin for first-party mods (PR B plan Q3, recommended answer).
export const FIRST_PARTY_AUTHOR = "Atlas";
export const FIRST_PARTY_REPO = "itsAtlas-org/Personal-Manager";

const PACKAGE_RS = "src-tauri/src/mods/package.rs";
const POLICY_RS = "src-tauri/src/mods/policy.rs";
const TAURI_DIR = "src-tauri";
const POSIX_ROOT = (root) => root.replace(/\\/g, "/").replace(/\/+$/, "");

const readText = (root, rel) => readFileSync(join(root, rel), "utf8").replace(/\r\n/g, "\n");
const isObject = (v) => v !== null && typeof v === "object" && !Array.isArray(v);

// ---- Walking --------------------------------------------------------------------------------------

/**
 * Every file under `dir` (repo-relative, posix), plus the symbolic links met on the way, which are
 * recorded and never followed. `skip(relativeToDir)` names a directory to leave alone.
 */
function walk(root, dir, skip = () => false) {
  const files = [];
  const links = [];
  const visit = (rel) => {
    for (const ent of readdirSync(join(root, rel), { withFileTypes: true })) {
      const path = `${rel}/${ent.name}`;
      if (ent.isSymbolicLink()) links.push(path);
      else if (ent.isDirectory()) {
        if (!skip(path.slice(dir.length + 1))) visit(path);
      } else if (ent.isFile()) files.push(path);
    }
  };
  if (existsSync(join(root, dir))) visit(dir);
  return { files, links };
}

const isDir = (root, rel) =>
  existsSync(join(root, rel)) && lstatSync(join(root, rel)).isDirectory();

// ---- What Rust says -------------------------------------------------------------------------------

/**
 * The permission ids and the served API range, read out of the Rust source by pattern. They are the
 * truth: the dispatcher serves them, so a node-side copy can only agree with them or be wrong. A
 * constant that is renamed or reshaped makes this throw, which fails the gate loudly instead of
 * letting every manifest through against an empty list.
 */
export function rustConstants(root) {
  const pkg = readText(root, PACKAGE_RS);
  const policy = readText(root, POLICY_RS);
  const perms = /pub const PERMISSIONS: &\[&str\] = &\[([^\]]*)\];/.exec(pkg);
  const version = /pub const MOD_API_VERSION: \(u16, u16\) = \((\d+), (\d+)\);/.exec(policy);
  if (!perms || !version) {
    throw new Error(
      `could not read PERMISSIONS from ${PACKAGE_RS} or MOD_API_VERSION ` +
        `from ${POLICY_RS}: a constant was renamed or reshaped, so this gate's pattern needs updating`,
    );
  }
  const permissions = [...perms[1].replace(/\/\/[^\n]*/g, "").matchAll(/"([^"]*)"/g)].map(
    (m) => m[1],
  );
  if (permissions.length === 0) throw new Error(`${PACKAGE_RS} PERMISSIONS read as empty`);
  return {
    permissions,
    api: [Number(version[1]), Number(version[2])],
  };
}

// ---- One mod folder -------------------------------------------------------------------------------

/** The top entry of a mod's CHANGELOG.md: the version its `## ` heading names, and whether it has a body. */
export function changelogTop(text) {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const at = lines.findIndex((l) => /^## /.test(l));
  if (at < 0) return null;
  const heading = /^## (\S+)(?:\s.*)?$/.exec(lines[at]);
  let end = lines.findIndex((l, i) => i > at && /^## /.test(l));
  if (end < 0) end = lines.length;
  return {
    version: heading ? heading[1] : null,
    hasBody: lines.slice(at + 1, end).some((l) => l.trim() !== ""),
  };
}

/** JSON with comments and trailing commas, which is what a tsconfig is. */
function parseJsonc(text) {
  let out = "";
  let i = 0;
  while (i < text.length) {
    const c = text[i];
    if (c === '"') {
      let j = i + 1;
      while (j < text.length && text[j] !== '"') j += text[j] === "\\" ? 2 : 1;
      out += text.slice(i, j + 1);
      i = j + 1;
    } else if (c === "/" && text[i + 1] === "/") {
      while (i < text.length && text[i] !== "\n") i++;
    } else if (c === "/" && text[i + 1] === "*") {
      const end = text.indexOf("*/", i + 2);
      i = end < 0 ? text.length : end + 2;
    } else {
      out += c;
      i++;
    }
  }
  return JSON.parse(out.replace(/,(\s*[}\]])/g, "$1"));
}

/** The folders a mod's source loads a Worker from: `import w from "inline-worker:<path>"`. */
function workerFolders(root, sourceFiles) {
  const found = new Map();
  for (const file of sourceFiles) {
    if (!/\.(m?[jt]sx?|cjs|cts)$/.test(file)) continue;
    const text = readText(root, file);
    for (const m of text.matchAll(/(["'])inline-worker:([^"'\n]+)\1/g)) {
      const target = posix.normalize(posix.join(posix.dirname(file), m[2]));
      found.set(posix.dirname(target), file);
    }
  }
  return found;
}

/**
 * Every problem with one mod folder. `kind` is "mod" (under mods/: first party) or "fixture" (under
 * tests/mod-fixtures: somebody else's mod, which the toolchain tests build).
 */
function checkModFolder(root, dir, kind, rust) {
  const problems = [];
  const bad = (msg) => problems.push(`${dir}: ${msg}`);
  const folder = dir.slice(dir.lastIndexOf("/") + 1);
  const manifestPath = join(root, dir, "manifest.json");
  if (!existsSync(manifestPath)) {
    bad("has no manifest.json (every folder under mods/ is a mod)");
    return problems;
  }
  const bytes = readFileSync(manifestPath);
  if (bytes.length > MANIFEST_CAP) {
    bad("manifest.json is too large");
    return problems;
  }
  let raw;
  try {
    raw = parseStrictJson(new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes));
  } catch (e) {
    bad(`manifest.json: ${e instanceof PackError ? e.message : "is not valid UTF-8 text"}`);
    return problems;
  }

  // The manifest names files as `app/<x>`; the build puts src/<x> there (the Vite root is src/).
  const sources = walk(
    root,
    `${dir}/src`,
    (rel) => rel === "node_modules" || rel.endsWith("/node_modules"),
  );
  const appFiles = new Set(sources.files.map((f) => `app/${f.slice(dir.length + "/src/".length)}`));
  let m;
  try {
    m = checkManifest(raw, appFiles);
  } catch (e) {
    if (!(e instanceof PackError)) throw e;
    bad(e.message);
    return problems;
  }
  if (m.id !== folder) bad(`the id is ${m.id} but the folder is ${folder}: they must be equal`);
  if (kind === "mod") {
    if (!isReservedId(m.id) || m.tier !== "first") {
      bad('a mod under mods/ is an Atlas mod: its id starts "atlas." and its tier is "first"');
    }
    for (const [what, got, want] of [
      ["author", m.author, FIRST_PARTY_AUTHOR],
      ["sourceRepo", m.sourceRepo, FIRST_PARTY_REPO],
    ]) {
      if (got !== want) {
        bad(
          `publisher.${what} is ${JSON.stringify(got)} but a first-party mod's is ${JSON.stringify(want)}: ` +
            `PM refuses an update whose publisher differs, and an installed copy can then only be ` +
            `removed and installed again`,
        );
      }
    }
  } else {
    if (isReservedId(m.id))
      bad('a test fixture may not take an "atlas." id (it must not look first party)');
    try {
      checkTier(m.id, m.tier);
    } catch (e) {
      if (!(e instanceof PackError)) throw e;
      bad(e.message);
    }
  }
  if (m.commit !== PLACEHOLDER_COMMIT) {
    bad(
      "publisher.commit must be the all-zero placeholder in the source tree: a manifest cannot " +
        "hold the hash of the commit that contains it (pack-mod --commit stamps the real one)",
    );
  }
  for (const p of Array.isArray(raw.permissions) ? raw.permissions : []) {
    if (!rust.permissions.includes(p.id)) {
      bad(`permission ${JSON.stringify(p.id)} is not one of package.rs's PERMISSIONS`);
    }
  }
  const target = m.api.split("-")[1].split(".").map(Number);
  const cmp = (a, b) => (a[0] - b[0] !== 0 ? a[0] - b[0] : a[1] - b[1]);
  // checkManifest has already held min <= target, so the target alone bounds both from above.
  if (cmp(target, rust.api) > 0 || target[0] < rust.api[0]) {
    bad(
      `apiVersion ${m.api} is outside what this PM serves (${rust.api.join(".")}): a mod in this ` +
        `repo is built for exactly the API this PM has`,
    );
  }
  for (const link of walk(root, dir, (rel) => rel === "dist" || rel === ".build").links) {
    bad(`${link} is a symbolic link: nothing under a mod folder may be one`);
  }
  if (kind === "mod") {
    problems.push(...changelogProblems(root, dir, m.version));
    problems.push(...workerProblems(root, dir, sources.files));
  }
  return problems;
}

/** CHANGELOG.md: present, and its top entry is `version` and is not empty. */
function changelogProblems(root, dir, version) {
  const path = join(root, dir, "CHANGELOG.md");
  if (!existsSync(path))
    return [`${dir}: has no CHANGELOG.md (D12: a mod's own history carries its bumps)`];
  const top = changelogTop(readFileSync(path, "utf8"));
  if (!top) return [`${dir}/CHANGELOG.md: has no "## <version>" entry`];
  const out = [];
  if (top.version !== version) {
    out.push(
      `${dir}/CHANGELOG.md: the top entry is ${JSON.stringify(top.version)} but manifest.json says ` +
        `${version}: add an entry for ${version} at the top`,
    );
  }
  if (!top.hasBody) out.push(`${dir}/CHANGELOG.md: the top entry (${top.version}) says nothing`);
  return out;
}

/** A Worker folder is its own TypeScript project, left out of the DOM one, and has a `tsc` line. */
function workerProblems(root, dir, sourceFiles) {
  const out = [];
  const srcRoot = `${dir}/src`;
  const justfile = existsSync(join(root, "justfile")) ? readText(root, "justfile") : "";
  let domExclude = null;
  try {
    domExclude = parseJsonc(readText(root, `${MODS_DIR}/tsconfig.json`)).exclude;
  } catch {
    // reported below, once per worker folder
  }
  for (const [wdir, importer] of workerFolders(root, sourceFiles)) {
    const bad = (msg) => out.push(`${importer}: its Worker folder ${wdir} ${msg}`);
    if (!wdir.startsWith(`${srcRoot}/`)) {
      bad("must be a folder of its own inside the mod's src/, not src/ itself");
      continue;
    }
    let tsconfig = null;
    try {
      tsconfig = parseJsonc(readText(root, `${wdir}/tsconfig.json`));
    } catch {
      bad("has no readable tsconfig.json (a Worker has a different global scope from the page)");
    }
    if (tsconfig) {
      const lib = (tsconfig.compilerOptions?.lib ?? []).map((l) => String(l).toLowerCase());
      if (!lib.includes("webworker") || lib.includes("dom")) {
        bad("has a tsconfig.json whose lib is not WebWorker without DOM");
      }
    }
    const fromMods = wdir.slice(MODS_DIR.length + 1);
    if (!Array.isArray(domExclude) || !domExclude.includes(fromMods)) {
      bad(
        `is not in the "exclude" of ${MODS_DIR}/tsconfig.json (${JSON.stringify(fromMods)}): the DOM check would read it`,
      );
    }
    const line = new RegExp(
      `\\btsc\\s+-p\\s+${wdir.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(\\s|$)`,
    );
    if (!line.test(justfile)) {
      bad(`has no \`tsc -p ${wdir}\` line in the justfile's tsc recipe: nothing type-checks it`);
    }
  }
  return out;
}

// ---- The tauri configs ----------------------------------------------------------------------------

/** RFC 7386 JSON Merge Patch, which is how Tauri layers tauri.<platform>.conf.json on the base. */
function mergePatch(target, patch) {
  if (!isObject(patch)) return patch;
  const out = isObject(target) ? { ...target } : {};
  for (const [k, v] of Object.entries(patch)) {
    if (v === null) delete out[k];
    else out[k] = mergePatch(out[k], v);
  }
  return out;
}

/** The sources of a `bundle.resources` value: a list of paths, or a map from source path to destination. */
function resourceSources(resources) {
  if (Array.isArray(resources)) return resources.filter((r) => typeof r === "string");
  if (isObject(resources)) return Object.keys(resources);
  return [];
}

// Bundle keys whose value is a path (or a list of paths) to a repo file that goes into the installer.
// Each is written once, in Tauri's camelCase spelling; `keyed` below reads every spelling Tauri's serde
// accepts for it (kebab-case aliases such as `license-file`, and `macos` for `macOS`).
const PATH_KEYS = [
  ["icon"],
  ["licenseFile"],
  ["license"],
  ...["frameworks", "entitlements", "license", "infoPlist"].map((k) => ["macOS", k]),
  ["macOS", "dmg", "background"],
  ...["template", "frameworks", "infoPlist"].map((k) => ["iOS", k]),
  ...[
    "template",
    "installerHooks",
    "license",
    "headerImage",
    "sidebarImage",
    "installerIcon",
    "uninstallerIcon",
    "uninstallerHeaderImage",
  ].map((k) => ["windows", "nsis", k]),
  ...["template", "fragmentPaths", "bannerPath", "dialogImagePath", "license"].map((k) => [
    "windows",
    "wix",
    k,
  ]),
  // `language` is a map from a language name to { localePath }; `*` is any key.
  ["windows", "wix", "language", "*", "localePath"],
  ["windows", "webviewInstallMode", "path"],
  ...["deb", "rpm"].flatMap((kind) =>
    [
      "desktopTemplate",
      "preInstallScript",
      "postInstallScript",
      "preRemoveScript",
      "postRemoveScript",
      "changelog",
    ].map((k) => ["linux", kind, k]),
  ),
];
const FILE_MAPS = [
  ["linux", "deb", "files"],
  ["linux", "rpm", "files"],
  ["linux", "appimage", "files"],
  ["macOS", "files"],
  ["windows", "nsis", "customLanguageFiles"],
];

// One spelling-blind form of a config key: Tauri's serde accepts kebab-case aliases (`license-file`,
// `external-bin`, `installer-hooks`...) next to camelCase, and `macos` next to `macOS`, and one alias
// is not the camelCase of its field (`install-icon` for `installerIcon`). Dropping hyphens and case
// folds all of them; reading a spelling Tauri would refuse is harmless, missing one it takes is not.
const KEY_ALIASES = { installicon: "installericon" };
const foldKey = (k) => {
  const f = k.replace(/[-_]/g, "").toLowerCase();
  return KEY_ALIASES[f] ?? f;
};

/**
 * Every value at `keyPath` under `root`, with the path as WRITTEN in the config (so a message points at
 * the line the author typed). `*` matches any key.
 */
function keyed(root, keyPath, written = []) {
  if (keyPath.length === 0) return [[written, root]];
  if (!isObject(root)) return [];
  const [head, ...rest] = keyPath;
  const want = foldKey(head);
  const out = [];
  for (const [k, v] of Object.entries(root)) {
    if (head === "*" || foldKey(k) === want) out.push(...keyed(v, rest, [...written, k]));
  }
  return out;
}

/**
 * Every repo path a bundle config copies into the installer, with the key that named it. Not only
 * `resources`: tauri-utils' config also lets `linux.{deb,rpm,appimage}.files` and `macOS.files` (a
 * map from destination to SOURCE path) and `externalBin` pull in arbitrary files, so a gate that read
 * `resources` alone would stay green while a mod shipped through one of the others. The plain path
 * keys (`PATH_KEYS`: icons, licence files, frameworks, templates, hooks, scripts, WiX fragments) copy
 * a repo file into the installer too, so they are read the same way, under every spelling Tauri
 * accepts (`keyed`). The list is the path-valued keys of Tauri 2.10's bundle config as known when this
 * was written; a new Tauri key that copies files
 * is not seen until it is added there.
 */
export function bundleSources(bundle) {
  if (!isObject(bundle)) return [];
  const out = resourceSources(bundle.resources).map((source) => ({
    key: "bundle.resources",
    source,
  }));
  for (const keyPath of PATH_KEYS) {
    for (const [key, value] of keyed(bundle, keyPath)) {
      for (const source of Array.isArray(value) ? value : [value]) {
        if (typeof source === "string") out.push({ key: `bundle.${key.join(".")}`, source });
      }
    }
  }
  // Maps whose VALUES are source paths (the keys are destinations or languages).
  for (const keyPath of FILE_MAPS) {
    for (const [key, files] of keyed(bundle, keyPath)) {
      if (!isObject(files)) continue;
      for (const source of Object.values(files)) {
        if (typeof source === "string") out.push({ key: `bundle.${key.join(".")}`, source });
      }
    }
  }
  // A list only: Tauri refuses a bare string there, so a string is not a source.
  for (const [key, list] of keyed(bundle, ["externalBin"])) {
    if (!Array.isArray(list)) continue;
    for (const source of list) {
      if (typeof source === "string") out.push({ key: `bundle.${key.join(".")}`, source });
    }
  }
  return out;
}

/** A glob as the `glob` crate Tauri uses reads it, as an anchored case-insensitive pattern. */
function globRegex(glob) {
  let re = "";
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i];
    if (c === "*" && glob[i + 1] === "*") {
      if (glob[i + 2] === "/") {
        re += "(?:.*/)?";
        i += 2;
      } else {
        re += ".*";
        i += 1;
      }
    } else if (c === "*") re += "[^/]*";
    else if (c === "?") re += "[^/]";
    else if (c === "[" && glob.indexOf("]", i + 2) > 0) {
      const end = glob.indexOf("]", i + 2);
      re += `[${glob
        .slice(i + 1, end)
        .replace(/^!/, "^")
        .replace(/\\/g, "\\\\")}]`;
      i = end;
    } else if (c === "{" && glob.indexOf("}", i) > 0) {
      const end = glob.indexOf("}", i);
      re += `(?:${glob
        .slice(i + 1, end)
        .split(",")
        .map((s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))
        .join("|")})`;
      i = end;
    } else re += c.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  }
  return new RegExp(`^${re}$`, "i");
}

/**
 * Why a resource source would put something under mods/ into the installer, or null. Sources are
 * relative to src-tauri/.
 */
export function resourceReaches(source, root, modNames = []) {
  const slashed = source.replace(/\\+/g, "/");
  let rel;
  if (slashed.startsWith("/") || /^[A-Za-z]:\//.test(slashed)) {
    const prefix = `${POSIX_ROOT(root)}/`;
    if (slashed.startsWith(prefix)) rel = posix.normalize(slashed.slice(prefix.length));
    else if (slashed.split("/").some((s) => s.toLowerCase() === MODS_DIR))
      return "is an absolute path through mods/";
    else return null;
  } else {
    rel = posix.normalize(posix.join(TAURI_DIR, slashed));
  }
  rel = rel.replace(/\/+$/, "");
  const lower = rel.toLowerCase();
  if (/[*?[{]/.test(rel)) {
    if (/^(\.\.\/?)+/.test(rel)) return "is a glob that starts outside the repository";
    const probes = [MODS_DIR, `${MODS_DIR}/x`, `${MODS_DIR}/x/y`];
    for (const name of modNames) {
      probes.push(`${MODS_DIR}/${name}/manifest.json`, `${MODS_DIR}/${name}/dist/app/index.html`);
    }
    const re = globRegex(rel);
    return probes.some((p) => re.test(p)) ? "is a glob that matches files under mods/" : null;
  }
  if (lower === MODS_DIR || lower.startsWith(`${MODS_DIR}/`)) return "is under mods/";
  if (rel === "." || rel === "" || /^(\.\.\/?)+$/.test(rel)) {
    return "is a folder that holds the whole repository, mods/ included";
  }
  return null;
}

function checkTauriConfigs(root, modNames) {
  const problems = [];
  const names = existsSync(join(root, TAURI_DIR))
    ? readdirSync(join(root, TAURI_DIR)).filter((n) =>
        /^tauri(\.[a-z0-9_-]+)?\.conf\.json$/.test(n),
      )
    : [];
  if (!names.includes("tauri.conf.json")) {
    return { problems: [`${TAURI_DIR}/tauri.conf.json is missing: nothing to check`], configs: 0 };
  }
  const read = (n) => JSON.parse(readText(root, `${TAURI_DIR}/${n}`));
  const base = read("tauri.conf.json");
  const merged = [["tauri.conf.json", base]];
  for (const n of names.filter((n) => n !== "tauri.conf.json").sort()) {
    merged.push([n, mergePatch(base, read(n))]);
  }
  for (const [name, config] of merged) {
    for (const { key, source } of bundleSources(config.bundle)) {
      const why = resourceReaches(source, root, modNames);
      if (why) {
        problems.push(
          `${TAURI_DIR}/${name}: ${key} source ${JSON.stringify(source)} ${why} ` +
            `(I-25: no mod ships in the installer)`,
        );
      }
    }
  }
  return { problems, configs: merged.length };
}

// ---- src/ never imports mods/ ---------------------------------------------------------------------

const SOURCE_FILE = /\.(m?[jt]sx?|cjs|cts|css|html)$/;
const SPECIFIERS = [
  /\bfrom\s*(["'`])([^"'`\n]*)\1/g,
  /\bimport\s*(["'`])([^"'`\n]*)\1/g,
  /\bimport\s*\(\s*(["'`])([^"'`\n]*)\1/g,
  /\brequire\s*\(\s*(["'`])([^"'`\n]*)\1/g,
  /\bnew\s+URL\s*\(\s*(["'`])([^"'`\n]*)\1/g,
  /<reference\s+path\s*=\s*(["'])([^"'\n]*)\1/g,
];
const CSS_SPECIFIERS = [
  /@import\s+(?:url\(\s*)?(["']?)([^"')\s;]+)\1/g,
  /\burl\(\s*(["']?)([^"')\s]+)\1\s*\)/g,
];
// A page names its scripts and styles by attribute, and may carry an inline module script.
const HTML_SPECIFIERS = [/\b(?:src|href)\s*=\s*(["'])([^"'\n]*)\1/g, ...SPECIFIERS];

/** Every module specifier a file's text names, however it is spelled (comments included: this fails closed). */
export function specifiersIn(text, file) {
  const out = [];
  const patterns = /\.css$/.test(file)
    ? CSS_SPECIFIERS
    : /\.html$/.test(file)
      ? HTML_SPECIFIERS
      : SPECIFIERS;
  for (const re of patterns) for (const m of text.matchAll(re)) out.push(m[2]);
  if (!/\.css$/.test(file)) {
    for (const g of text.matchAll(/import\.meta\.glob(?:Eager)?\s*\(([^)]*)\)/g)) {
      for (const s of g[1].matchAll(/(["'`])([^"'`\n]*)\1/g)) out.push(s[2]);
    }
  }
  return out;
}

/** Whether `specifier`, written in repo-relative `fromFile`, names the top-level mods/ folder or a file in it. */
export function resolvesIntoMods(specifier, fromFile, root) {
  const s = specifier
    .replace(/[?#].*$/, "")
    .replace(/\\+/g, "/")
    .replace(/^!/, "");
  if (s === "") return false;
  const rootPrefix = `${POSIX_ROOT(root)}/`;
  let rel;
  if (s.startsWith("./") || s.startsWith("../")) {
    rel = posix.normalize(posix.join(posix.dirname(fromFile), s));
  } else if (s.startsWith(rootPrefix)) {
    rel = posix.normalize(s.slice(rootPrefix.length)); // an absolute path into this checkout
  } else if (s.startsWith("/")) {
    rel = posix.normalize(s.slice(1)); // Vite reads a leading slash as the project root
  } else {
    rel = posix.normalize(s); // bare: no package of ours is called mods
  }
  const lower = rel.toLowerCase();
  return lower === MODS_DIR || lower.startsWith(`${MODS_DIR}/`);
}

function checkSrcImports(root) {
  const problems = [];
  const { files } = walk(
    root,
    "src",
    (rel) => rel === "node_modules" || rel.endsWith("/node_modules"),
  );
  let scanned = 0;
  // The Vite entry page at the repo root names PM's main script by path, so it is read like src/.
  const pages = existsSync(join(root, "index.html")) ? ["index.html"] : [];
  for (const file of [...files.filter((f) => SOURCE_FILE.test(f)), ...pages]) {
    if (!pages.includes(file)) scanned++;
    for (const spec of specifiersIn(readText(root, file), file)) {
      if (resolvesIntoMods(spec, file, root)) {
        problems.push(
          `${file}: imports ${JSON.stringify(spec)}, which is the top-level mods/ folder ` +
            `(I-25: PM's code never reaches a mod's source; a mod reaches PM only as a signed package)`,
        );
      }
    }
  }
  if (scanned === 0)
    problems.push("src/ held no source files to scan: the path moved, or the walk broke");
  return { problems, scanned };
}

// ---- Rust embedding a mod's file ------------------------------------------------------------------

// A Rust `include_str!` of a mod's file would put that file inside PM's own binary, which skips the
// verifier, the ledger and the blacklist just as a bundle resource would. This is a mistake-catching
// lint (a careless edit), not a wall against someone hiding the path, so it reads the forms people
// write: a string literal, and `concat!(env!("CARGO_MANIFEST_DIR"), "...")`.
//
// Test code may embed a mod's committed file (a test that parses mods/atlas.map/manifest.json is how
// the Rust job notices drift). The gate knows test code by: a file named `tests.rs` or `*_tests.rs`, a
// file under a `tests/` folder, and an item behind `#[cfg(test)]` exactly. Any other test-only use
// (for instance `#[cfg(all(test, ...))]`) is allow-listed here by FILE PATH, never by a marker comment.
export const RUST_INCLUDE_ALLOWED = [];

const INCLUDE_MACROS = new Set(["include_str", "include_bytes", "include"]);

/**
 * Rust source as tokens: `{ t: "id" | "str" | "p", v, at }`. Comments are dropped, a string keeps its
 * value (raw and byte strings too), and a character literal is told from a lifetime, so a brace or a
 * quote inside either cannot unbalance what reads these.
 */
export function rustTokens(text) {
  const out = [];
  let i = 0;
  const n = text.length;
  while (i < n) {
    const c = text[i];
    if (/\s/.test(c)) i++;
    else if (c === "/" && text[i + 1] === "/") {
      while (i < n && text[i] !== "\n") i++;
    } else if (c === "/" && text[i + 1] === "*") {
      let depth = 1;
      i += 2;
      while (i < n && depth > 0) {
        if (text[i] === "/" && text[i + 1] === "*") {
          depth++;
          i += 2;
        } else if (text[i] === "*" && text[i + 1] === "/") {
          depth--;
          i += 2;
        } else i++;
      }
    } else if (/[A-Za-z_]/.test(c)) {
      let j = i;
      while (j < n && /[A-Za-z0-9_]/.test(text[j])) j++;
      const word = text.slice(i, j);
      const raw = /^(?:b?r)$/.test(word) && /["#]/.test(text[j] ?? "");
      if (raw) {
        let k = j;
        while (text[k] === "#") k++;
        const hashes = k - j;
        if (text[k] === '"') {
          const close = `"${"#".repeat(hashes)}`;
          const end = text.indexOf(close, k + 1);
          const stop = end < 0 ? n : end;
          out.push({ t: "str", v: text.slice(k + 1, stop), at: i });
          i = end < 0 ? n : end + close.length;
          continue;
        }
      }
      if (word === "b" && text[j] === '"') {
        i = j; // a byte string reads like a string; the quote is handled next time round
        continue;
      }
      out.push({ t: "id", v: word, at: i });
      i = j;
    } else if (c === '"') {
      let j = i + 1;
      let v = "";
      while (j < n && text[j] !== '"') {
        if (text[j] === "\\") {
          const e = text[j + 1];
          v += e === "\\" || e === '"' ? e : `\\${e ?? ""}`;
          j += 2;
        } else v += text[j++];
      }
      out.push({ t: "str", v, at: i });
      i = j + 1;
    } else if (c === "'") {
      // A character literal ('x', '\n', '\'') or a lifetime ('a).
      const cp = text.codePointAt(i + 1);
      const width = cp === undefined ? 0 : String.fromCodePoint(cp).length;
      if (text[i + 1] === "\\") {
        let j = i + 2;
        while (j < n && text[j] !== "'") j += text[j] === "\\" ? 2 : 1;
        i = j + 1;
      } else if (width > 0 && text[i + 1 + width] === "'") i += width + 2;
      else i++;
    } else if (/[0-9]/.test(c)) {
      let j = i;
      while (j < n && /[A-Za-z0-9_]/.test(text[j])) j++;
      out.push({ t: "id", v: text.slice(i, j), at: i });
      i = j;
    } else {
      out.push({ t: "p", v: c, at: i });
      i++;
    }
  }
  return out;
}

/** The index of the token that closes the bracket opened at `open`, or -1. */
function closeOf(tokens, open) {
  const pairs = { "(": ")", "[": "]", "{": "}" };
  const stack = [];
  for (let k = open; k < tokens.length; k++) {
    const tok = tokens[k];
    if (tok.t !== "p") continue;
    if (pairs[tok.v]) stack.push(pairs[tok.v]);
    else if (tok.v === stack[stack.length - 1]) {
      stack.pop();
      if (stack.length === 0) return k;
    }
  }
  return -1;
}

/**
 * The token ranges `[from, to]` that are test-only: the item behind each `#[cfg(test)]` (a module, a
 * function, a `use`), and the whole file for an inner `#![cfg(test)]`. Only `cfg(test)` exactly counts.
 */
export function testRanges(tokens) {
  const ranges = [];
  const isCfgTest = (from, to) =>
    tokens
      .slice(from, to + 1)
      .map((t) => t.v)
      .join("") === "cfg(test)";
  for (let k = 0; k < tokens.length; k++) {
    if (tokens[k].v !== "#") continue;
    const inner = tokens[k + 1]?.v === "!";
    const open = k + (inner ? 2 : 1);
    if (tokens[open]?.v !== "[") continue;
    const close = closeOf(tokens, open);
    if (close < 0 || !isCfgTest(open + 1, close - 1)) continue;
    if (inner) {
      ranges.push([0, tokens.length - 1]);
      continue;
    }
    let at = close + 1;
    while (tokens[at]?.v === "#" && tokens[at + 1]?.v === "[") at = closeOf(tokens, at + 1) + 1;
    // The item runs to its closing brace, or to the `;` that ends it, whichever comes first at depth 0.
    let end = -1;
    for (let m = at; m < tokens.length; m++) {
      const v = tokens[m].t === "p" ? tokens[m].v : "";
      if (v === "(" || v === "[") {
        m = closeOf(tokens, m);
        if (m < 0) break;
      } else if (v === "{") {
        end = closeOf(tokens, m);
        break;
      } else if (v === ";") {
        end = m;
        break;
      }
    }
    ranges.push([k, end < 0 ? tokens.length - 1 : end]);
  }
  return ranges;
}

const MODS_SEGMENT = /(^|[\\/])mods([\\/]|$)/i;
const inTopLevelMods = (rel) => {
  const lower = rel.toLowerCase();
  return lower === MODS_DIR || lower.startsWith(`${MODS_DIR}/`);
};

/** The nearest folder at or above `file` that holds a Cargo.toml, under src-tauri/ (what CARGO_MANIFEST_DIR is). */
function crateDir(root, file) {
  let dir = posix.dirname(file);
  while (dir.length >= TAURI_DIR.length && dir.startsWith(TAURI_DIR)) {
    if (existsSync(join(root, dir, "Cargo.toml"))) return dir;
    dir = posix.dirname(dir);
  }
  return TAURI_DIR;
}

/**
 * The repo-relative path an include macro's argument tokens name, `null` when it names nothing the
 * gate can place. Two forms: one string literal (rustc reads it relative to the file holding the
 * macro), and `concat!` of string literals, optionally led by `env!("CARGO_MANIFEST_DIR")`.
 */
function includedPath(args, file, manifestDir) {
  const parts = args[args.length - 1]?.v === "," ? args.slice(0, -1) : args;
  const fromFile = (s) => posix.normalize(posix.join(posix.dirname(file), s.replace(/\\/g, "/")));
  if (parts.length === 1 && parts[0].t === "str") return fromFile(parts[0].v);
  if (parts[0]?.v !== "concat" || parts[1]?.v !== "!" || parts[2]?.v !== "(") return null;
  const inner = parts.slice(3, -1);
  if (parts[parts.length - 1]?.v !== ")") return null;
  let text = "";
  let usesManifest = false;
  for (let k = 0; k < inner.length;) {
    const tok = inner[k];
    if (tok.t === "str") {
      text += tok.v;
      k += 1;
    } else if (
      tok.v === "env" &&
      inner[k + 1]?.v === "!" &&
      inner[k + 2]?.v === "(" &&
      inner[k + 3]?.t === "str" &&
      inner[k + 3].v === "CARGO_MANIFEST_DIR" &&
      inner[k + 4]?.v === ")" &&
      k === 0
    ) {
      usesManifest = true;
      text += `${manifestDir}/`;
      k += 5;
    } else return null;
    if (inner[k]?.v === ",") k++;
    else if (k < inner.length) return null;
  }
  const clean = text.replace(/\\/g, "/");
  return usesManifest ? posix.normalize(clean) : fromFile(clean);
}

const isTestFile = (file) =>
  /(^|\/)tests\.rs$/.test(file) || /_tests\.rs$/.test(file) || file.split("/").includes("tests");

function checkRustIncludes(root) {
  const problems = [];
  const { files } = walk(root, TAURI_DIR, (rel) => rel === "target" || rel.endsWith("/target"));
  const sources = files.filter((f) => f.endsWith(".rs"));
  for (const file of sources) {
    const text = readText(root, file);
    const tokens = rustTokens(text);
    const line = (tok) => text.slice(0, tok.at).split("\n").length;
    const manifestDir = crateDir(root, file);
    if (posix.basename(file) === "build.rs") {
      // Copying mods/ into OUT_DIR is the same embedding by another road. Cargo runs a build script
      // from its crate's folder, so a path literal is read from there.
      const touchesOut = tokens.some((t) => t.v === "OUT_DIR");
      const hit = tokens.find(
        (t) =>
          t.t === "str" &&
          touchesOut &&
          inTopLevelMods(posix.normalize(posix.join(manifestDir, t.v.replace(/\\/g, "/")))),
      );
      if (hit) {
        problems.push(
          `${file}:${line(hit)}: names ${JSON.stringify(hit.v)} (the top-level mods/ folder) in a ` +
            `build script that uses OUT_DIR, which looks like a copy of a mod into PM's build ` +
            `(I-25: a mod reaches a user only as a signed package)`,
        );
      }
    }
    if (isTestFile(file) || RUST_INCLUDE_ALLOWED.includes(file)) continue;
    const ranges = testRanges(tokens);
    tokens.forEach((tok, k) => {
      if (tok.t !== "id" || !INCLUDE_MACROS.has(tok.v) || tokens[k + 1]?.v !== "!") return;
      if (ranges.some(([from, to]) => k >= from && k <= to)) return;
      const open = k + 2;
      if (!["(", "[", "{"].includes(tokens[open]?.v)) return;
      const args = tokens.slice(open + 1, closeOf(tokens, open));
      const rel = includedPath(args, file, manifestDir);
      const where = `${file}:${line(tok)}: ${tok.v}!`;
      const fix =
        `(I-25: a mod reaches a user only as a signed package. Test code may embed one: use a ` +
        `tests.rs or *_tests.rs file, a tests/ folder or #[cfg(test)]. A test-only use the gate ` +
        `cannot tell, such as cfg(all(test, ...)), goes in RUST_INCLUDE_ALLOWED in scripts/check-mods.mjs)`;
      if (rel !== null) {
        if (inTopLevelMods(rel))
          problems.push(`${where} embeds ${rel}, in the top-level mods/ folder ${fix}`);
      } else if (args.some((a) => a.t === "str" && MODS_SEGMENT.test(a.v))) {
        problems.push(
          `${where} names something under mods/ in a form this gate cannot resolve (it reads one ` +
            `string literal, or concat! of literals led by env!("CARGO_MANIFEST_DIR")), so it ` +
            `refuses it ${fix}`,
        );
      }
    });
  }
  if (sources.length === 0)
    problems.push(`${TAURI_DIR}/ held no Rust files to scan: the path moved, or the walk broke`);
  return { problems, scanned: sources.length };
}

// ---- The whole tree -------------------------------------------------------------------------------

/** Every rule that needs no git history. */
export function checkMods(root = repoRoot) {
  const problems = [];
  let rust;
  try {
    rust = rustConstants(root);
  } catch (e) {
    return { problems: [e.message], mods: 0, fixtures: 0, scanned: 0, configs: 0, rustScanned: 0 };
  }
  if (JSON.stringify(rust.permissions) !== JSON.stringify(PACK_MOD_PERMISSIONS)) {
    problems.push(
      `scripts/pack-mod.mjs PERMISSIONS differs from package.rs's: pack-mod would pack a manifest ` +
        `PM refuses (or refuse one PM takes). Make them equal, Rust wins.`,
    );
  }
  const folders = (base) =>
    existsSync(join(root, base))
      ? readdirSync(join(root, base), { withFileTypes: true })
          .filter((e) => e.isDirectory() || e.isSymbolicLink())
          .map((e) => e.name)
          .sort()
      : [];
  const mods = folders(MODS_DIR);
  for (const name of mods) {
    if (!isDir(root, `${MODS_DIR}/${name}`))
      problems.push(`${MODS_DIR}/${name}: is a symbolic link`);
    else problems.push(...checkModFolder(root, `${MODS_DIR}/${name}`, "mod", rust));
  }
  if (mods.length === 0) {
    problems.push(
      `${MODS_DIR}/ holds no mod folder: a gate that inspects nothing passes everything`,
    );
  }
  let fixtures = 0;
  for (const name of folders(FIXTURES_DIR)) {
    const dir = `${FIXTURES_DIR}/${name}`;
    if (!existsSync(join(root, dir, "manifest.json"))) continue;
    fixtures++;
    problems.push(...checkModFolder(root, dir, "fixture", rust));
  }
  const tauri = checkTauriConfigs(root, mods);
  problems.push(...tauri.problems);
  const src = checkSrcImports(root);
  problems.push(...src.problems);
  const rustSrc = checkRustIncludes(root);
  problems.push(...rustSrc.problems);
  return {
    problems,
    mods: mods.length,
    fixtures,
    scanned: src.scanned,
    configs: tauri.configs,
    rustScanned: rustSrc.scanned,
  };
}

// ---- The diff: D12 and --base ---------------------------------------------------------------------

function git(root, args) {
  return execFileSync("git", args, {
    cwd: root,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    maxBuffer: 1 << 26,
  });
}

/**
 * The paths this change touches relative to `baseRef`: committed, staged, unstaged, and untracked
 * (so a pre-commit run sees the commit it is about to make). Measured from the merge-base, so work
 * that landed on the base branch since the fork is not counted as this change's. Renames are listed
 * as the two paths they are, so moving a file into mods/ from outside it still touches outside.
 */
export function changedPaths(baseRef, root = repoRoot) {
  if (typeof baseRef !== "string" || baseRef === "" || baseRef.startsWith("-")) {
    return { ok: false, why: `${JSON.stringify(baseRef)} is not a ref` };
  }
  try {
    const mergeBase = git(root, ["merge-base", baseRef, "HEAD"]).trim();
    const tracked = git(root, ["diff", "--name-only", "--no-renames", "-z", mergeBase]);
    const untracked = git(root, ["ls-files", "--others", "--exclude-standard", "-z"]);
    const paths = [...new Set([...tracked.split("\0"), ...untracked.split("\0")].filter(Boolean))];
    return { ok: true, paths: paths.sort(), mergeBase };
  } catch (e) {
    return {
      ok: false,
      why: String(e.stderr || e.message)
        .trim()
        .split("\n")[0],
    };
  }
}

/**
 * D12: true only when something changed and every changed path is inside a mod's own folder
 * (`mods/<id>/...`). Never judgement. A file directly under mods/ (vite.config.ts, tsconfig.json,
 * vite-env.d.ts) is the build tooling SHARED by every mod: editing it can change the bytes of every
 * packed mod, yet no mod folder changed so none owes a bump (checkModBumps keys on `mods/<id>/`).
 * So such a change is PM's, and PM's version moves. This is stricter than the spec's wording ("touches
 * only mods/"); the review that found the gap left the final call to Bobby.
 */
export function isModsOnly(paths) {
  return paths.length > 0 && paths.every((p) => /^mods\/[^/]+\/./.test(p));
}

/**
 * D12 and the changed-folder rule: every mods/<id>/ in `diff` (from `changedPaths`) needs a manifest
 * version greater than the one at `baseRef`, and its top CHANGELOG entry. A folder that did not exist
 * at `baseRef` is new, so it has nothing to be greater than; one that no longer exists needs no bump.
 */
export function checkModBumps(baseRef, diff, root = repoRoot) {
  if (!diff.ok) {
    return {
      problems: [`could not list the changed files against ${baseRef}: ${diff.why}`],
      changed: 0,
      bumped: 0,
      fresh: 0,
    };
  }
  const changed = new Set();
  for (const p of diff.paths) {
    const m = /^mods\/([^/]+)\//.exec(p);
    if (m) changed.add(m[1]);
  }
  const problems = [];
  let bumped = 0;
  let fresh = 0;
  for (const name of [...changed].sort()) {
    const dir = `${MODS_DIR}/${name}`;
    const manifestPath = join(root, dir, "manifest.json");
    if (!existsSync(manifestPath)) continue;
    let before;
    try {
      const listed = git(root, ["ls-tree", "--name-only", baseRef, "--", `${dir}/manifest.json`]);
      before = listed.trim() === "" ? null : git(root, ["show", `${baseRef}:${dir}/manifest.json`]);
    } catch (e) {
      problems.push(
        `${dir}: could not read its manifest at ${baseRef}: ${
          String(e.stderr || e.message)
            .trim()
            .split("\n")[0]
        }`,
      );
      continue;
    }
    let now;
    try {
      now = parseStrictJson(readFileSync(manifestPath, "utf8")).version;
    } catch (e) {
      problems.push(`${dir}/manifest.json: ${e.message}`);
      continue;
    }
    if (before === null) {
      fresh++; // new at the base: the first version has nothing to exceed, so nothing was compared
    } else {
      let then;
      try {
        then = JSON.parse(before).version;
        if (typeof then !== "string" || typeof now !== "string" || semverCmp(now, then) <= 0) {
          problems.push(
            `${dir} changed but its version is ${now}, not greater than ${then} at ${baseRef}: ` +
              `bump manifest.json (D12: a mod's change moves the mod's version) and add the CHANGELOG entry`,
          );
          continue;
        }
      } catch (e) {
        problems.push(`${dir}: its version at ${baseRef} or now is not comparable (${e.message})`);
        continue;
      }
      bumped++;
    }
    problems.push(...changelogProblems(root, dir, now));
  }
  return { problems, changed: changed.size, bumped, fresh };
}

/** "2 changed mod folder(s) (1 new, 1 with a greater version)": what `checkModBumps` really verified. */
export function bumpSummary({ changed, bumped, fresh }) {
  return `${changed} changed mod folder(s) (${fresh} new, ${bumped} with a greater version)`;
}

// ---- Run ------------------------------------------------------------------------------------------

function main(argv) {
  const baseAt = argv.indexOf("--base");
  const extra = argv.filter((a, i) => !(a === "--base" || (baseAt >= 0 && i === baseAt + 1)));
  if (extra.length > 0 || (baseAt >= 0 && !argv[baseAt + 1])) {
    console.error("usage: node scripts/check-mods.mjs [--base <ref>]");
    process.exit(2);
  }
  const base = baseAt >= 0 ? argv[baseAt + 1] : undefined;
  const result = checkMods(repoRoot);
  const problems = new Set(result.problems);
  let bumpNote = "";
  if (base) {
    const bumps = checkModBumps(base, changedPaths(base, repoRoot), repoRoot);
    for (const p of bumps.problems) problems.add(p);
    bumpNote = `; ${bumpSummary(bumps)} against ${base}`;
  }
  if (problems.size > 0) {
    console.error("✗ check-mods:\n");
    for (const p of problems) console.error(`  • ${p}`);
    console.error("");
    process.exit(1);
  }
  console.log(
    `✓ check-mods: ${result.mods} mod folder(s) and ${result.fixtures} fixture(s) valid, ` +
      `${result.scanned} src files import nothing from mods/, ` +
      `${result.rustScanned} Rust files embed nothing from it outside tests, ` +
      `${result.configs} tauri config(s) bundle nothing from mods/${bumpNote}`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2));
}
