// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The page, read as text, against the frame it runs in. The frame's CSP (src-tauri/src/mods/headers.rs)
// allows scripts, styles, images and fonts only from the mod's own folder and has `connect-src 'none'`,
// and `pack-mod --verify` lints the BUILT output; these tests hold the SOURCE to the same walls, so a
// breach is found in the unit run and not first in a live frame. The ESLint block for mods already bans
// the HTML sinks, Tauri, dynamic import and imports from PM's src/ (except src/kit); this adds what no
// lint rule says: no network, no storage of its own, no style attribute, no colour literal in the page.

import { describe, expect, it } from "vitest";
// @ts-expect-error no @types/node (this repo has none: the root vite.config.ts says the same)
import { readFileSync } from "node:fs";
import indexHtml from "./index.html?raw";

// Read from disk: vitest stubs every CSS import, `?raw` included, to an empty string (src/theme/
// designGuards.test.ts says the same), and a guard that cannot read the file it is about guards nothing.
const styleCss: string = readFileSync(new URL("./style.css", import.meta.url), "utf8");

const sources = import.meta.glob("./*.ts", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

/** The files that ship in the page's bundle: every module but a test or a test-only helper. */
const TEST_ONLY = /\.test\.ts$|\/(testing|memoryHost|domTesting|canvasTesting)\.ts$/;
const shipped = Object.entries(sources).filter(([path]) => !TEST_ONLY.test(path));
const named = (name: string) => {
  const hit = shipped.find(([path]) => path === `./${name}`);
  if (!hit) throw new Error(`no shipped source ${name}`);
  return hit[1];
};

/** Source with comments removed, so a sentence about a ban is not read as a breach of it. */
const code = (source: string) =>
  source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:"'`])\/\/.*$/gm, "$1");

describe("what ships in the page", () => {
  it("is the modules the page imports, and not a test helper", () => {
    const names = shipped.map(([path]) => path.slice(2)).sort();
    expect(names).toEqual(
      expect.arrayContaining([
        "app.ts",
        "boot.ts",
        "changes.ts",
        "compute.ts",
        "copy.ts",
        "draw.ts",
        "main.ts",
        "palette.ts",
        "shell.ts",
        "stage.ts",
        "theme.ts",
        "view.ts",
      ]),
    );
    for (const n of names) expect(n).not.toMatch(/test|Testing|memoryHost/);
  });

  it("reaches no network, no browser storage and no other window", () => {
    const banned: [RegExp, string][] = [
      [/\bfetch\s*\(/, "fetch"],
      [/\bXMLHttpRequest\b/, "XMLHttpRequest"],
      [/\bWebSocket\b/, "WebSocket"],
      [/\bEventSource\b/, "EventSource"],
      [/\bsendBeacon\b/, "sendBeacon"],
      [/\blocalStorage\b/, "localStorage"],
      [/\bsessionStorage\b/, "sessionStorage"],
      [/\bindexedDB\b/, "indexedDB"],
      [/\bdocument\.cookie\b/, "cookies"],
      [/\bwindow\.open\b/, "window.open"],
      [/\blocation\.(href|assign|replace)\b/, "navigation"],
      [/\beval\s*\(/, "eval"],
      [/\bnew\s+Function\b/, "new Function"],
      [/\bimportScripts\b/, "importScripts"],
    ];
    for (const [path, source] of shipped) {
      for (const [pattern, what] of banned) {
        expect(pattern.test(code(source)), `${path} uses ${what}`).toBe(false);
      }
    }
  });

  it("keeps what the layout workers run inside what the frame's CSP allows", () => {
    // The Map starts two blob workers (one for the by-project layout, one for PCA and t-SNE). The frame
    // allows `worker-src blob:` (any number of them) and nothing a worker could add: no WebAssembly, no
    // shared memory, no code built from text, no network. The files a worker runs are in `shipped`, so
    // the walls above already read them; this names them, so a rename cannot drop them from that check.
    const headers: string = readFileSync(
      new URL("../../../src-tauri/src/mods/headers.rs", import.meta.url),
      "utf8",
    );
    expect(headers).toMatch(/connect-src 'none';[\s\S]*?worker-src blob:/);
    for (const name of ["tsne.ts", "pca.ts", "workerProtocol.ts", "forceLayout.ts"]) {
      const text = code(named(name));
      expect(/\bWebAssembly\b|\bSharedArrayBuffer\b|\bAtomics\b/.test(text), name).toBe(false);
      expect(/\bnew\s+Function\b|\beval\s*\(|\bimportScripts\b|\bfetch\s*\(/.test(text), name).toBe(
        false,
      );
    }
  });

  it("makes a Blob and a Worker nowhere (the kit's spawnWorker is the one place)", () => {
    for (const [path, source] of shipped) {
      expect(/\bnew\s+(Blob|Worker|SharedWorker)\b/.test(code(source)), path).toBe(false);
      expect(/createObjectURL/.test(code(source)), path).toBe(false);
    }
  });

  it("sets style only through the CSSOM, and only in theme.ts (never the style attribute)", () => {
    for (const [path, source] of shipped) {
      const text = code(source);
      expect(/setAttribute\(\s*["']style["']/.test(text), `${path} writes a style attribute`).toBe(
        false,
      );
      expect(/\.cssText\b/.test(text), `${path} writes cssText`).toBe(false);
      expect(/\.style\./.test(text) || /\.style\[/.test(text), `${path} sets a style`).toBe(
        path === "./theme.ts",
      );
      expect(
        /insertRule|replaceSync|adoptedStyleSheets|createElement\("style"\)/.test(text),
        path,
      ).toBe(false);
    }
    // The one place it does: PM's variables and the colour scheme, on the root.
    const theme = code(named("theme.ts"));
    expect(theme).toContain("root.style.setProperty(name, value)");
    expect(theme).toContain("root.style.colorScheme");
  });

  it("builds its page from nodes, never from markup", () => {
    for (const [path, source] of shipped) {
      const text = code(source);
      expect(
        /innerHTML|outerHTML|insertAdjacentHTML|DOMParser|document\.write/.test(text),
        path,
      ).toBe(false);
    }
    // Every document-supplied string goes in as text.
    const shell = code(named("shell.ts"));
    expect(shell).toContain("textContent");
    expect(shell).toContain("li.append(svg, entry.name)");
  });

  it("holds the project palette in palette.ts alone: no other page file has a colour literal", () => {
    const hex = /#[0-9a-fA-F]{3,8}\b/;
    for (const [path, source] of shipped) {
      if (path === "./palette.ts") continue;
      expect(hex.test(code(source)), `${path} has a hex colour`).toBe(false);
    }
    expect(/\brgba?\(|\bhsla?\(|\boklch\(/.test(code(named("draw.ts")))).toBe(false);
  });
});

describe("the page's markup and stylesheet", () => {
  it("has no inline style, no inline script, no event-handler attribute, no external link", () => {
    expect(indexHtml).not.toMatch(/\sstyle\s*=/i);
    expect(indexHtml).not.toMatch(/<style[\s>]/i);
    expect(indexHtml).not.toMatch(/<script(?![^>]*\ssrc=)/i);
    expect(indexHtml).not.toMatch(/\son[a-z]+\s*=/i);
    expect(indexHtml).not.toMatch(/https?:\/\//i);
    expect(indexHtml).not.toMatch(/<(iframe|object|embed|link|base|meta\s+http-equiv)/i);
  });

  it("has a title, a language and one app root", () => {
    expect(indexHtml).toContain("<title>Document Map</title>");
    expect(indexHtml).toContain('lang="en"');
    expect(indexHtml.match(/id="app"/g)).toHaveLength(1);
  });

  it("starts the page with the one module, which Vite turns into the classic script", () => {
    expect(indexHtml.match(/<script/g)).toHaveLength(1);
    expect(indexHtml).toContain('src="./main.ts"');
  });

  it("is read, not an empty stub", () => {
    expect(styleCss.length).toBeGreaterThan(3000);
    expect(styleCss).toContain(".card");
  });

  it("scrolls a long legend instead of clipping it (a name nowhere else would be lost)", () => {
    const rules = styleCss.replace(/\/\*[\s\S]*?\*\//g, "");
    const legend = /\.legend\s*\{([^}]*)\}/.exec(rules)?.[1] ?? "";
    expect(legend).toMatch(/max-height:/);
    expect(legend).toMatch(/overflow-y:\s*auto/);
    expect(legend).not.toMatch(/overflow(-y)?:\s*hidden/);
  });

  it("has no colour literal, no url, no import and no font face (the frame loads none of them)", () => {
    // Comments out: the header says what the rules ban, in the words of the ban.
    const rules = styleCss.replace(/\/\*[\s\S]*?\*\//g, "");
    expect(rules).not.toMatch(/#[0-9a-fA-F]{3,8}\b/);
    expect(rules).not.toMatch(/\brgba?\(|\bhsla?\(/);
    expect(rules).not.toMatch(/url\(/i);
    expect(rules).not.toMatch(/@import|@font-face|data:|expression\(|behavior:/i);
  });

  it("wears PM's variables, with a system colour as the fallback for each it names", () => {
    const used = Array.from(styleCss.matchAll(/var\(--([a-z0-9-]+)\s*(,[^)]*)?\)/g));
    expect(used.length).toBeGreaterThan(40);
    // Every PM theme variable has a fallback (the page is legible before the theme arrives); only the
    // ones that nest another var() or are a number or a size may have a plain one.
    const missing = used.filter((m) => m[2] === undefined).map((m) => m[1]);
    expect(missing).toEqual([]);
    const names = new Set(used.map((m) => m[1]));
    for (const wanted of ["bg", "ink", "ink3", "ink4", "border", "surface", "panel", "accent"]) {
      expect(names.has(wanted), wanted).toBe(true);
    }
  });

  it("scales with PM's text size and honours reduced motion", () => {
    expect(styleCss).toContain("calc(100% * var(--font-scale, 1))");
    const animated = styleCss.match(/animation:/g) ?? [];
    expect(animated.length).toBeGreaterThan(0);
    // Every animation sits inside the no-preference query.
    const guarded = styleCss.slice(
      styleCss.indexOf("@media (prefers-reduced-motion: no-preference)"),
    );
    expect((guarded.match(/animation:/g) ?? []).length).toBe(animated.length);
  });

  it("draws a keyboard focus ring on every control", () => {
    expect(styleCss).toMatch(/:where\(button, select, input\):focus-visible/);
  });

  it("hides what is hidden whatever display it was given", () => {
    expect(styleCss).toMatch(/\[hidden\]\s*\{\s*display:\s*none\s*!important/);
  });

  // WCAG 1.4.11: a control's edge needs 3:1 against its ground, and only --ink4 gives it (--border and
  // --border2 are decoration). The built-in Map's tests held this by reading its source; they go when the
  // Map becomes this mod, so the mod holds its own edges here.
  describe("holds the control edges at 3:1 (WCAG 1.4.11)", () => {
    const rules = styleCss.replace(/\/\*[\s\S]*?\*\//g, "");
    /** The declarations of every rule whose selector list names exactly `selector`. */
    const declarations = (selector: string): string[] => {
      const found: string[] = [];
      for (const m of rules.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
        const list = m[1].split(",").map((part) => part.trim());
        if (list.includes(selector)) {
          found.push(...m[2].split(";").map((d) => d.trim().replace(/\s+/g, " ")));
        }
      }
      return found;
    };
    /** The colour of the edge: the `border` or `border-color` declarations that draw one (not `border: 0`). */
    const edges = (selector: string) =>
      declarations(selector).filter((d) => /^border(-color)?:/.test(d) && !/^border: 0$/.test(d));

    it.each(["select", 'input[type="number"]', ".chip", ".seg", ".nav-button", ".check"])(
      "draws the edge of %s in --ink4, never --border or --border2",
      (selector) => {
        const drawn = edges(selector);
        expect(drawn.length, `${selector} has no edge`).toBeGreaterThan(0);
        for (const edge of drawn) {
          expect(edge, selector).toMatch(/var\(--ink4\b/);
          expect(edge, selector).not.toMatch(/--border2?\b/);
        }
      },
    );

    it("gives the active segment an inset --ink4 rim", () => {
      const shadow = declarations(".seg-button.active").filter((d) => d.startsWith("box-shadow:"));
      expect(shadow).toHaveLength(1);
      expect(shadow[0]).toMatch(/^box-shadow: inset 0 0 0 1px var\(--ink4\b/);
    });

    it("draws the Enhanced checkbox itself: appearance none, so its edge is the --ink4 one", () => {
      expect(declarations(".check")).toContain("appearance: none");
      // The page builds that checkbox with the class this rule is on.
      expect(code(named("shell.ts"))).toMatch(/enhanced\.className = "check"/);
    });

    it("sets Cancel and Sharpen as link buttons, in the accent text colour", () => {
      const shell = code(named("shell.ts"));
      expect(shell).toContain('button("link", CANCEL)');
      expect(shell).toContain('button("link", SHARPEN)');
      expect(declarations(".link")).toContain("color: var(--accent-text, LinkText)");
    });
  });
});
