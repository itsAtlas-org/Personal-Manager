// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The design system's rules, held for the Pinboard's own sources: the mod's copy of the guards in
// src/theme/designGuards.test.ts and src/theme/contrast.test.ts that name the Pinboard. Core's guards
// read PM's src/ tree and never see a mod's folder (B2 learned that no mod test held these), so the
// Pinboard's copy of the board, and what it draws, is read here. It reads its own sources the same way
// (`import.meta.glob(… ?raw)`, no Node API), tests excluded: a test may quote what it forbids.
//
// The kit files the Pinboard draws (Button, Input, Select, SegmentedControl, Modal ...) are core's, and
// core's guards hold them. What the mod adds is its own markup: the dialogs it opens, the labels it
// writes, the edges of the controls it styles by hand, and the one segmented switch it carries.

import { describe, expect, it } from "vitest";

const RAW = import.meta.glob("./**/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

/** Source path (relative to the mod's src/) → contents, tests excluded. */
const SOURCES: ReadonlyArray<readonly [string, string]> = Object.entries(RAW)
  .map(([key, text]) => [key.replace(/^\.\//, ""), text] as const)
  .filter(([path]) => !/\.test\.tsx?$/.test(path))
  .sort(([a], [b]) => a.localeCompare(b));

const source = (path: string): string => {
  const hit = SOURCES.find(([p]) => p === path);
  if (!hit) throw new Error(`${path} is not one of the mod's sources`);
  return hit[1];
};

/** The opening `<Modal …>` tag only, brace-depth aware (a `>` inside an arrow function in an attribute
 *  does not end it early, and a `label=` on a CHILD element cannot be mistaken for one). */
function openingTag(src: string, start: number): string {
  let depth = 0;
  for (let i = start; i < src.length; i++) {
    const ch = src[i];
    if (ch === "{") depth++;
    else if (ch === "}") depth--;
    else if (ch === ">" && depth === 0) return src.slice(start, i + 1);
  }
  return src.slice(start);
}

describe("every dialog the Pinboard opens has an accessible name", () => {
  it("finds the dialogs it is meant to be scanning", () => {
    const withModals = SOURCES.filter(([, t]) => /<Modal[\s>]/.test(t)).map(([p]) => p);
    expect(withModals).toContain("components/PinboardView.tsx");
  });

  it("has no unnamed <Modal> in the mod", () => {
    const offenders = SOURCES.filter(([, text]) =>
      [...text.matchAll(/<Modal[\s>]/g)].some((m) => {
        const tag = openingTag(text, m.index);
        return !/\blabelledBy\b/.test(tag) && !/\blabel\s*=/.test(tag);
      }),
    ).map(([path]) => path);
    expect(offenders).toEqual([]);
  });
});

describe("`text-faint` is decorative, never informational text", () => {
  it("is used nowhere in the mod's own sources", () => {
    // Core's allow-list is separators and disabled kit buttons; the Pinboard has neither of its own.
    expect(SOURCES.filter(([, text]) => text.includes("text-faint")).map(([p]) => p)).toEqual([]);
  });
});

const LABELABLE =
  /<(input|select|textarea|Input|Select|Textarea|Toggle|SegmentedControl|DateField|TimeField)[\s/>]/;

/** The `<label>`'s children, nesting-aware (an inner `<label>` cannot close the outer one). */
function elementBody(src: string, afterOpeningTag: number): string {
  let depth = 1;
  let i = afterOpeningTag;
  while (i < src.length) {
    const open = src.indexOf("<label", i);
    const close = src.indexOf("</label>", i);
    if (close === -1) return src.slice(afterOpeningTag);
    if (open !== -1 && open < close) {
      depth++;
      i = open + 6;
    } else {
      depth--;
      if (depth === 0) return src.slice(afterOpeningTag, close);
      i = close + 8;
    }
  }
  return src.slice(afterOpeningTag);
}

describe("every <label> is associated with a control", () => {
  it("finds the labels it is meant to be scanning", () => {
    const withLabels = SOURCES.filter(([, t]) => /<label[\s>]/.test(t)).map(([p]) => p);
    expect(withLabels).toContain("components/PinboardView.tsx");
  });

  it("has no orphan <label> in the mod", () => {
    const offenders: string[] = [];
    for (const [path, text] of SOURCES) {
      const lines = text.split("\n");
      for (const m of text.matchAll(/<label[\s>]/g)) {
        const lineNo = text.slice(0, m.index).split("\n").length;
        const lead = lines[lineNo - 1].trimStart();
        if (lead.startsWith("//") || lead.startsWith("*") || lead.startsWith("/*")) continue;
        const tag = openingTag(text, m.index);
        const named = /\bhtmlFor\b/.test(tag) || /\{\.\.\./.test(tag);
        const wraps =
          !/\/>$/.test(tag) &&
          (() => {
            const body = elementBody(text, m.index + tag.length);
            return LABELABLE.test(body) || body.includes("{children}");
          })();
        if (!named && !wraps) offenders.push(`${path}:${lineNo}`);
      }
    }
    expect(offenders).toEqual([]);
  });
});

// A control's edge is its visual boundary, so it is `--ink4` (3:1, WCAG 1.4.11; contrast.test.ts
// measures it), never `--border2` (1.33 to 1.84:1 at the default Contrast). A bare `border-border2` in
// the Pinboard is right on a panel (the folder's open card, the folder board) and wrong on a control.
describe("the Pinboard's hand-styled controls edge in --ink4", () => {
  const BARE_BORDER2 = /(?<![\w-])(?<!disabled:)border-border2\b/;

  it("the only bare border-border2 in the mod is on a panel", () => {
    const hits: string[] = [];
    for (const [path, text] of SOURCES) {
      text.split("\n").forEach((line) => {
        if (/^\s*(\/\/|\*|\/\*|\{\/\*)/.test(line)) return;
        if (BARE_BORDER2.test(line)) hits.push(`${path}: ${line.trim()}`);
      });
    }
    // The folder's open card and the folder board: panels, not controls.
    expect(hits.map((h) => h.split(": ")[0])).toEqual([
      "components/FolderGame.tsx",
      "components/PinboardView.tsx",
    ]);
    expect(hits[0]).toContain("bg-surface");
    expect(hits[1]).toContain("bg-panel");
  });

  it("the card's project field keeps its --ink4 edge", () => {
    const anchor = "text-[0.6875rem] text-ink3 focus:border-accent-text";
    const hits = source("components/PinboardView.tsx")
      .split("\n")
      .filter((l) => l.includes(anchor));
    expect(hits.length, `no line holds "${anchor}"`).toBeGreaterThan(0);
    for (const l of hits) expect(l).toContain("border-ink4");
  });
});

describe("the Pinboard's list/timeline switch is rimmed", () => {
  const RIM = "shadow-[inset_0_0_0_1px_var(--ink4)]";

  it("the chosen icon carries the --ink4 rim", () => {
    expect(source("components/PinboardView.tsx")).toContain(`"bg-accent text-accent-ink ${RIM}"`);
  });
});
