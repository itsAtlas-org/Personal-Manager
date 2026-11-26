// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, it, expect } from "vitest";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import rehypeExternalLinks from "rehype-external-links";
import remarkGfm from "remark-gfm";
import {
  Markdown,
  REHYPE_PLUGINS,
  REMARK_PLUGINS,
  REMARK_PLUGINS_PLAIN_LINES,
  safeUrl,
  SCHEMA,
} from "./markdown";
import { remarkPlainLines } from "./markdownPlainLines";

// The markdown pipeline is PM's single sanitizing boundary for untrusted, ingested content. These lock
// the two pure pieces of that boundary so a refactor can't silently weaken it (T-07), plus the SHAPE
// of the pipeline itself (H6). Behaviour through the real renderer is in `markdown.render.test.tsx`;
// the two halves are deliberately separate so the pure suite keeps its node environment.

describe("safeUrl", () => {
  it("gives an image no source at all, whatever its URL", () => {
    for (const url of [
      "https://example.com/x.png",
      "/local.png",
      "./rel.png",
      "#a",
      "data:image/png;base64,AA",
    ]) {
      expect(safeUrl(url, "src"), url).toBe("");
    }
    // A link's URL is judged as before.
    expect(safeUrl("https://example.com/x", "href")).toBe("https://example.com/x");
    expect(safeUrl("/path", "href")).toBe("/path");
  });

  it("keeps http(s) / mailto absolute URLs", () => {
    expect(safeUrl("https://example.com/x")).toBe("https://example.com/x");
    expect(safeUrl("http://example.com")).toBe("http://example.com");
    expect(safeUrl("mailto:a@b.co")).toBe("mailto:a@b.co");
  });

  it("keeps relative and in-page targets", () => {
    expect(safeUrl("#anchor")).toBe("#anchor");
    expect(safeUrl("/path")).toBe("/path");
    expect(safeUrl("./rel")).toBe("./rel");
    expect(safeUrl("../up")).toBe("../up");
  });

  // The gap the "keeps relative and in-page targets" case above could not see: every one of its
  // inputs uses ONE slash, and two slashes mean something entirely different. A protocol-relative
  // target has no scheme, so the sanitizer's protocol allowlist passes it (it bails out "allowed"
  // when there is no colon) and the `/`-prefix branch here used to hand it straight back — while the
  // browser resolves it against the PAGE's protocol, i.e. `//evil.example/x` from
  // `http://tauri.localhost` is `http://evil.example/x`. `safeUrl` is the LAST thing to touch a URL,
  // so it is the only layer that can neutralise it.
  it("treats a protocol-relative target as absolute, not relative", () => {
    expect(safeUrl("//evil.example/x")).toBe("");
    expect(safeUrl("//evil.example")).toBe("");
    // Chromium's URL parser treats `/\` like `//`, so it is guarded too. NOT a case that can arrive
    // from Markdown — remark percent-encodes destinations, so `/\evil` gets here as `/%5Cevil` and
    // stays same-origin — it is cheap defence for a caller that isn't remark.
    expect(safeUrl("/\\evil.example/x")).toBe("");
    expect(safeUrl("\\\\evil.example/x")).toBe("");
    // The counter-assertion: a genuine single-slash relative target must still survive, or the fix
    // has over-corrected into breaking every in-app link.
    expect(safeUrl("/path")).toBe("/path");
    expect(safeUrl("/")).toBe("/");
  });

  it("neutralises a hostile or non-allowlisted scheme to empty", () => {
    expect(safeUrl("javascript:alert(1)")).toBe("");
    expect(safeUrl("data:text/html,<script>alert(1)</script>")).toBe("");
    expect(safeUrl("vbscript:msgbox('x')")).toBe("");
    expect(safeUrl("file:///etc/passwd")).toBe("");
  });

  it("is case-insensitive on the scheme", () => {
    expect(safeUrl("HTTPS://example.com")).toBe("HTTPS://example.com");
    expect(safeUrl("JavaScript:alert(1)")).toBe("");
  });
});

describe("SCHEMA", () => {
  it("extends the default anchor allowlist with target + rel", () => {
    expect(SCHEMA.attributes.a).toContain("target");
    expect(SCHEMA.attributes.a).toContain("rel");
  });

  it("never allows a <script> tag (the GitHub default omits it)", () => {
    expect(SCHEMA.tagNames ?? []).not.toContain("script");
  });

  // The assertion above reads hast-util-sanitize's own allow-list, because SCHEMA never sets
  // `tagNames` of its own — it would only catch PM literally appending "script". This is the
  // PM-side pin: the ONLY things we widen are `attributes.a`. Everything else,
  // `protocols` above all (the list that makes a `javascript:` href die at the sanitizer even if
  // urlTransform were bypassed), has to stay exactly the library default.
  it("widens the default schema in exactly one place and no other", () => {
    const differing = Object.keys(defaultSchema).filter(
      (k) =>
        SCHEMA[k as keyof typeof SCHEMA] !== defaultSchema[k as keyof typeof defaultSchema] &&
        k !== "attributes",
    );
    expect(differing).toEqual([]);
    expect(Object.keys(SCHEMA).sort()).toEqual(Object.keys(defaultSchema).sort());

    const attrs = SCHEMA.attributes as Record<string, unknown>;
    const defaults = (defaultSchema.attributes ?? {}) as Record<string, unknown>;
    const changedAttrs = Object.keys(defaults).filter((k) => attrs[k] !== defaults[k]);
    expect(changedAttrs).toEqual(["a"]);
    expect(Object.keys(attrs).sort()).toEqual(Object.keys(defaults).sort());
  });

  // The one class a `ul` may carry is the library's own `contains-task-list`, VALUE-PINNED: the
  // `["className", "…"]` form admits that literal and nothing else, exactly as the library's own
  // schema admits `task-list-item` on an `li`. A regression to a bare `"className"` would let any
  // class through and would not be caught by the diff above. (PM once added `pm-dash-list` here for
  // the Pinboard's dash points; the Pinboard mod draws those in its own editor now, so a widening
  // of the one sanitizing renderer for it would be attack surface for nothing.)
  it("admits only the library's own task-list class on a ul, by literal value, in a single entry", () => {
    const ul = (SCHEMA.attributes as Record<string, unknown[]>).ul;
    const classEntries = ul.filter(
      (e) => e === "className" || (Array.isArray(e) && e[0] === "className"),
    );
    expect(classEntries).toEqual([["className", "contains-task-list"]]);
    expect(ul).not.toContain("className");
  });

  it("keeps every non-class ul attribute the library allowed", () => {
    const ul = (SCHEMA.attributes as Record<string, unknown[]>).ul;
    const defaults = (defaultSchema.attributes?.ul ?? []) as unknown[];
    for (const entry of defaults) {
      if (Array.isArray(entry) && entry[0] === "className") continue;
      expect(ul).toContainEqual(entry);
    }
  });

  it("only adds to the anchor allow-list, never replaces it", () => {
    const a = (SCHEMA.attributes as Record<string, unknown[]>).a;
    for (const entry of (defaultSchema.attributes?.a ?? []) as unknown[]) {
      expect(a).toContainEqual(entry);
    }
    expect(a).toContain("target");
    expect(a).toContain("rel");
  });
});

// Remark plugins run on mdast, upstream of the whole rehype chain, so none of them can put anything
// past the sanitizer no matter what it emits. These pin which surfaces get which.
describe("the remark pipeline", () => {
  it("leaves the default surface untouched", () => {
    expect(REMARK_PLUGINS).toHaveLength(1);
    expect(REMARK_PLUGINS[0]).toBe(remarkGfm);
  });

  it("adds the plain-lines parser setting only on the opted-in surface", () => {
    expect(REMARK_PLUGINS_PLAIN_LINES).toHaveLength(2);
    expect(REMARK_PLUGINS_PLAIN_LINES[0]).toBe(remarkGfm);
    expect(REMARK_PLUGINS_PLAIN_LINES[1]).toBe(remarkPlainLines);
  });

  it("does not touch the rehype array, whose ORDER is the security property", () => {
    expect(REHYPE_PLUGINS).toHaveLength(2);
  });
});

// H6. Order is the security property, and it is not visible from any behavioural test: a plugin
// added AFTER the sanitizer gets whatever it emits straight into the DOM. Adding `rehype-raw` — the
// plausible one-line change to "make HTML in a captured web article render" — is exactly the shape
// of edit these exist to stop passing silently.
describe("the rehype pipeline", () => {
  const entry = (i: number) => REHYPE_PLUGINS[i] as unknown[];

  it("runs the sanitizer LAST", () => {
    const last = entry(REHYPE_PLUGINS.length - 1);
    expect(last[0]).toBe(rehypeSanitize);
  });

  it("hands the sanitizer PM's schema, not a fresh one", () => {
    const last = entry(REHYPE_PLUGINS.length - 1);
    expect(last[1]).toBe(SCHEMA);
  });

  // Strict on purpose. A new rehype plugin is a decision about where it sits relative to the
  // sanitizer, so it should have to come here and say so rather than be appended quietly.
  it("holds exactly the two plugins it is meant to", () => {
    expect(REHYPE_PLUGINS).toHaveLength(2);
    expect(entry(0)[0]).toBe(rehypeExternalLinks);
  });
});

// What the renderer does with a note's list markers. The Pinboard's "-" points keep their en dash
// because the Pinboard MOD's own editor draws it; PM's Markdown has no dash-list mode, so a "- item"
// in a document, a chat answer or a promoted note is an ordinary bullet. These are literal strings
// on purpose: a class or a marker that appeared would show in them.
describe("list markers in ordinary Markdown", () => {
  const render = (source: string): string =>
    renderToStaticMarkup(createElement(Markdown, null, source));

  it("renders a `- a` line as an ordinary bullet list, with no dash-list class", () => {
    const out = render("- a");
    expect(out).toBe('<div class="pm-markdown"><ul>\n<li>a</li>\n</ul></div>');
    expect(out).not.toContain("pm-dash-list");
  });

  it("renders a `+ a` line the same, so a note read back from the vault is a plain list", () => {
    expect(render("+ a")).toBe('<div class="pm-markdown"><ul>\n<li>a</li>\n</ul></div>');
  });

  it("keeps a `*` list and a `+` list as two ordinary lists, neither carrying a class", () => {
    expect(render("* a\n\n+ b")).toBe(
      '<div class="pm-markdown"><ul>\n<li>a</li>\n</ul>\n<ul>\n<li>b</li>\n</ul></div>',
    );
  });

  it("turns a two-space hard break into a <br/>", () => {
    expect(render("one  \ntwo")).toBe('<div class="pm-markdown"><p>one<br/>\ntwo</p></div>');
  });

  it("keeps the task-list classes the checklist CSS relies on", () => {
    // The `:has()` selectors in index.css are a fallback for renderers that drop these; on an
    // engine without `:has()` support these classes are the ONLY thing carrying the rule.
    const out = render("- [ ] todo");
    expect(out).toContain('<ul class="contains-task-list">');
    expect(out).toContain('<li class="task-list-item">');
    expect(out).not.toContain("pm-dash-list");
  });
});
