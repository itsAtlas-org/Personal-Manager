// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Button's sizing contract. The defect this locks down is subtle and was invisible for months: 50
// call sites passed their own `px-*`/`py-*`/`text-*`, and because `cn()` is a plain joiner the
// classes did not replace the base — both survived, and Tailwind's emission order decided. Spacing
// is emitted ASCENDING, so the base `px-3 py-1.5` beat every `px-2`/`py-1` a call site asked for;
// font sizes are emitted alphabetically, so `text-xs` DID beat `text-sm`. Every compact button in
// PM was therefore a full-size box with shrunken type, and nothing failed.
//
// So the assertions here are COUNTS, not `toContain`. `toContain("px-2")` passes just as happily on
// a re-layered class list that also carries `px-3` — a count is the only shape that catches the
// regression this seam exists to prevent.
//
// `useTheme` is stubbed rather than wrapped in the real provider (which pulls in IPC) — the same
// stub ErrorBoundary.test.tsx and ConnectorItemRow.test.tsx use. `system` is mutable so the
// terminal branch can be exercised without a second mock.

import { render, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const theme = vi.hoisted(() => ({ system: "slate" as "slate" | "editorial" | "terminal" }));
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: theme.system, mode: "dark", accent: "mono", depth: "standard" }),
}));

import { Button, type ButtonSize } from "./Button";
import { Select } from "./Select";
import { SegmentedControl } from "./SegmentedControl";
import { TONE_MIX } from "./tone";

afterEach(() => {
  theme.system = "slate";
  cleanup();
});

/** Only the font-SIZE utilities — `text-ink2` and `disabled:text-faint` are colours and must not
 *  count. The `(?:^|\s)` anchor is what excludes the variant-prefixed forms. */
const TEXT_SIZE = /(?:^|\s)(text-(?:xs|sm|base|lg|xl|\[[^\]]+\]))/g;
const PX = /(?:^|\s)(px-[\w.[\]/-]+)/g;
const PY = /(?:^|\s)(py-[\w.[\]/-]+)/g;

function sizing(className: string) {
  const one = (re: RegExp) => [...className.matchAll(re)].map((m) => m[1]);
  return { px: one(PX), py: one(PY), text: one(TEXT_SIZE), minH: one(MIN_H) };
}

const SIZES: ButtonSize[] = ["xs", "sm", "md", "lg", "bar"];
const MIN_H = /(?:^|\s)(min-h-\S+)/g;

describe("Button sizing", () => {
  it.each(SIZES)("emits exactly one px / py / text-size at %s", (size) => {
    const { getByRole } = render(<Button size={size}>Go</Button>);
    const { px, py, text, minH } = sizing(getByRole("button").className);
    expect(px).toHaveLength(1);
    expect(py).toHaveLength(1);
    expect(text).toHaveLength(1);
    // The height floor is part of the size, so two of them can never fight on stylesheet order.
    expect(minH).toHaveLength(1);
  });

  it("leaves the 200 unannotated call sites exactly where they were", () => {
    const { getByRole } = render(<Button>Go</Button>);
    expect(sizing(getByRole("button").className)).toEqual({
      px: ["px-3"],
      py: ["py-1.5"],
      text: ["text-sm"],
      minH: ["min-h-[var(--tap-min,24px)]"],
    });
  });

  it("matches Select's compact triple at sm, so the two primitives line up", () => {
    const { getByRole: getButton } = render(
      <Button size="sm" aria-label="Add">
        Add
      </Button>,
    );
    const { px, py, text } = sizing(getButton("button").className);
    const btn = { px, py, text };
    cleanup();
    const { getByRole: getSelect } = render(
      <Select compact aria-label="Zone">
        <option>UTC</option>
      </Select>,
    );
    const { px: sx, py: sy, text: st } = sizing(getSelect("combobox").className);
    const sel = { px: sx, py: sy, text: st };
    expect(btn).toEqual(sel);
  });

  it("keeps the tap-target floor at every size", () => {
    // WCAG 2.5.8. jsdom computes no layout, so this asserts the floor is still DECLARED — the
    // rendered height at xs is entirely --tap-min, which is why xs and sm share a height at
    // Standard density and differ only in horizontal padding. `bar` floors higher, at the
    // SegmentedControl's own height (pinned below), so it is never lower than --tap-min.
    for (const size of SIZES) {
      cleanup();
      const { getByRole } = render(<Button size={size}>Go</Button>);
      const cls = getByRole("button").className;
      expect(cls).toContain(
        size === "bar"
          ? "min-h-[calc(max(var(--tap-min,24px),1.5rem)_+_0.25rem_+_2px)]"
          : "min-h-[var(--tap-min,24px)]",
      );
      expect(cls).toContain("min-w-[var(--tap-min,24px)]");
    }
  });

  it("never lets size reach the DOM", () => {
    // <button> has no `size` attribute (only input/select do), so TypeScript cannot catch this one:
    // leaving `size` in `...rest` would type-check and quietly emit an invalid attribute.
    const { getByRole } = render(<Button size="sm">Go</Button>);
    expect(getByRole("button").hasAttribute("size")).toBe(false);
  });

  it("still brackets the label under the terminal System at the smallest size", () => {
    theme.system = "terminal";
    const { getByRole } = render(<Button size="xs">×</Button>);
    const btn = getByRole("button");
    expect(btn.textContent).toContain("[");
    expect(btn.textContent).toContain("]");
    expect(btn.className).toContain("font-mono");
    expect(btn.className).toContain("min-w-[var(--tap-min,24px)]");
  });
});

describe("Button danger variant", () => {
  it("takes its tint from the shared tone map, not a fifth hand-typed ratio", () => {
    const { getByRole } = render(<Button variant="danger">Delete</Button>);
    const cls = getByRole("button").className;
    // Spelled literally in Button.tsx because Tailwind extracts class names by scanning source
    // text; this is what stops the literal drifting away from tone.ts.
    expect(cls).toContain(`var(--st-due)_${TONE_MIX.fill}%`);
    expect(cls).toContain(`var(--st-due)_${TONE_MIX.fillHover}%`);
    expect(cls).not.toContain("#");
  });

  it("carries no inline style, so a call site never has to hand-tint a primary", () => {
    const { getByRole } = render(<Button variant="danger">Delete</Button>);
    expect(getByRole("button").getAttribute("style")).toBeNull();
  });

  it("sizes independently of its variant", () => {
    const { getByRole } = render(
      <Button variant="danger" size="sm">
        Delete
      </Button>,
    );
    const { px, py, text } = sizing(getByRole("button").className);
    expect(px).toEqual(["px-2"]);
    expect(py).toEqual(["py-1"]);
    expect(text).toEqual(["text-xs"]);
  });
});

describe("Button variant", () => {
  // Readable from outside, so a screen's primaries can be counted — the one-primary rule — without
  // parsing class strings.
  it("names its variant on the element, secondary by default", () => {
    const { getAllByRole } = render(
      <>
        <Button variant="primary">A</Button>
        <Button>B</Button>
        <Button variant="tertiary">C</Button>
        <Button variant="danger">D</Button>
      </>,
    );
    expect(getAllByRole("button").map((b) => b.getAttribute("data-variant"))).toEqual([
      "primary",
      "secondary",
      "tertiary",
      "danger",
    ]);
  });
});

// "bar" is the height of a SegmentedControl, so a control in a row with one (the Calendar header's)
// lines up at every Text size and Density. jsdom draws nothing, so the heights are WORKED OUT from
// the classes the components really emit, and compared. In Tailwind v4 (border-box, via preflight)
// spacing n is n x 0.25rem, text-xs has a 1rem line and text-sm a 1.25rem one:
//
//   SegmentedControl  = 2 x 1px border + 2 x p-0.5 (0.25rem) + item
//       item          = max(--tap-min, 2 x py-1 (0.5rem) + 1rem)       -> 2px + 0.25rem + max(tap, 1.5rem)
//   Button bar floor  = max(--tap-min, 1.5rem) + 0.25rem + 2px     (the same number, whatever is inside)
//   Button bar text   = 2px + 2 x py-1.5 (0.75rem) + 1rem            (a text label: also the same number)
//
// The floor takes the max(--tap-min, 1.5rem) itself, not just --tap-min, because an icon-only button
// (Pop out, Refresh) holds a 0.875rem glyph, shorter than a 1rem text line, and at Large text the
// padding alone would leave it a few px short of the segmented control. The floor lifts it to match.
//
// e.g. Standard (tap 24px) at a 16px root: 30px both; Large text (18.4px root): 34.2px both;
// 1.3 text (20.8px root): 38.4px both; Comfortable (tap 44px): 50px both. Change SegmentedControl's border, p-0.5, or item py-1 /
// text-xs / floor, or bar's py, text size or floor, and this fails until they agree again.
describe("Button bar matches a SegmentedControl", () => {
  const LINE_REM: Record<string, number> = { "text-xs": 1, "text-sm": 1.25 };
  const unit = (cls: string, re: RegExp): number => {
    const m = cls.match(re);
    if (!m) throw new Error(`no ${re} in ${cls}`);
    return Number(m[1]) * 0.25;
  };
  /** The border-box height in px of a box with these classes (an item or a bordered control). */
  function heightPx(
    cls: string,
    tap: number,
    rem: number,
    edge: boolean,
    contentRem?: number,
  ): number {
    const pad = 2 * unit(cls, /(?:^|\s)py-([\d.]+)(?:\s|$)/) * rem;
    // A text label is one line of its type size; an icon-only button passes its glyph's height.
    const line = (contentRem ?? LINE_REM[cls.match(/(?:^|\s)(text-(?:xs|sm))(?:\s|$)/)![1]]) * rem;
    const border = edge ? 2 : 0;
    const floor = cls.includes("min-h-[calc(max(var(--tap-min,24px),1.5rem)_+_0.25rem_+_2px)]")
      ? Math.max(tap, 1.5 * rem) + 0.25 * rem + 2
      : cls.includes("min-h-[var(--tap-min,24px)]")
        ? tap
        : 0;
    return Math.max(floor, border + pad + line);
  }
  const CASES = [
    [24, 16],
    [24, 18.4],
    [24, 20.8],
    [44, 16],
    [44, 18.4],
    [44, 20.8],
  ] as const;

  function segmented() {
    const { container } = render(
      <SegmentedControl
        ariaLabel="x"
        options={[{ value: "a", label: "A" }]}
        value="a"
        onChange={() => {}}
      />,
    );
    const frame = container.firstElementChild as HTMLElement;
    const item = frame.querySelector("button")!;
    return { frame: frame.className, item: item.className };
  }
  /** The frame's outer height: its border and p-0.5 around one item. */
  function segmentedPx(tap: number, rem: number) {
    const { frame, item } = segmented();
    cleanup();
    const edge = /(?:^|\s)border(?:\s|$)/.test(frame) ? 2 : 0;
    const pad = 2 * unit(frame, /(?:^|\s)p-([\d.]+)(?:\s|$)/) * rem;
    return edge + pad + heightPx(item, tap, rem, false);
  }

  it.each(CASES)("is the same height at a tap target of %ipx and a %ipx root", (tap, rem) => {
    const want = segmentedPx(tap, rem);
    const { getByRole } = render(<Button size="bar">Go</Button>);
    expect(heightPx(getByRole("button").className, tap, rem, true)).toBeCloseTo(want, 5);
  });

  it.each(CASES)("holds the height for an icon-only button too, at %ipx and %ipx", (tap, rem) => {
    // Pop out and Refresh hold a 0.875rem (h-3.5) glyph, not a text line, and are borderless.
    const want = segmentedPx(tap, rem);
    const { getByRole } = render(
      <Button size="bar" variant="tertiary" aria-label="Refresh">
        <svg className="h-3.5 w-3.5" />
      </Button>,
    );
    const cls = getByRole("button").className;
    expect(heightPx(cls, tap, rem, false, 0.875)).toBeCloseTo(want, 5);
  });

  it.each(CASES)("is Select's bar height too, at %ipx and %ipx", (tap, rem) => {
    const want = segmentedPx(tap, rem);
    const { getByRole } = render(
      <Select bar aria-label="Days">
        <option>1</option>
      </Select>,
    );
    expect(heightPx(getByRole("combobox").className, tap, rem, true)).toBeCloseTo(want, 5);
  });

  it("is a bordered secondary like the segmented frame, and the same type size", () => {
    const { frame, item } = segmented();
    cleanup();
    expect(frame).toMatch(/(?:^|\s)border(?:\s|$)/);
    expect(item).toContain("text-xs");
    const { getByRole } = render(<Button size="bar">Go</Button>);
    expect(getByRole("button").className).toMatch(/(?:^|\s)border(?:\s|$)/);
    expect(sizing(getByRole("button").className).text).toEqual(["text-xs"]);
  });

  it("gives Select's bar the same py, text size and floor as Button's", () => {
    const { getByRole: getButton } = render(<Button size="bar">Go</Button>);
    const btn = sizing(getButton("button").className);
    cleanup();
    const { getByRole: getSelect } = render(
      <Select bar aria-label="Days">
        <option>1</option>
      </Select>,
    );
    const sel = sizing(getSelect("combobox").className);
    expect({ py: sel.py, text: sel.text, minH: sel.minH }).toEqual({
      py: btn.py,
      text: btn.text,
      minH: btn.minH,
    });
  });
});
