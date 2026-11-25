// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The one build config for every mod (Mods spec §7): `PM_MOD_DIR=mods/<id> vite build --config
// mods/vite.config.ts` (`just build-mods` loops over the mod folders). The folder's `src/` is the Vite
// root, and the output is `<mod>/dist/app/`, the `app/` of the package; `scripts/pack-mod.mjs` then
// writes the rest of the package around it.
//
// A mod is a sealed frame (src-tauri/src/mods/headers.rs), and the shape of the output follows from
// that policy, measured in the B0 spikes (build log, "PR B B0"):
//   - ONE classic script. The frame's CSP is `script-src <the app folder>`, a package may not use a
//     module script (PM refuses nothing about it, but `pack-mod --verify` does), so the bundle is an
//     IIFE, one chunk, one external stylesheet, fixed file names the manifest and the page can name.
//   - Vite still writes `<script type="module" crossorigin>` and `crossorigin` on the stylesheet link
//     into the page, whatever the output format, so `classicHtml` rewrites the tags (and fails the
//     build if a tag it did not expect is left).
//   - No `data:` URLs: the CSP's `img-src`/`font-src` are the app folder only, and Vite's default
//     inlines a small asset as a `data:` URI, so the inline limit is 0.
//   - No fetch from the frame (`connect-src 'none'`), so a Worker cannot be loaded from a file by URL
//     either. A worker is built with a nested Vite build and becomes ONE string literal in the page
//     (`import source from "inline-worker:./w.ts"`), which the page turns into a blob Worker. Vite's
//     own `?worker&inline` builds the Blob from an `atob` call instead, which a review lint for
//     "Blob built from non-literal data" would flag; this form is a plain string literal.
//
// The NOTICE of a package must name every npm package the bundle contains, so the bundle's module ids
// are written to `<mod>/.build/modules.json` (outside `dist/`: it is a build record, not a part of
// the package). A worker is a separate nested build whose modules the outer build cannot see, so its
// ids are collected into the same set. The record also carries a digest of the output it describes, so
// a stale record (a build that failed halfway, a hand-edited dist) is refused by `pack-mod` rather
// than writing a NOTICE that omits a package.

// This repo has no @types/node (the root vite.config.ts says the same of `process`), so the Node
// imports below are untyped on purpose, and each suppression turns into an error the day the types
// are installed, which is the cue to remove it.
// @ts-expect-error no @types/node
import { createHash } from "node:crypto";
// @ts-expect-error no @types/node
import { mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
// @ts-expect-error no @types/node
import { dirname, join, relative, resolve, sep } from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { build as viteBuild, defineConfig, type Plugin, type UserConfig } from "vite";

// @ts-expect-error no @types/node (import.meta.dirname)
const repoRoot = resolve(import.meta.dirname, "..");

/** What `modules.json` is called and where (next to `dist/`, never inside it). */
export const MODULES_RECORD = join(".build", "modules.json");

/** The inline-worker import prefix a mod's source uses. */
const WORKER_PREFIX = "inline-worker:";

/**
 * A digest of an output folder, spelled the same way `scripts/pack-mod.mjs` spells it (the two are
 * pinned together by `scripts/mods-build.test.mjs`: a packer that disagreed would refuse every build):
 * sha256 over `<path>\0<sha256 of the file>\n` for each file, paths relative to the folder, `/`
 * separated, in byte order.
 */
export function outputDigest(dir: string): string {
  const files: string[] = [];
  const walk = (rel: string) => {
    for (const entry of readdirSync(join(dir, rel), { withFileTypes: true })) {
      const next = rel === "" ? entry.name : `${rel}/${entry.name}`;
      if (entry.isDirectory()) walk(next);
      else files.push(next);
    }
  };
  walk("");
  files.sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
  const outer = createHash("sha256");
  for (const f of files) {
    const inner = createHash("sha256")
      .update(readFileSync(join(dir, f)))
      .digest("hex");
    outer.update(`${f}\0${inner}\n`);
  }
  return outer.digest("hex");
}

/**
 * The repo-relative, `/`-separated path of a bundled module that lives in a `node_modules`, or null
 * for anything else (the mod's own sources, the html and css entries, rolldown's virtual modules).
 * The ids are absolute and machine-specific, so they are made relative before they are written: two
 * checkouts must write the same record.
 */
export function packageModulePath(id: string): string | null {
  // `\0` marks a virtual module and `?…` a loader suffix (`?commonjs-module`); neither is a path.
  const bare = id.replace(/^\0/, "").replace(/\?.*$/, "");
  const path = relative(repoRoot, bare).split(sep).join("/");
  return path.split("/").includes("node_modules") ? path : null;
}

/**
 * The one chunk of a worker's nested build. A worker becomes a single string literal, so a build that
 * produced two chunks (a future Vite that splits where it inlines today: a dynamic import is inlined
 * now, measured in B0) is refused, not half-bundled.
 */
export function onlyChunk(out: Awaited<ReturnType<typeof viteBuild>>) {
  const outputs = Array.isArray(out) ? out : [out];
  const chunks = outputs
    .flatMap((o) => ("output" in o ? o.output : []))
    .filter((o) => o.type === "chunk");
  if (chunks.length !== 1) throw new Error("a mod's worker must build to exactly one chunk");
  return chunks[0];
}

function inlineWorker(collected: Set<string>): Plugin {
  return {
    name: "mod-inline-worker",
    enforce: "pre",
    async resolveId(source, importer) {
      if (!source.startsWith(WORKER_PREFIX)) return null;
      const resolved = await this.resolve(source.slice(WORKER_PREFIX.length), importer, {
        skipSelf: true,
      });
      return resolved ? `\0${WORKER_PREFIX}${resolved.id}` : null;
    },
    async load(id) {
      if (!id.startsWith(`\0${WORKER_PREFIX}`)) return null;
      const entry = id.slice(`\0${WORKER_PREFIX}`.length);
      // `import { build } from "vite"`, not rolldown: vite is the declared dependency.
      const out = await viteBuild({
        configFile: false,
        logLevel: "silent",
        envDir: false,
        build: {
          write: false,
          minify: true,
          target: "es2022",
          lib: { entry, formats: ["iife"], name: "w", fileName: "w" },
        },
      });
      const chunk = onlyChunk(out);
      for (const m of chunk.moduleIds) collected.add(m);
      this.addWatchFile(entry);
      return `export default ${JSON.stringify(chunk.code)};`;
    },
  };
}

/**
 * Vite writes `<script type="module" crossorigin src=…>` into the page; the frame loads a classic
 * script. Rewritten after every other html transform, and if any module script or `crossorigin`
 * survives the rewrite the build fails: the shape of Vite's tags is not a promise, and a silently
 * unrewritten tag would be a page whose script the frame cannot run.
 */
function classicHtml(): Plugin {
  return {
    name: "mod-classic-html",
    transformIndexHtml: {
      order: "post",
      handler(html) {
        const out = html
          .replace(/<script type="module" crossorigin src=/g, "<script defer src=")
          .replace(/ crossorigin(?=[\s>/])/g, "");
        if (/type\s*=\s*["']?module/i.test(out) || /crossorigin/i.test(out)) {
          throw new Error(
            "a module script or a crossorigin attribute is left in the page after the classic-script rewrite",
          );
        }
        return out;
      },
    },
  };
}

/**
 * The stylesheet with its comments removed. A `/*` inside a quoted string is not a comment opener, so
 * the scan walks the text and copies a string through whole: a regex over the raw text would take
 * `a{content:"/*"} @import "./x.css"; b{content:"*\/"}` for one long comment and miss the `@import`
 * between the two strings, while Tailwind (which reads strings as strings) would follow it. A string
 * ends at its closing quote, or at a newline (an unterminated string is an error to Tailwind). A
 * backslash outside a string escapes the next character, so an escaped slash before a star opens no
 * comment, and a directive between two such pairs is still read.
 */
export function withoutComments(css: string): string {
  let out = "";
  let i = 0;
  while (i < css.length) {
    const c = css[i];
    if (c === "/" && css[i + 1] === "*") {
      const end = css.indexOf("*/", i + 2);
      i = end === -1 ? css.length : end + 2;
    } else if (c === '"' || c === "'") {
      let j = i + 1;
      while (j < css.length && css[j] !== c && css[j] !== "\n") j += css[j] === "\\" ? 2 : 1;
      out += css.slice(i, j + 1);
      i = j + 1;
    } else if (c === "\\") {
      // An escape outside a string takes the next character whole: a backslash, a slash and a star
      // are an escaped slash and a star, not a comment opener, exactly as Tailwind reads them.
      out += css.slice(i, i + 2);
      i += 2;
    } else {
      out += c;
      i += 1;
    }
  }
  return out;
}

/**
 * What Tailwind may scan for class names in a mod's build: the kit (`src/kit`, the only part of PM a
 * mod imports) and the mod's own `src/`, test files excluded, and nothing else. Tailwind's own
 * default is the folder of the stylesheet (it would miss the kit), and an `@source` written by hand
 * could name any folder (it would pull core's classes into a mod's CSS). So a mod's stylesheet says
 * only `@import "tailwindcss";` and this plugin writes the rest: `source(none)` switches the automatic
 * scan off, and the `@source` lines name the two folders. Any other form of a Tailwind import, an
 * `@source` of the mod's own, or any other `@import`, `@config`, `@plugin` or `@reference` (the file
 * they load never reaches this plugin) fails the build, as `classicHtml` does for a script tag it did not expect.
 */
export function tailwindScan(modSrc: string): Plugin {
  const folders = [join(repoRoot, "src", "kit"), modSrc].map((d) => d.split(sep).join("/"));
  const inModSrc = (id: string) => {
    const file = id.replace(/\?.*$/, "").split(sep).join("/");
    return file.startsWith(`${folders[1]}/`);
  };
  const scan = folders
    .flatMap((d) => [`@source "${d}";`, `@source not "${d}/**/*.test.*";`])
    .join("\n");
  return {
    name: "mod-tailwind-scan",
    enforce: "pre",
    transform(code, id) {
      if (!id.replace(/\?.*$/, "").endsWith(".css")) return null;
      // A comment is not code: the stylesheet may explain these directives without using them.
      const live = withoutComments(code);
      if (/@source\b/i.test(live)) {
        throw new Error(
          "a mod's stylesheet may not use @source: the build names the folders Tailwind scans",
        );
      }
      const imports = live.match(/@import[^;]*tailwindcss[^;]*;/g) ?? [];
      if (
        imports.length > 1 ||
        (imports.length === 1 && !/^@import\s+(["'])tailwindcss\1\s*;$/.test(imports[0]))
      ) {
        throw new Error(
          'a mod\'s stylesheet imports Tailwind as one `@import "tailwindcss";`: the build limits what it scans',
        );
      }
      // Tailwind resolves a stylesheet's own `@import` itself, from disk: the imported file never
      // reaches this hook, so an `@source` (or a second Tailwind import) inside it would go unseen. A
      // mod's own stylesheet therefore imports nothing but the one Tailwind import, and names no
      // `@config`, `@plugin` or `@reference` (each loads another file the same way).
      if (inModSrc(id)) {
        const other = live.replace(/@import\s+(["'])tailwindcss\1\s*;/gi, "");
        if (/@(?:import|config|plugin|reference)\b/i.test(other)) {
          throw new Error(
            "a mod's stylesheet may not use @import (but for Tailwind), @config, @plugin or @reference: the build cannot see the file they load",
          );
        }
      }
      if (imports.length === 0) return null;
      const bare = /@import\s+(["'])tailwindcss\1\s*;/;
      return {
        code: code.replace(bare, `@import "tailwindcss" source(none);\n${scan}`),
        map: null,
      };
    },
  };
}

/** Writes the record of which node_modules files the bundle (and its workers) contains. */
function moduleRecord(modDir: string, outDir: string, collected: Set<string>): Plugin {
  return {
    name: "mod-module-record",
    generateBundle(_options, bundle) {
      for (const item of Object.values(bundle)) {
        if (item.type !== "chunk") continue;
        for (const id of item.moduleIds) collected.add(id);
      }
    },
    writeBundle() {
      const modules = new Set<string>();
      for (const id of collected) {
        const path = packageModulePath(id);
        if (path !== null) modules.add(path);
      }
      const record = {
        format: 1,
        bundle: outputDigest(outDir),
        modules: [...modules].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0)),
      };
      const file = join(modDir, MODULES_RECORD);
      mkdirSync(dirname(file), { recursive: true });
      writeFileSync(file, `${JSON.stringify(record, null, 2)}\n`);
    },
  };
}

/** The Vite config for the mod in `modDir` (a folder holding `manifest.json` and `src/`). */
export function modConfig(modDir: string): UserConfig {
  const dir = resolve(modDir);
  const outDir = join(dir, "dist", "app");
  const collected = new Set<string>();
  return {
    root: join(dir, "src"),
    base: "./",
    // Nothing from the developer's machine reaches the bundle: no .env files, no public/ copy.
    envDir: false,
    publicDir: false,
    // React (and any package that branches on it) picks its production build from this, and Vite reads
    // NODE_ENV from the developer's shell first: a shell with NODE_ENV=development, or a test runner,
    // would bundle React's development build, a different (and larger) bundle and a different
    // modules.json from the same sources. A mod ships production code whatever the environment says.
    define: { "process.env.NODE_ENV": JSON.stringify("production") },
    // The same for JSX: Vite compiles it as development JSX (`jsxDEV`, with the source file's absolute
    // path in every call) unless NODE_ENV says production, which is the other half of that rule.
    oxc: { jsx: { development: false } },
    plugins: [
      inlineWorker(collected),
      react(),
      tailwindScan(join(dir, "src")),
      tailwindcss(),
      moduleRecord(dir, outDir, collected),
      classicHtml(),
    ],
    build: {
      outDir,
      emptyOutDir: true,
      target: "es2022",
      minify: true,
      sourcemap: false,
      cssCodeSplit: false,
      // 0: a `data:` URI is blocked by the frame's CSP (img-src and font-src are the app folder).
      assetsInlineLimit: 0,
      modulePreload: false,
      reportCompressedSize: false,
      rolldownOptions: {
        output: {
          format: "iife",
          // Fixed names: the page and the manifest name these files, and a hashed name would change
          // with every edit of a dependency for no benefit (the package is hashed as a whole).
          entryFileNames: "assets/app.js",
          assetFileNames: "assets/[name][extname]",
        },
      },
    },
  };
}

export default defineConfig(() => {
  // @ts-expect-error process is a nodejs global
  const modDir = process.env.PM_MOD_DIR;
  if (!modDir) {
    throw new Error("PM_MOD_DIR names the mod folder to build, e.g. PM_MOD_DIR=mods/atlas.map");
  }
  return modConfig(modDir);
});
