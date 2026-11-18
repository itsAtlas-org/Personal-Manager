// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Packs a built mod into a `.pmmod` file, and checks one (Mods spec §1.2, §7). Zero-dependency and
// offline: it runs in the pm-mods build job and on Bobby's laptop, and the two must produce the same
// bytes, so nothing here reads a clock, a locale, an environment or an absolute path.
//
//   node scripts/pack-mod.mjs <mod folder> [--commit <sha>] [--unsigned] [--out <file>]
//       the package, without a signature: the file the pm-mods sign job signs. Reads
//       <mod>/manifest.json, <mod>/dist/app/** (mods/vite.config.ts made it) and
//       <mod>/.build/modules.json, and the repo's LICENCE.txt. Prints `sha256  file`.
//       --commit stamps `publisher.commit` (the source tree cannot hold its own commit, so it carries
//       an all-zero placeholder; the verifier and the trusted comment need the real 40 hex digits).
//       A package for signing is REFUSED while it still holds the placeholder, because the
//       signature would vouch for a commit that does not exist; so is `attach` to one.
//       --unsigned makes the file a developer's own: the manifest must say tier "third" and the id
//       must not be an atlas. one (a file PM installs unsigned is never first party).
//   node scripts/pack-mod.mjs <mod folder> --stage [--commit <sha>]
//       writes manifest.json, LICENCE.txt and NOTICE.txt into <mod>/dist beside app/, which makes
//       dist/ a folder the developer-mode folder loader can open. No archive.
//   node scripts/pack-mod.mjs attach <archive> <signature file> --out <file>
//       adds `pmmod/signature.minisig` and writes the archive again with this same writer, so the
//       signed file is byte-for-byte what packing it with a signature would have been.
//   node scripts/pack-mod.mjs --verify <file>
//       checks a `.pmmod` against the rules src-tauri/src/mods/package.rs applies, plus the rules for
//       built output (no inline script or style, no module script, no `crossorigin`, no script or
//       link that loads from off-folder). Those are an author's lint, read broadly off the raw text
//       (a comment that mentions `<script` is refused too): the frame's CSP is the guard that holds
//       at run time. It cannot check the signature itself: the trusted keys are embedded
//       in PM, and only PM verifies against them.
//
// TWO IMPLEMENTATIONS OF ONE FORMAT. package.rs is the verifier that counts; the checks here are a
// mirror so a bad file fails in the build, not at install. They can drift, so the constants are
// pinned to package.rs by scripts/pack-mod.test.mjs, and a golden file built by this script is
// verified by package.rs in the Rust tests. A rule here that is stricter than package.rs only ever
// refuses a file PM would have taken; one that is looser is the dangerous direction, so when the two
// differ, the Rust text wins and this file changes.
//
// The archive: entries sorted by path, STORED (not deflated, so no dependence on a zlib build), no
// folder entries, DOS time 1980-01-01 00:00, creator Unix with mode 0644, no extra fields or comments.

import { createHash } from "node:crypto";
import {
  existsSync,
  lstatSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import vm from "node:vm";
import { crc32, inflateRawSync } from "node:zlib";

import { acceptable } from "./check-npm-licenses.mjs";
import { licenceTextFor } from "./generate-npm-notice.mjs";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

/** Refused input. The message is the whole explanation; nothing here is retried. */
export class PackError extends Error {}
const refuse = (message) => {
  throw new PackError(message);
};
const cmp = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

// ---- Constants, pinned to package.rs by the test --------------------------------------------------

export const MANIFEST = "manifest.json";
export const LICENCE_FILE = "LICENCE.txt";
export const NOTICE_FILE = "NOTICE.txt";
export const CHECKSUMS = "pmmod/checksums.json";
export const SIGNATURE = "pmmod/signature.minisig";
export const APP_DIR = "app/";
export const LICENCE = "AGPL-3.0-or-later";
export const MANIFEST_CAP = 256 << 10;
export const CHECKSUMS_CAP = 1 << 20;
export const SIGNATURE_CAP = 4 << 10;
export const PATH_CAP = 200;
export const COMPONENT_CAP = 100;
export const DEPTH_CAP = 16;
export const LIMITS = { file: 32 << 20, entries: 4096, entry: 16 << 20, total: 64 << 20 };
export const PERMISSIONS = [
  "storage",
  "ui.tab",
  "notify",
  "projects.read",
  "tasks.read",
  "documents.meta",
  "documents.vectors",
  "documents.text",
  "calendar.read",
  "calendar.details",
  "tasks.write",
  "milestones.write",
  "documents.write",
  "ai",
  "internet",
  "links",
];
const FORBIDDEN_SECRET_HEADERS = [
  "connection",
  "content-length",
  "cookie",
  "host",
  "keep-alive",
  "proxy-authenticate",
  "proxy-authorization",
  "proxy-connection",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
];
const TRUSTED_KEYS = ["pmmod", "id", "version", "tier", "author", "sourceRepo", "commit", "api"];

// ---- Paths (package.rs `check_path`, ids.rs `is_device_name`) -------------------------------------

export function isDeviceName(component) {
  const lower = component.split(".")[0].toLowerCase();
  if (["con", "prn", "aux", "nul", "conin$", "conout$"].includes(lower)) return true;
  return /^(com|lpt)[0-9]$/.test(lower);
}

/** The path as an error message shows it: quoted, cut, escaped, so it cannot fake the text round it. */
const quote = (s) => JSON.stringify(s.length > 80 ? `${s.slice(0, 80)}…` : s);

export function checkPath(path) {
  const bad = (why) => refuse(`${quote(path)} isn't allowed in a mod (${why})`);
  if (path === "") bad("empty name");
  if (Buffer.byteLength(path) > PATH_CAP) bad("name too long");
  if (path.startsWith("/")) bad("absolute path");
  if (path.includes("\\")) bad("backslash");
  if (path.includes(":")) bad("colon");
  const components = path.split("/");
  if (components.length > DEPTH_CAP) bad("too deep");
  for (const c of components) {
    if (c === "") bad("empty folder name");
    if (c === "." || c === "..") bad("dot segment");
    if (Buffer.byteLength(c) > COMPONENT_CAP) bad("name too long");
    if (c.endsWith(".") || c.endsWith(" ")) bad("trailing dot or space");
    if (!/^[A-Za-z0-9._+-]+$/.test(c)) bad("character");
    if (isDeviceName(c)) bad("device name");
  }
  const inLayout =
    [MANIFEST, LICENCE_FILE, NOTICE_FILE, CHECKSUMS, SIGNATURE].includes(path) ||
    path.startsWith(APP_DIR);
  if (!inLayout) bad("outside the package layout");
}

/** No two names equal after ASCII case-folding, and no file whose folded name is also a folder. */
export function checkCaseCollisions(names) {
  const folded = new Map();
  for (const name of names) {
    const key = name.replace(/[A-Z]/g, (c) => c.toLowerCase());
    if (folded.has(key)) {
      refuse(`${quote(folded.get(key))} and ${quote(name)} are the same file on Windows and macOS`);
    }
    folded.set(key, name);
  }
  for (const name of names) {
    const lower = name.replace(/[A-Z]/g, (c) => c.toLowerCase());
    for (let i = lower.indexOf("/"); i !== -1; i = lower.indexOf("/", i + 1)) {
      if (folded.has(lower.slice(0, i))) {
        refuse(`${quote(folded.get(lower.slice(0, i)))} is a file and a folder`);
      }
    }
  }
}

// ---- Strict JSON ---------------------------------------------------------------------------------

/**
 * JSON that refuses what serde_json refuses and JSON.parse forgives: a repeated key (JSON.parse keeps
 * the last, so a manifest could say one thing to a reader and another to PM), a byte-order mark, a
 * lone surrogate escape, trailing text. Objects have no prototype, so a `__proto__` key is data.
 */
export function parseStrictJson(text) {
  let i = 0;
  const fail = (why) => refuse(`not valid JSON (${why} at ${i})`);
  const ws = () => {
    while (i < text.length && " \t\n\r".includes(text[i])) i++;
  };
  const string = () => {
    i++;
    let out = "";
    for (;;) {
      if (i >= text.length) fail("unterminated string");
      const c = text[i++];
      if (c === '"') return out;
      if (c.charCodeAt(0) < 0x20) fail("control character in string");
      if (c !== "\\") {
        out += c;
        continue;
      }
      const e = text[i++];
      const simple = {
        '"': '"',
        "\\": "\\",
        "/": "/",
        b: "\b",
        f: "\f",
        n: "\n",
        r: "\r",
        t: "\t",
      };
      if (e in simple) out += simple[e];
      else if (e === "u") {
        const hex = text.slice(i, i + 4);
        if (!/^[0-9a-fA-F]{4}$/.test(hex)) fail("bad \\u escape");
        i += 4;
        out += String.fromCharCode(parseInt(hex, 16));
      } else fail("bad escape");
    }
  };
  const value = (depth) => {
    if (depth > 64) fail("nested too deep");
    ws();
    const c = text[i];
    if (c === "{") {
      i++;
      const obj = Object.create(null);
      ws();
      if (text[i] === "}") {
        i++;
        return obj;
      }
      for (;;) {
        ws();
        if (text[i] !== '"') fail("expected a key");
        const key = string();
        if (key in obj) fail(`the key ${quote(key)} is repeated`);
        ws();
        if (text[i++] !== ":") fail("expected a colon");
        obj[key] = value(depth + 1);
        ws();
        const next = text[i++];
        if (next === "}") return obj;
        if (next !== ",") fail("expected a comma or a closing brace");
      }
    }
    if (c === "[") {
      i++;
      const arr = [];
      ws();
      if (text[i] === "]") {
        i++;
        return arr;
      }
      for (;;) {
        arr.push(value(depth + 1));
        ws();
        const next = text[i++];
        if (next === "]") return arr;
        if (next !== ",") fail("expected a comma or a closing bracket");
      }
    }
    if (c === '"') return string();
    const literal = /^(true|false|null|-?(0|[1-9]\d*)(\.\d+)?([eE][+-]?\d+)?)/.exec(text.slice(i));
    if (!literal) fail("unexpected character");
    i += literal[0].length;
    return literal[0] === "true"
      ? true
      : literal[0] === "false"
        ? false
        : literal[0] === "null"
          ? null
          : Number(literal[0]);
  };
  const result = value(0);
  ws();
  if (i !== text.length) fail("text after the value");
  // A lone surrogate (an escape or a raw one) is not a Unicode string serde would accept.
  const walk = (v) => {
    if (typeof v === "string") {
      if (!v.isWellFormed()) refuse("not valid JSON (a lone surrogate)");
    } else if (Array.isArray(v)) v.forEach(walk);
    else if (v && typeof v === "object") {
      for (const [k, x] of Object.entries(v)) {
        walk(k);
        walk(x);
      }
    }
  };
  walk(result);
  return result;
}

// ---- The manifest (package.rs `parse_manifest`) ---------------------------------------------------

const isObject = (v) => v !== null && typeof v === "object" && !Array.isArray(v);
const isWord = (s, max, extra, upper) =>
  typeof s === "string" &&
  s.length > 0 &&
  Buffer.byteLength(s) <= max &&
  [...s].every(
    (c) =>
      (c >= "a" && c <= "z") ||
      (c >= "0" && c <= "9") ||
      (upper && c >= "A" && c <= "Z") ||
      extra.includes(c),
  );
const isLower = (c) => c >= "a" && c <= "z";

/** Fields of an object: exactly `required` plus any of `optional`, nothing else. */
function fields(value, what, required, optional = []) {
  if (!isObject(value)) refuse(`its manifest is invalid (${what} must be an object)`);
  for (const k of Object.keys(value)) {
    if (!required.includes(k) && !optional.includes(k)) {
      refuse(`its manifest is invalid (${what} has an unknown field ${quote(k)})`);
    }
  }
  for (const k of required) {
    if (!(k in value)) refuse(`its manifest is invalid (${what} has no ${k})`);
  }
  return value;
}
const manifestError = (why) => refuse(`its manifest is invalid (${why})`);
function str(v, what) {
  if (typeof v !== "string") manifestError(`${what} must be text`);
  return v;
}
function list(v, what) {
  if (!Array.isArray(v)) manifestError(`${what} must be a list`);
  return v;
}

/** Format and direction characters, zero-width ones, fillers, variation selectors and tags. */
const INVISIBLE = [
  [0x00ad, 0x00ad],
  [0x034f, 0x034f],
  [0x061c, 0x061c],
  [0x115f, 0x1160],
  [0x17b4, 0x17b5],
  [0x180b, 0x180f],
  [0x200b, 0x200f],
  [0x2028, 0x202e],
  [0x2060, 0x206f],
  [0x3164, 0x3164],
  [0xfe00, 0xfe0f],
  [0xfeff, 0xfeff],
  [0xffa0, 0xffa0],
  [0xfff0, 0xfffb],
  [0xe0000, 0xe0fff],
];

/**
 * package.rs `MAX_STACKED_MARKS`: the most combining marks (category M) one letter may carry. Real
 * scripts stack three or four; more than this in a row is refused.
 *
 * `\p{M}` here is the Unicode version of this Node's ICU, and package.rs's is its regex crate's
 * (Node 24.19's is newer: about 40 marks added since, e.g. U+1AE0). So this lint can only be
 * stricter than package.rs, never looser (a code point is a mark in the newer table whenever it is
 * one in the older), which is the safe direction for "--verify never accepts what Rust refuses".
 */
export const MAX_STACKED_MARKS = 4;
const MARK_RUN = new RegExp(`\\p{M}{${MAX_STACKED_MARKS + 1}}`, "u");

/**
 * package.rs `plain_text_fault`: bounded, trimmed, no control, hidden or direction characters, and
 * no run of more than `MAX_STACKED_MARKS` combining marks.
 */
export function plainTextFault(s, maxChars, newlines, mayBeEmpty) {
  if (s === "") return mayBeEmpty ? null : "is empty";
  if ([...s].length > maxChars) return `is longer than ${maxChars} characters`;
  if (s.trim() !== s) return "starts or ends with a space";
  for (const ch of s) {
    const allowed = ch === " " || (newlines && ch === "\n");
    const cp = ch.codePointAt(0);
    if (
      (/\s/u.test(ch) && !allowed) ||
      (/\p{Cc}/u.test(ch) && !allowed) ||
      INVISIBLE.some(([lo, hi]) => cp >= lo && cp <= hi)
    ) {
      return "has a hidden or control character";
    }
  }
  if (MARK_RUN.test(s)) return "has too many accent marks stacked on one letter";
  return null;
}
function plainText(what, s, max, newlines, mayBeEmpty) {
  const fault = plainTextFault(str(s, what), max, newlines, mayBeEmpty);
  if (fault) manifestError(`${what} ${fault}`);
}

export function checkModId(id) {
  const segments = id.split(".");
  if (segments.length < 2 || segments.length > 5)
    refuse("its id isn't two to five dot-separated parts");
  for (const seg of segments) {
    if (!/^[a-z][a-z0-9-]{0,30}$/.test(seg)) {
      refuse(
        "a mod id is two to five dot-separated parts, each a lowercase letter then up to 30 " +
          "lowercase letters, digits or hyphens",
      );
    }
  }
  if (segments.some(isDeviceName))
    refuse("a mod id can't use a name Windows reserves for a device");
}
export const isReservedId = (id) => id.slice(0, 6).toLowerCase() === "atlas.";

/** How many people a manifest may credit (package.rs `MAX_AUTHORS`). */
const MAX_AUTHORS = 8;

const isCommit = (s) => typeof s === "string" && /^[0-9a-f]{40}$/.test(s);
const isSourceRepo = (s) => {
  if (typeof s !== "string") return false;
  const at = s.indexOf("/");
  if (at < 0) return false;
  const owner = s.slice(0, at);
  const repo = s.slice(at + 1);
  return (
    isWord(owner, 39, "-", true) &&
    !owner.startsWith("-") &&
    isWord(repo, 100, "._-", true) &&
    repo !== "." &&
    repo !== ".."
  );
};
const isInternetHost = (h) => {
  if (h.length > 253 || h === "localhost" || h.endsWith(".localhost")) return false;
  const labels = h.split(".");
  return (
    labels.length >= 2 &&
    labels.every((l) => isWord(l, 63, "-", false) && !l.startsWith("-") && !l.endsWith("-")) &&
    !/^[0-9]+$/.test(labels[labels.length - 1])
  );
};
const isDataKey = (k) => isWord(k, 64, "_.-", false) && isLower(k[0]);
const extensionIs = (path, ext) => {
  const dot = path.lastIndexOf(".");
  return dot > 0 && path[dot - 1] !== "/" && path.slice(dot + 1).toLowerCase() === ext;
};
const parseMajorMinor = (s) => {
  const m = typeof s === "string" ? /^(0|[1-9][0-9]{0,3})\.(0|[1-9][0-9]{0,3})$/.exec(s) : null;
  return m ? [Number(m[1]), Number(m[2])] : null;
};
const SEMVER =
  /^(0|[1-9]\d{0,17})\.(0|[1-9]\d{0,17})\.(0|[1-9]\d{0,17})(-((0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(\.(0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?$/;

/**
 * Check a parsed manifest the way package.rs does and return the fields the rest of the packer needs.
 * `files` is the set of package paths the package holds, for the paths the manifest names.
 */
export function checkManifest(raw, files) {
  fields(
    raw,
    "the manifest",
    ["id", "name", "description", "version", "apiVersion", "publisher", "licence", "entry"],
    [
      "icon",
      "authors",
      "permissions",
      "internetHosts",
      "secrets",
      "contributions",
      "data",
      "requires",
      "engines",
    ],
  );
  const id = str(raw.id, "the id");
  checkModId(id);
  plainText("name", raw.name, 40, false, false);
  plainText("description", raw.description, 280, true, true);
  const listedApp = (what, path) => {
    if (typeof path !== "string" || !path.startsWith(APP_DIR) || !files.has(path)) {
      manifestError(`the ${what} ${quote(String(path))} isn't one of its app/ files`);
    }
  };
  if ("icon" in raw) {
    const icon = str(raw.icon, "icon");
    if (!["png", "jpg", "jpeg", "webp", "gif", "svg"].some((e) => extensionIs(icon, e))) {
      manifestError("icon must be a PNG, JPEG, WebP, GIF or SVG");
    }
    listedApp("icon", icon);
  }
  const version = str(raw.version, "version");
  if (version.length > 64 || !SEMVER.test(version)) {
    manifestError(`version ${quote(version)} isn't a plain semver`);
  }
  fields(raw.apiVersion, "apiVersion", ["min", "target"]);
  const min = parseMajorMinor(raw.apiVersion.min);
  const target = parseMajorMinor(raw.apiVersion.target);
  if (!min || !target || min[0] > target[0] || (min[0] === target[0] && min[1] > target[1])) {
    manifestError('apiVersion must be {"min": "M.m", "target": "M.m"} with min <= target');
  }
  const publisher = fields(raw.publisher, "publisher", ["tier", "author", "sourceRepo", "commit"]);
  if (!["first", "second", "third"].includes(publisher.tier)) manifestError("publisher.tier");
  plainText("publisher.author", publisher.author, 64, false, false);
  if (!isSourceRepo(publisher.sourceRepo)) manifestError("publisher.sourceRepo must be owner/repo");
  if (!isCommit(publisher.commit)) {
    manifestError("publisher.commit must be a 40-digit lowercase hex commit");
  }
  // `authors`: display-only credit (package.rs `parse_authors`). Absent is fine; present it holds 1 to 8
  // names, each plain text of at most 64 characters like publisher.author. No pin reads it.
  if ("authors" in raw) {
    const names = list(raw.authors, "authors");
    if (names.length < 1 || names.length > MAX_AUTHORS) {
      manifestError(`authors must name 1 to ${MAX_AUTHORS} people`);
    }
    for (const name of names) plainText("an author's name", name, 64, false, false);
  }
  if (raw.licence !== LICENCE) refuse(`its licence must be ${LICENCE}`);

  const granted = new Set();
  for (const p of "permissions" in raw ? list(raw.permissions, "permissions") : []) {
    fields(p, "a permission", ["id", "why"]);
    if (!PERMISSIONS.includes(p.id)) manifestError(`unknown permission ${quote(String(p.id))}`);
    if (granted.has(p.id)) manifestError(`${quote(p.id)} is listed twice`);
    granted.add(p.id);
    plainText("a permission's why", p.why, 160, false, false);
  }
  const hosts = new Set();
  const hostList = "internetHosts" in raw ? list(raw.internetHosts, "internetHosts") : [];
  if (hostList.length > 32) manifestError("too many internet hosts");
  for (const h of hostList) {
    if (typeof h !== "string" || !isInternetHost(h)) {
      manifestError(`${quote(String(h))} isn't an internet host name`);
    }
    if (hosts.has(h)) manifestError(`${quote(h)} is listed twice`);
    hosts.add(h);
  }
  if ((hosts.size === 0) === granted.has("internet")) {
    manifestError("the internet permission and internetHosts go together");
  }
  const secretNames = new Set();
  const secretList = "secrets" in raw ? list(raw.secrets, "secrets") : [];
  if (secretList.length > 16) manifestError("too many secrets");
  for (const s of secretList) {
    fields(s, "a secret", ["name", "host", "header"]);
    if (!isWord(s.name, 32, "_-", true) || !isLower(s.name[0])) {
      manifestError(`secret name ${quote(String(s.name))}`);
    }
    if (secretNames.has(s.name)) manifestError(`secret ${quote(s.name)} is listed twice`);
    secretNames.add(s.name);
    if (!hosts.has(s.host)) {
      manifestError(`secret ${quote(s.name)} goes to a host internetHosts doesn't list`);
    }
    const headerOk =
      isWord(s.header, 64, "-", true) &&
      /^[A-Za-z]/.test(s.header) &&
      !FORBIDDEN_SECRET_HEADERS.includes(s.header.toLowerCase());
    if (!headerOk)
      manifestError(`secret ${quote(s.name)} can't go in header ${quote(String(s.header))}`);
  }
  if ("contributions" in raw) {
    const c = fields(
      raw.contributions,
      "contributions",
      [],
      ["nav.tab", "calendar.overlay", "help"],
    );
    if ("nav.tab" in c) {
      fields(c["nav.tab"], "nav.tab", ["label"]);
      plainText("nav.tab's label", c["nav.tab"].label, 24, false, false);
      if (!granted.has("ui.tab")) manifestError("a nav.tab needs the ui.tab permission");
    }
    if ("calendar.overlay" in c) {
      fields(c["calendar.overlay"], "calendar.overlay", ["label"]);
      plainText("calendar.overlay's label", c["calendar.overlay"].label, 40, false, false);
    }
    const help = "help" in c ? list(c.help, "help") : [];
    if (help.length > 64) manifestError("too many help entries");
    const helpIds = new Set();
    for (const h of help) {
      fields(h, "a help entry", ["id", "title", "body"]);
      const ok = isWord(h.id, 48, "-", false) && isLower(h.id[0]);
      if (!ok || helpIds.has(h.id)) manifestError(`help id ${quote(String(h.id))}`);
      helpIds.add(h.id);
      plainText("a help title", h.title, 80, false, false);
      plainText("a help body", h.body, 2000, true, false);
    }
  }
  if ("data" in raw) {
    const d = fields(raw.data, "data", [], ["keys", "projectRefs"]);
    const keys = "keys" in d ? list(d.keys, "data.keys") : [];
    const refs = "projectRefs" in d ? list(d.projectRefs, "data.projectRefs") : [];
    if (keys.length > 0 && !granted.has("storage"))
      manifestError("data keys need the storage permission");
    if (keys.length > 64 || refs.length > 64) manifestError("too many data keys or project refs");
    const seen = new Set();
    for (const k of keys) {
      fields(k, "a data key", ["key", "class"]);
      if (!["content", "cache"].includes(k.class))
        manifestError(`data key ${quote(String(k.key))}'s class`);
      if (typeof k.key !== "string" || !isDataKey(k.key) || seen.has(k.key)) {
        manifestError(`data key ${quote(String(k.key))}`);
      }
      seen.add(k.key);
    }
    for (const r of refs) {
      fields(r, "a projectRef", ["key", "path"]);
      if (!seen.has(r.key)) {
        manifestError(
          `a projectRef names ${quote(String(r.key))}, which data.keys doesn't declare`,
        );
      }
      const ok =
        typeof r.path === "string" &&
        r.path.length >= 1 &&
        r.path.length <= 256 &&
        /^[\x21-\x7e]+$/.test(r.path);
      if (!ok) manifestError(`projectRef path ${quote(String(r.path))}`);
    }
  }
  const entry = fields(raw.entry, "entry", ["page"], ["workers"]);
  listedApp("entry page", entry.page);
  if (!extensionIs(entry.page, "html")) manifestError("the entry page must be an .html file");
  const workers = "workers" in entry ? list(entry.workers, "entry.workers") : [];
  if (workers.length > 8) manifestError("too many workers");
  const workerSet = new Set();
  for (const w of workers) {
    listedApp("worker", w);
    if (!(extensionIs(w, "js") || extensionIs(w, "mjs")))
      manifestError(`worker ${quote(w)} isn't a script`);
    if (workerSet.has(w)) manifestError(`worker ${quote(w)} is listed twice`);
    workerSet.add(w);
  }
  const rooms = new Set();
  for (const r of "requires" in raw ? list(raw.requires, "requires") : []) {
    if (!["calendar", "localai"].includes(r)) manifestError(`a required room ${quote(String(r))}`);
    if (rooms.has(r)) manifestError("a required room is listed twice");
    rooms.add(r);
  }
  if ("engines" in raw) {
    fields(raw.engines, "engines", ["frame"]);
    if (raw.engines.frame !== true) manifestError('engines must be {"frame": true}');
  }
  return {
    id,
    version,
    tier: publisher.tier,
    author: publisher.author,
    sourceRepo: publisher.sourceRepo,
    commit: publisher.commit,
    api: `${min[0]}.${min[1]}-${target[0]}.${target[1]}`,
    entryPage: entry.page,
  };
}

/** package.rs `ids::check_tier`: `atlas.` is first party's and first party publishes only there. */
export function checkTier(id, tier) {
  const reserved = isReservedId(id);
  if (reserved && tier === "first") return;
  if (!reserved && (tier === "second" || tier === "third")) return;
  refuse(
    reserved
      ? 'only Atlas\'s own mods may use an id starting "atlas."'
      : 'an Atlas mod\'s id must start "atlas."',
  );
}

// ---- The zip --------------------------------------------------------------------------------------

const DOS_DATE = (0 << 9) | (1 << 5) | 1; // 1980-01-01
const DOS_TIME = 0;
const MADE_BY_UNIX = (3 << 8) | 20;
const FILE_MODE = (0o100644 << 16) >>> 0;

/** Entries (path to bytes) as a zip, in path order. The only writer. */
export function writeZip(entries) {
  const names = [...entries.keys()].sort(cmp);
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const name of names) {
    const data = entries.get(name);
    const nameBytes = Buffer.from(name, "utf8");
    const crc = crc32(data);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(10, 4); // needed: stored
    local.writeUInt16LE(0, 6); // flags
    local.writeUInt16LE(0, 8); // stored
    local.writeUInt16LE(DOS_TIME, 10);
    local.writeUInt16LE(DOS_DATE, 12);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(nameBytes.length, 26);
    local.writeUInt16LE(0, 28);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(MADE_BY_UNIX, 4);
    central.writeUInt16LE(10, 6);
    central.writeUInt16LE(0, 8);
    central.writeUInt16LE(0, 10);
    central.writeUInt16LE(DOS_TIME, 12);
    central.writeUInt16LE(DOS_DATE, 14);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(data.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(nameBytes.length, 28);
    central.writeUInt16LE(0, 30); // extra
    central.writeUInt16LE(0, 32); // comment
    central.writeUInt16LE(0, 34); // disk
    central.writeUInt16LE(0, 36); // internal attributes
    central.writeUInt32LE(FILE_MODE, 38);
    central.writeUInt32LE(offset, 42);
    locals.push(local, nameBytes, data);
    centrals.push(central, nameBytes);
    offset += 30 + nameBytes.length + data.length;
  }
  const directory = Buffer.concat(centrals);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(names.length, 8);
  end.writeUInt16LE(names.length, 10);
  end.writeUInt32LE(directory.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, directory, end]);
}

/**
 * Read a zip the way package.rs opens one: a single archive from byte 0 with nothing before, between
 * or after its entries, one entry per name, regular files only, caps enforced on the bytes actually
 * decompressed. Stricter than the Rust reader in one way: no extra fields, comments, data descriptors
 * or zip64, none of which this writer emits. Returns the entries in archive order.
 */
export function readZip(buf, limits = LIMITS) {
  const bad = (why) => refuse(`it isn't a readable zip (${why})`);
  if (buf.length > limits.file) refuse("the file is too large");
  if (buf.length < 22 || buf.readUInt32LE(buf.length - 22) !== 0x06054b50) bad("no end record");
  const end = buf.length - 22;
  const count = buf.readUInt16LE(end + 10);
  const dirSize = buf.readUInt32LE(end + 12);
  const dirOffset = buf.readUInt32LE(end + 16);
  if (
    buf.readUInt16LE(end + 4) !== 0 ||
    buf.readUInt16LE(end + 6) !== 0 ||
    buf.readUInt16LE(end + 8) !== count ||
    buf.readUInt16LE(end + 20) !== 0 ||
    count === 0xffff ||
    dirSize === 0xffffffff ||
    dirOffset === 0xffffffff
  ) {
    bad("multi-disk, zip64 or commented archives aren't allowed");
  }
  if (dirOffset + dirSize !== end) bad("the directory doesn't end where the end record starts");
  if (count > limits.entries) refuse("it holds too many files");

  const entries = [];
  let at = dirOffset;
  let expectedOffset = 0;
  let total = 0;
  const seen = new Set();
  for (let n = 0; n < count; n++) {
    if (at + 46 > end || buf.readUInt32LE(at) !== 0x02014b50) bad("a directory entry is damaged");
    const madeBy = buf.readUInt16LE(at + 4);
    const flags = buf.readUInt16LE(at + 8);
    const method = buf.readUInt16LE(at + 10);
    const crc = buf.readUInt32LE(at + 16);
    const csize = buf.readUInt32LE(at + 20);
    const usize = buf.readUInt32LE(at + 24);
    const nameLen = buf.readUInt16LE(at + 28);
    const extraLen = buf.readUInt16LE(at + 30);
    const commentLen = buf.readUInt16LE(at + 32);
    const external = buf.readUInt32LE(at + 38);
    const localOffset = buf.readUInt32LE(at + 42);
    if (at + 46 + nameLen > end) bad("a directory entry is damaged");
    const nameBytes = buf.subarray(at + 46, at + 46 + nameLen);
    let name;
    try {
      name = new TextDecoder("utf-8", { fatal: true }).decode(nameBytes);
    } catch {
      refuse(`${quote(nameBytes.toString("latin1"))} isn't a plain file name`);
    }
    if (extraLen !== 0 || commentLen !== 0) bad("extra fields and comments aren't allowed");
    if (name.endsWith("/") || (external & 0x10) !== 0) refuse(`${quote(name)} is a folder entry`);
    if (madeBy >> 8 === 3) {
      const type = (external >>> 16) & 0o170000;
      if (type !== 0 && type !== 0o100000) refuse(`${quote(name)} isn't a plain file`);
    }
    checkPath(name);
    if (seen.has(name)) bad("it names one file twice");
    seen.add(name);
    if ((flags & 1) !== 0) refuse(`${quote(name)} is encrypted`);
    if ((flags & ~0x800) !== 0)
      bad("a flag that isn't allowed (data descriptors, strong encryption)");
    if (method !== 0 && method !== 8) refuse(`${quote(name)} isn't stored or deflated`);
    if (usize > limits.entry) refuse(`${quote(name)} is too large`);
    total += usize;
    if (total > limits.total) refuse("its files are too large together");
    if (method === 0 && csize !== usize) bad("a stored entry's sizes disagree");

    // The local header agrees with the directory, and the entries tile the file with no gap.
    if (localOffset !== expectedOffset) bad("entries overlap or leave a gap");
    if (localOffset + 30 + nameLen > dirOffset || buf.readUInt32LE(localOffset) !== 0x04034b50) {
      bad("a local header is damaged");
    }
    const localName = buf.subarray(localOffset + 30, localOffset + 30 + nameLen);
    const localExtra = buf.readUInt16LE(localOffset + 28);
    if (
      !localName.equals(nameBytes) ||
      localExtra !== 0 ||
      buf.readUInt16LE(localOffset + 6) !== flags ||
      buf.readUInt16LE(localOffset + 8) !== method ||
      buf.readUInt32LE(localOffset + 14) !== crc ||
      buf.readUInt32LE(localOffset + 18) !== csize ||
      buf.readUInt32LE(localOffset + 22) !== usize
    ) {
      bad("its zip headers don't agree");
    }
    const dataStart = localOffset + 30 + nameLen;
    if (dataStart + csize > dirOffset) bad("an entry runs into the directory");
    expectedOffset = dataStart + csize;
    const stored = buf.subarray(dataStart, dataStart + csize);
    let data;
    if (method === 0) data = Buffer.from(stored);
    else {
      try {
        data = inflateRawSync(stored, { maxOutputLength: usize + 1 });
      } catch {
        refuse(`${quote(name)} couldn't be read`);
      }
    }
    if (data.length !== usize || crc32(data) !== crc)
      refuse(`${quote(name)} couldn't be read (damaged)`);
    entries.push([name, data]);
    at += 46 + nameLen;
  }
  if (at !== end) bad("the directory has trailing bytes");
  if (expectedOffset !== dirOffset) bad("bytes between the entries and the directory");
  checkCaseCollisions(entries.map(([n]) => n));
  return new Map(entries);
}

// ---- Verifying a package --------------------------------------------------------------------------

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
// ignoreBOM: the decoder keeps a leading U+FEFF instead of silently dropping it, so the JSON parser
// refuses it as serde_json does (a BOM is how a Windows editor leaves a file Rust then cannot read).
const utf8 = (bytes) => new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);

/** `checksums.json`: sorted, no repeats, every path a package path outside `pmmod/`. */
export function parseChecksums(bytes) {
  const bad = (why) => refuse(`its checksums are malformed (${why})`);
  let raw;
  try {
    raw = parseStrictJson(utf8(bytes));
  } catch (e) {
    bad(e.message);
  }
  if (!isObject(raw) || Object.keys(raw).sort().join() !== "files,format") bad("unexpected fields");
  if (raw.format !== 1) bad("unknown format");
  if (!Array.isArray(raw.files)) bad("files must be a list");
  const out = new Map();
  let previous = null;
  for (const item of raw.files) {
    if (!isObject(item) || Object.keys(item).sort().join() !== "path,sha256")
      bad("unexpected fields");
    if (typeof item.path !== "string" || typeof item.sha256 !== "string") bad("not text");
    checkPath(item.path);
    if (item.path.startsWith("pmmod/")) bad(`${quote(item.path)} can't be listed`);
    if (previous !== null && previous >= item.path) bad("not sorted, or a path is listed twice");
    previous = item.path;
    if (!/^[0-9a-f]{64}$/.test(item.sha256)) bad(`${quote(item.path)}'s sha256`);
    out.set(item.path, item.sha256);
  }
  for (const required of [MANIFEST, LICENCE_FILE]) {
    if (!out.has(required)) refuse(`it has no ${required}`);
  }
  return out;
}

/** `checksums.json` for a set of files (everything but the two `pmmod/` entries). */
export function checksumsFor(files) {
  const list = [...files.keys()]
    .filter((p) => !p.startsWith("pmmod/"))
    .sort(cmp)
    .map((path) => ({ path, sha256: sha256(files.get(path)) }));
  return Buffer.from(`${JSON.stringify({ format: 1, files: list }, null, 2)}\n`);
}

/** `LICENCE.txt` holds the AGPL's text (its title and version). */
function checkLicenceText(files) {
  let text = "";
  try {
    // package.rs reads it as text and reports a failure as the same Licence refusal.
    if (files.has(LICENCE_FILE)) text = utf8(files.get(LICENCE_FILE));
  } catch {
    // left empty: refused below with the Rust wording
  }
  if (!(text.includes("GNU AFFERO GENERAL PUBLIC LICENSE") && text.includes("Version 3"))) {
    refuse(`its ${LICENCE_FILE} isn't the GNU AGPL, version 3`);
  }
}

/** The signature file's shape (four minisign lines) and its trusted comment, checked against the manifest. */
export function checkSignatureFile(bytes, manifest) {
  if (bytes.length > SIGNATURE_CAP) refuse("its signature is too large");
  let text;
  try {
    text = utf8(bytes);
  } catch {
    refuse("its signature is malformed");
  }
  const lines = text.split(/\r?\n/);
  if (lines[lines.length - 1] === "") lines.pop();
  const base64Length = (line) =>
    /^[A-Za-z0-9+/]+={0,2}$/.test(line) ? Buffer.from(line, "base64").length : -1;
  if (
    lines.length !== 4 ||
    !lines[0].startsWith("untrusted comment:") ||
    !lines[2].startsWith("trusted comment: ") ||
    base64Length(lines[1]) !== 74 ||
    base64Length(lines[3]) !== 64
  ) {
    refuse("its signature is malformed");
  }
  const comment = lines[2].slice("trusted comment: ".length);
  const bad = (why) => refuse(`its signed description is malformed (${why})`);
  const parts = comment.split("\t");
  if (parts.length !== TRUSTED_KEYS.length) bad("wrong number of fields");
  const values = parts.map((part, n) => {
    const prefix = `${TRUSTED_KEYS[n]}:`;
    if (!part.startsWith(prefix)) bad(`expected ${TRUSTED_KEYS[n]}`);
    const value = part.slice(prefix.length);
    if (value === "" || /\p{Cc}/u.test(value))
      bad(`${TRUSTED_KEYS[n]} is empty or has a control character`);
    return value;
  });
  if (values[0] !== "1") bad("not a mod's signature");
  if (values[3] !== "first" && values[3] !== "second") bad("tier");
  if (!isCommit(values[6])) bad("commit isn't 40 lowercase hex digits");
  const pairs = [
    ["id", values[1], manifest.id],
    ["version", values[2], manifest.version],
    ["tier", values[3], manifest.tier],
    ["author", values[4], manifest.author],
    ["source repo", values[5], manifest.sourceRepo],
    ["commit", values[6], manifest.commit],
    ["API range", values[7], manifest.api],
  ];
  for (const [what, signed, fromManifest] of pairs) {
    if (signed !== fromManifest) refuse(`its manifest's ${what} isn't the one that was signed`);
  }
}

/**
 * HTML's whitespace is these five characters only. JS's `\s` also matches a no-break space, a
 * vertical tab, U+2028 and U+FEFF, which a browser reads as part of a name or an unquoted value:
 * a tokenizer that skips them sees a quote where the browser sees none, and so a different end of
 * the tag. Every whitespace test that models a start tag uses this set, never `\s`.
 */
const isHtmlSpace = (ch) => ch !== undefined && /^[\t\n\f\r ]$/.test(ch);

/**
 * The attributes of one start tag whose name ends at `from`, and where the tag ends: a quoted
 * attribute value may hold ">", an attribute may follow a closing quote or a "/" with no space.
 * `closed` is false for a tag the text ends inside.
 */
function readStartTag(html, from) {
  const attrs = [];
  const n = html.length;
  let i = from;
  let closed = false;
  while (i < n) {
    while (i < n && (html[i] === "/" || isHtmlSpace(html[i]))) i++;
    if (i >= n) break;
    if (html[i] === ">") {
      closed = true;
      i++;
      break;
    }
    const nameAt = i++; // a leading "=" belongs to the name, as in HTML
    while (i < n && !isHtmlSpace(html[i]) && !"/>=".includes(html[i])) i++;
    const name = html.slice(nameAt, i).toLowerCase();
    let value = "";
    let j = i;
    while (j < n && isHtmlSpace(html[j])) j++;
    if (html[j] === "=") {
      j++;
      while (j < n && isHtmlSpace(html[j])) j++;
      if (html[j] === '"' || html[j] === "'") {
        const end = html.indexOf(html[j], j + 1);
        value = html.slice(j + 1, end < 0 ? n : end);
        i = end < 0 ? n : end + 1;
      } else {
        const valueFrom = j;
        while (j < n && !isHtmlSpace(html[j]) && html[j] !== ">") j++;
        value = html.slice(valueFrom, j);
        i = j;
      }
    }
    attrs.push({ name, value });
  }
  return { attrs, end: i, closed };
}

/**
 * Every `<` plus a letter in HTML text read as the start of a tag, by `readStartTag` rather than a
 * `<[^>]*>` pattern, and with every name counted. It models this one corner of a browser's
 * tokenizer (attributes, quotes, slashes), not the whole parser, so it does not know a comment, a
 * title or a textarea from markup: a `<a b='` inside a comment would swallow the real tags up to a
 * quote in a later comment if each read skipped to its own end. So the reads OVERLAP: the search
 * resumes right after the tag's NAME, never after its end, and each candidate tag is checked from
 * its own start. A `<x` inside a quoted value or a comment is read too, which can only ever refuse
 * more (fail closed). A generator, so the first refusal stops the work.
 *
 * Overlapping reads can cost quadratic time on markup whose every read runs on to a far quote or
 * `>` (`<a b='` repeated, or `<a ` repeated with no `>`). Built output never looks like that (each
 * tag ends at its own `>`, so the reads add up to about the file's length), so the reads share a
 * budget and markup that spends past it is refused as too tangled to check (`tangled: true`), never
 * left to run for minutes. This is a lint for the author's slips; the frame's CSP is the guard.
 */
export const TAG_READ_BUDGET = (length) => 64 * length + 65536;
function* startTags(html) {
  const open = /<([a-zA-Z][^\t\n\f\r />]*)/g;
  const budget = TAG_READ_BUDGET(html.length);
  let spent = 0;
  for (let m = open.exec(html); m !== null; m = open.exec(html)) {
    const { attrs, end, closed } = readStartTag(html, open.lastIndex);
    spent += end - m.index;
    if (spent > budget) {
      yield { name: m[1].toLowerCase(), attrs: [], end, closed: true, tangled: true };
      return;
    }
    yield { name: m[1].toLowerCase(), attrs, end, closed };
  }
}

/**
 * An attribute value with its numeric character references (any letter or digit can be spelled one)
 * and the few named ones that matter for a URL decoded.
 */
function decodeReferences(value) {
  const named = { colon: ":", tab: "\t", newline: "\n", sol: "/", bsol: "\\" };
  return value.replace(
    /&#x([0-9a-f]{1,6});?|&#([0-9]{1,7});?|&(colon|tab|newline|sol|bsol);/gi,
    (_, h, d, w) => {
      if (w) return named[w.toLowerCase()];
      const code = h ? parseInt(h, 16) : Number(d);
      return code <= 0x10ffff ? String.fromCodePoint(code) : "";
    },
  );
}

/** An attribute value as a browser reads it for a URL: character references decoded, tabs and newlines gone. */
function urlText(value) {
  const decoded = decodeReferences(value).replace(/[\t\n\r]/g, "");
  // A URL's leading C0 controls and spaces are stripped (a char-code loop: a control-character
  // regex is a lint error).
  let start = 0;
  while (start < decoded.length && decoded.charCodeAt(start) <= 0x20) start++;
  return decoded.slice(start);
}

/**
 * Said with every refusal of a raw-text rule below: those rules read the file as plain text, with no
 * notion of a comment (a hand-rolled comment stripper is what three review rounds kept finding holes
 * in), so a comment that merely mentions the markup is refused too. That is deliberate.
 */
const RAW_RULE =
  "this check reads the raw text, comments included: remove a comment that mentions it";

/**
 * The script rule, on the RAW text: every `<script` (any case, anywhere, inside a comment or a quoted
 * value too) must be the one shape a Vite build emits after the mods' own rewrite: a classic script
 * with a `src`, nothing inside it, and `</script>` right after (whitespace allowed between). Anything
 * else, however it is dressed up, is refused rather than interpreted.
 */
function checkScripts(html, where) {
  const re = /<script/gi;
  for (let m = re.exec(html); m !== null; m = re.exec(html)) {
    const after = html[m.index + 7];
    if (after !== undefined && after !== "/" && after !== ">" && !isHtmlSpace(after)) {
      where(`has a "<script" that isn't a script tag (${RAW_RULE})`);
    }
    const tag = readStartTag(html, m.index + 7);
    if (!tag.closed) where("has a script tag that never ends");
    const close = /^[\t\n\f\r ]*<\/script>/i.exec(html.slice(tag.end));
    if (!close) {
      where(
        /<\/script/i.test(html.slice(tag.end))
          ? `has an inline script (${RAW_RULE})`
          : `has an unclosed script tag (${RAW_RULE})`,
      );
    }
    const all = (name) => tag.attrs.filter((a) => a.name === name).map((a) => a.value);
    if (all("src").length === 0) where(`has an inline script (a script with no src; ${RAW_RULE})`);
    // The first `type` is the one a browser reads; an allow-list, so anything odd is refused.
    const type = (all("type")[0] ?? "").toLowerCase();
    if (!["", "text/javascript", "application/javascript"].includes(type)) {
      where(`has a script of type ${quote(type)} (only classic scripts load in the frame)`);
    }
    re.lastIndex = tag.end + close[0].length;
  }
}

/**
 * The rules for one HTML file (`checkBuiltFile` calls it): raw-text rules first (`checkScripts`,
 * then a `<style`, `crossorigin` or `type=module` anywhere in the text), then the start tags.
 */
function checkBuiltHtml(html, where) {
  checkScripts(html, where);
  if (/<style/i.test(html)) where(`has an inline style element (${RAW_RULE})`);
  if (/crossorigin/i.test(html)) where(`has a crossorigin attribute (${RAW_RULE})`);
  if (/type\s*=\s*["']?\s*module/i.test(html)) {
    where(`has a type=module (only classic scripts load in the frame; ${RAW_RULE})`);
  }
  for (const tag of startTags(html)) {
    if (tag.tangled) where("is too tangled for this check to read (its tags run into each other)");
    if (!tag.closed) where("has a tag that never ends");
    const all = (name) => tag.attrs.filter((a) => a.name === name).map((a) => a.value);
    const names = tag.attrs.map((a) => a.name);
    if (names.includes("style")) where("has an inline style attribute");
    if (names.some((a) => a.startsWith("on"))) where("has an inline event handler");
    // `rel` as a browser reads it: references decoded, split on HTML whitespace.
    if (
      all("rel").some((v) =>
        decodeReferences(v)
          .toLowerCase()
          .split(/[\t\n\f\r ]+/)
          .includes("modulepreload"),
      )
    ) {
      where("preloads a module");
    }
    // A `srcdoc` is a whole HTML document held in a character-reference-encoded attribute, which
    // the raw `<script` rule cannot see; nothing a mod ships needs one.
    if (names.includes("srcdoc"))
      where("has a srcdoc attribute (its HTML is hidden from this check)");
    for (const name of ["href", "src", "action", "formaction", "xlink:href"]) {
      if (all(name).some((v) => /^javascript:/i.test(urlText(v)))) where("has a javascript: URL");
    }
    // What a page pulls in must be the mod's own file. The frame's CSP is what holds at run time;
    // this catches a build that emitted a CDN link before it ships and then fails silently.
    const loads = tag.name === "script" ? "src" : tag.name === "link" ? "href" : null;
    for (const value of loads ? all(loads) : []) {
      const url = urlText(value);
      if (/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(url) || /^[\\/]{2}/.test(url)) {
        where(
          `has a ${tag.name} that loads ${quote(value)}, which isn't one of the mod's own files`,
        );
      }
    }
  }
}

/**
 * An AUTHOR'S LINT for built output, not a security boundary. package.rs does not look at HTML (a
 * package is data to it), and what stops inline script and style in every engine is the frame's CSP
 * (src-tauri/src/mods/headers.rs: script-src and style-src are the mod's own app/ folder, no
 * 'unsafe-inline'). This catches a build that went wrong or an author's slip before it ships and
 * then fails silently. So it is broad and fails closed instead of modelling a browser: for HTML the
 * script, style, crossorigin and type=module rules read the RAW text with no comment handling at all
 * (a comment that mentions `<script` is refused on purpose: Vite's built output carries no comment
 * with a tag in it, so remove the comment), and only the per-attribute rules use a small start-tag
 * tokenizer (`startTags`). CSS and JS are text rules.
 */
export function checkBuiltFile(path, bytes) {
  const where = (why) => refuse(`${quote(path)} ${why}`);
  const isHtml = extensionIs(path, "html") || extensionIs(path, "htm");
  const isCss = extensionIs(path, "css");
  if (!isHtml && !isCss && !extensionIs(path, "js")) return;
  let text;
  try {
    text = utf8(bytes);
  } catch {
    where("isn't UTF-8 text");
  }
  if (isHtml) checkBuiltHtml(text, where);
  else if (isCss) {
    if (/url\(\s*["']?\s*data:/i.test(text)) {
      where("has a data: URL (the frame's CSP allows only the mod's own files)");
    }
  } else {
    try {
      // Compiled as a classic script and never run: `import` or `export` at the top is a syntax error.
      new vm.Script(text, { filename: path });
    } catch (e) {
      where(`isn't a classic script (${String(e.message).split("\n").pop()})`);
    }
  }
}

/**
 * Verify a package's bytes. `kind` is "signed" (a signature file is present), "pre-sign" (a first or
 * second party package without one: what the build job hands the sign job) or "unsigned" (third
 * party, a developer's own). The signature itself is not checked, only its shape and its agreement
 * with the manifest.
 */
export function verifyArchive(bytes) {
  const entries = readZip(bytes);
  const signed = entries.has(SIGNATURE);
  if (!entries.has(CHECKSUMS)) refuse(`it has no ${CHECKSUMS}`);
  if (
    entries.get(CHECKSUMS).length > CHECKSUMS_CAP ||
    (entries.has(MANIFEST) && entries.get(MANIFEST).length > MANIFEST_CAP)
  ) {
    refuse("a file in it is too large");
  }
  const listed = parseChecksums(entries.get(CHECKSUMS));
  const expected = new Set([...listed.keys(), CHECKSUMS]);
  if (signed) expected.add(SIGNATURE);
  for (const name of entries.keys()) {
    if (!expected.has(name)) refuse(`it holds ${quote(name)} that its checksums don't list`);
  }
  for (const name of expected) {
    if (!entries.has(name)) refuse(`its checksums list ${quote(name)}, which it doesn't hold`);
  }
  for (const [name, want] of listed) {
    if (sha256(entries.get(name)) !== want) {
      refuse(`${quote(name)} isn't the file its checksums list`);
    }
  }
  const files = new Map([...entries].filter(([n]) => n !== CHECKSUMS && n !== SIGNATURE));
  let manifestText;
  try {
    manifestText = utf8(files.get(MANIFEST));
  } catch {
    manifestError("not UTF-8");
  }
  let raw;
  try {
    raw = parseStrictJson(manifestText);
  } catch (e) {
    manifestError(e.message);
  }
  const manifest = checkManifest(raw, new Set(files.keys()));
  checkTier(manifest.id, manifest.tier);
  if (signed) checkSignatureFile(entries.get(SIGNATURE), manifest);
  checkLicenceText(files);
  for (const [name, data] of files) {
    if (name.startsWith(APP_DIR)) checkBuiltFile(name, data);
  }
  const kind = signed ? "signed" : manifest.tier === "third" ? "unsigned" : "pre-sign";
  return { kind, manifest, entries, files };
}

// ---- Bundled packages and NOTICE --------------------------------------------------------------------

/**
 * The package directory a bundled module path belongs to: from the repo root, up to and including
 * the package's own folder after the LAST `node_modules` segment (a nested copy is its own package;
 * a scoped name takes two segments). Refuses a path that leaves the repo or isn't one.
 */
export function packageDirOf(modulePath) {
  const parts = modulePath.split("/");
  // An absolute path has an empty first segment, so it is refused here too.
  if (parts.some((p) => p === "" || p === "." || p === "..")) {
    refuse(`a bundled module path ${quote(modulePath)} leaves the repository`);
  }
  const at = parts.lastIndexOf("node_modules");
  if (at < 0 || at + 1 >= parts.length) refuse(`${quote(modulePath)} isn't inside a package`);
  const scoped = parts[at + 1].startsWith("@");
  const end = at + (scoped ? 3 : 2);
  if (end > parts.length - 1) refuse(`${quote(modulePath)} isn't inside a package`);
  return parts.slice(0, end).join("/");
}

/** The digest `mods/vite.config.ts` writes (see `outputDigest` there; a test pins the two together). */
export function outputDigest(dir) {
  const files = [];
  const walk = (rel) => {
    for (const entry of readdirSync(join(dir, rel), { withFileTypes: true })) {
      const next = rel === "" ? entry.name : `${rel}/${entry.name}`;
      if (entry.isDirectory()) walk(next);
      else files.push(next);
    }
  };
  walk("");
  files.sort(cmp);
  const outer = createHash("sha256");
  for (const f of files) outer.update(`${f}\0${sha256(readFileSync(join(dir, f)))}\n`);
  return outer.digest("hex");
}

/**
 * The npm packages a mod's bundle contains, from `<mod>/.build/modules.json`, each with its licence
 * checked against the shipped-package allow-list and its text read from node_modules. Any gap is a
 * refusal: a NOTICE that omits a package reads as compliance.
 */
export function bundledPackages(modDir, root) {
  const recordPath = join(modDir, ".build", "modules.json");
  if (!existsSync(recordPath)) {
    refuse(`${recordPath} is missing: build the mod first (\`just build-mods\`)`);
  }
  let record;
  try {
    record = parseStrictJson(readFileSync(recordPath, "utf8"));
  } catch (e) {
    refuse(`${recordPath}: ${e.message}`);
  }
  if (
    !isObject(record) ||
    record.format !== 1 ||
    typeof record.bundle !== "string" ||
    !Array.isArray(record.modules) ||
    !record.modules.every((m) => typeof m === "string")
  ) {
    refuse(`${recordPath} isn't a module record this packer reads`);
  }
  const built = outputDigest(join(modDir, "dist", "app"));
  if (record.bundle !== built) {
    refuse(
      "the build record (.build/modules.json) is not for the output in dist/app: the build was " +
        "interrupted or the output changed since. Build again.",
    );
  }
  const dirs = [...new Set(record.modules.map(packageDirOf))].sort(cmp);
  const packages = [];
  for (const dir of dirs) {
    const packageJson = join(root, dir, "package.json");
    if (!existsSync(packageJson))
      refuse(`${dir} is bundled but has no package.json: run \`npm ci\``);
    const meta = JSON.parse(readFileSync(packageJson, "utf8"));
    const name = dir.slice(dir.lastIndexOf("node_modules/") + "node_modules/".length);
    if (meta.name !== name)
      refuse(`${dir}/package.json names ${quote(String(meta.name))}, not ${quote(name)}`);
    if (typeof meta.version !== "string") refuse(`${name} declares no version`);
    if (typeof meta.license !== "string" || !acceptable(meta.license)) {
      refuse(
        `${name}@${meta.version} is ${JSON.stringify(meta.license ?? null)}, which is not in the ` +
          `accepted set of scripts/check-npm-licenses.mjs: it can't be bundled into a mod`,
      );
    }
    const text = licenceTextFor(join(root, dir));
    if (text === null) refuse(`${name}@${meta.version} ships no licence text to put in NOTICE.txt`);
    packages.push({ name, version: meta.version, license: meta.license, text });
  }
  return packages;
}

/** The NOTICE.txt text: fixed words, then each package in name order. LF only, no dates. */
export function noticeText(packages) {
  const header = [
    "Third-party software bundled in this mod (npm packages)",
    "=".repeat(78),
    "",
    packages.length === 0
      ? "This mod bundles no npm packages."
      : `This mod bundles ${packages.length} npm package${packages.length === 1 ? "" : "s"}. Each is ` +
        "reproduced below with the licence and copyright notice its authors ship.",
    "",
  ].join("\n");
  const blocks = [...packages]
    .sort((a, b) => cmp(a.name, b.name) || cmp(a.version, b.version))
    .map((p) =>
      [
        "-".repeat(78),
        `${p.name} ${p.version}`,
        `SPDX-License-Identifier: ${p.license}`,
        "-".repeat(78),
        "",
        p.text,
        "",
      ].join("\n"),
    );
  return Buffer.from(header + (blocks.length > 0 ? `\n${blocks.join("\n")}` : ""));
}

// ---- Packing ----------------------------------------------------------------------------------------

/** Every regular file under `dir`, as package paths with `prefix`; anything else is refused. */
function readTree(dir, prefix) {
  const out = new Map();
  const walk = (rel) => {
    const here = rel === "" ? dir : join(dir, rel);
    for (const name of readdirSync(here).sort(cmp)) {
      const relPath = rel === "" ? name : `${rel}/${name}`;
      const stat = lstatSync(join(dir, relPath));
      if (stat.isDirectory()) walk(relPath);
      else if (stat.isFile()) out.set(`${prefix}${relPath}`, readFileSync(join(dir, relPath)));
      else refuse(`${quote(`${prefix}${relPath}`)} isn't a plain file or folder`);
    }
  };
  walk("");
  return out;
}

/**
 * Everything the package holds except `pmmod/`: the manifest (with `commit` stamped when given),
 * the repo's LICENCE.txt verbatim, NOTICE.txt, and the built `app/` tree.
 */
export function assemble({ modDir, root = repoRoot, commit }) {
  const manifestPath = join(modDir, MANIFEST);
  if (!existsSync(manifestPath)) refuse(`${manifestPath} is missing`);
  let manifestBytes = readFileSync(manifestPath);
  if (manifestBytes.length > MANIFEST_CAP) refuse("the manifest is too large");
  if (commit !== undefined) {
    if (!isCommit(commit)) refuse("--commit must be 40 lowercase hex digits");
    let parsed;
    try {
      parsed = parseStrictJson(utf8(manifestBytes));
    } catch (e) {
      refuse(`manifest.json: ${e.message}`);
    }
    if (!isObject(parsed) || !isObject(parsed.publisher))
      manifestError("publisher must be an object");
    parsed.publisher.commit = commit;
    manifestBytes = Buffer.from(`${JSON.stringify(parsed, null, 2)}\n`);
  }
  const appDir = join(modDir, "dist", "app");
  if (!existsSync(appDir) || !lstatSync(appDir).isDirectory()) {
    refuse(`${appDir} is missing: build the mod first (\`just build-mods\`)`);
  }
  const app = readTree(appDir, APP_DIR);
  if (app.size === 0) refuse(`${appDir} holds no files`);
  const licencePath = join(root, LICENCE_FILE);
  if (!existsSync(licencePath)) refuse(`${licencePath} is missing`);
  const files = new Map([
    [MANIFEST, manifestBytes],
    [LICENCE_FILE, readFileSync(licencePath)],
    [NOTICE_FILE, noticeText(bundledPackages(modDir, root))],
    ...app,
  ]);
  return files;
}

/** The archive for a file set, after the rules of its mode; verified with the verifier before it is returned. */
export function buildArchive(files, { unsigned = false, signature } = {}) {
  let raw;
  try {
    raw = parseStrictJson(utf8(files.get(MANIFEST)));
  } catch (e) {
    manifestError(e.message);
  }
  const manifest = checkManifest(raw, new Set(files.keys()));
  if (unsigned) {
    if (manifest.tier !== "third") {
      refuse('an unsigned file must say it is third party (publisher.tier "third")');
    }
    if (isReservedId(manifest.id))
      refuse("an atlas. id belongs to Atlas: an unsigned file can't use one");
  } else if (manifest.tier === "third") {
    refuse(
      'a file for signing must say first or second party; a developer\'s own file is packed with --unsigned (publisher.tier "third")',
    );
  }
  const entries = new Map(files);
  entries.set(CHECKSUMS, checksumsFor(files));
  if (signature !== undefined) entries.set(SIGNATURE, signature);
  const bytes = writeZip(entries);
  verifyArchive(bytes);
  return bytes;
}

/** Refuse the all-zero placeholder `publisher.commit` of a source manifest; `how` says how to fix it. */
function refusePlaceholderCommit(manifest, how) {
  if (/^0{40}$/.test(manifest.commit)) {
    refuse(
      `publisher.commit is the all-zero placeholder, not the commit this was built from (${how}): ` +
        `a signature would vouch for a commit that does not exist`,
    );
  }
}

/** Add a signature to an archive and write it again with the one writer. */
export function attach(archive, signature) {
  const before = verifyArchive(archive);
  if (before.kind === "signed") refuse("that archive is already signed");
  if (before.kind === "unsigned") refuse("a third-party file can't be signed by an Atlas key");
  // The signature's trusted comment must equal the manifest's commit, so checking the manifest
  // covers both: a signed claim to the all-zero commit would be a claim to a commit that doesn't exist.
  refusePlaceholderCommit(before.manifest, "it was packed without --commit");
  const entries = new Map(before.entries);
  entries.set(SIGNATURE, Buffer.from(signature));
  const bytes = writeZip(entries);
  verifyArchive(bytes);
  return bytes;
}

// ---- Command line -----------------------------------------------------------------------------------

const USAGE = `usage:
  pack-mod.mjs <mod folder> [--commit <sha>] [--unsigned] [--out <file>]
  pack-mod.mjs <mod folder> --stage [--commit <sha>]
  pack-mod.mjs attach <archive> <signature file> --out <file>
  pack-mod.mjs --verify <file>`;

function parseArgs(argv) {
  const flags = new Map();
  const positional = [];
  for (let n = 0; n < argv.length; n++) {
    const a = argv[n];
    if (["--unsigned", "--stage", "--verify"].includes(a)) flags.set(a, true);
    else if (["--commit", "--out"].includes(a)) {
      if (n + 1 >= argv.length) refuse(`${a} needs a value\n${USAGE}`);
      flags.set(a, argv[++n]);
    } else if (a.startsWith("--")) refuse(`unknown option ${a}\n${USAGE}`);
    else positional.push(a);
  }
  return { flags, positional };
}

const write = (file, bytes) => {
  mkdirSync(dirname(resolve(file)), { recursive: true });
  writeFileSync(file, bytes);
  console.log(`${sha256(bytes)}  ${file}`);
};

export function run(argv, root = repoRoot) {
  const { flags, positional } = parseArgs(argv);
  const known = (...names) => {
    for (const k of flags.keys())
      if (!names.includes(k)) refuse(`${k} doesn't go with this command\n${USAGE}`);
  };
  if (flags.has("--verify")) {
    known("--verify");
    if (positional.length !== 1) refuse(USAGE);
    const { kind, manifest, files } = verifyArchive(readFileSync(positional[0]));
    console.log(
      `✓ pack-mod: ${positional[0]} is a well-formed ${kind} package of ${manifest.id} ${manifest.version} ` +
        `(${files.size} files${kind === "signed" ? "; the signature itself is checked only by PM" : ""})`,
    );
    return;
  }
  if (positional[0] === "attach") {
    known("--out");
    if (positional.length !== 3 || !flags.has("--out")) refuse(USAGE);
    write(flags.get("--out"), attach(readFileSync(positional[1]), readFileSync(positional[2])));
    return;
  }
  if (positional.length !== 1) refuse(USAGE);
  const modDir = positional[0];
  if (flags.has("--stage")) {
    known("--stage", "--commit");
    const files = assemble({ modDir, root, commit: flags.get("--commit") });
    buildArchive(files, { unsigned: isThirdParty(files) });
    for (const name of [MANIFEST, LICENCE_FILE, NOTICE_FILE]) {
      writeFileSync(join(modDir, "dist", name), files.get(name));
    }
    console.log(`✓ pack-mod: staged ${modDir}/dist (manifest.json, LICENCE.txt, NOTICE.txt, app/)`);
    return;
  }
  known("--commit", "--unsigned", "--out");
  const files = assemble({ modDir, root, commit: flags.get("--commit") });
  const bytes = buildArchive(files, { unsigned: flags.has("--unsigned") });
  const { manifest } = verifyArchive(bytes);
  // A package for signing must name the commit it was built from: the signature vouches for it.
  // Only a developer's own --unsigned file (and --stage, above) may keep the placeholder.
  if (!flags.has("--unsigned")) refusePlaceholderCommit(manifest, "pass --commit <sha>");
  write(
    flags.get("--out") ?? join(modDir, "dist", `${manifest.id}-${manifest.version}.pmmod`),
    bytes,
  );
}

/** Whether a staged manifest says third party (a stage is for any mod; the mode follows the manifest). */
function isThirdParty(files) {
  return parseStrictJson(utf8(files.get(MANIFEST))).publisher?.tier === "third";
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    run(process.argv.slice(2));
  } catch (e) {
    if (!(e instanceof PackError)) throw e;
    console.error(`✗ pack-mod: ${e.message}`);
    process.exit(1);
  }
}
