// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The contrast-audit (the "do we adhere?" test, PR: contrast axis). Drives the real themeVars path
// for every System × Mode at each contrast level, converts the emitted oklch()/hex token values back
// to WCAG relative luminance, and asserts the level's targets: AA (1.4.3) lifts all body text to
// 4.5:1; High reaches AAA (7:1) for ink..ink3 and firms up ink4/faint (4.5:1) and the border edges
// (3:1, 1.4.11). Body text is checked against every background it sits on (bg/panel/surface) — the
// worst case. If a token ramp regresses, this fails in CI.
//
// The neutral ramp was the whole audit for a while, and that was the hole: --st-* is text too, and
// because it sits outside the contrast axis nothing was watching it. 15 of 18 STATUS.light cells and
// 4 of 6 STATUS_CVD.light cells were below AA — `look` as low as 3.06:1 — at every setting the app
// offered. The status case below closes that, and it sweeps every accent rather than the default,
// because the accent hue tints the backgrounds the status colours are calibrated against.

import { describe, expect, it } from "vitest";
import { boost, themeVars, type ThemeVars } from "./tokens";
import { oklabLCH, oklchLuminance, contrastRatio } from "./oklab";
import { SYSTEMS, MODES, ACCENTS, STATUS_KEYS } from "./profiles";

// Luminance of a token value, whether it's an oklch(L C H) string or a #hex (the mono --bg pin).
function lumOf(value: string): number {
  if (value.startsWith("#")) {
    const { L, C, H } = oklabLCH(value);
    return oklchLuminance(L, C, H);
  }
  const m = value.match(/oklch\(([-\d.]+)\s+([-\d.]+)\s+([-\d.]+)\)/);
  if (!m) throw new Error(`un-parseable token value: ${value}`);
  return oklchLuminance(Number(m[1]), Number(m[2]), Number(m[3]));
}

const ratioOf = (v: ThemeVars, role: string, bg: string): number =>
  contrastRatio(lumOf(v[`--${role}`]), lumOf(v[`--${bg}`]));

const TEXT_BGS = ["bg", "panel", "surface"] as const;

describe("contrast axis — WCAG targets across every system × mode", () => {
  for (const system of SYSTEMS) {
    const accent = ACCENTS[system][0]; // each System's default accent (mono for slate)
    for (const mode of MODES) {
      it(`${system}/${mode}: AA lifts every body-text role to 4.5:1`, () => {
        const v = themeVars(system, mode, accent, false, "aa");
        for (const role of ["ink", "ink2", "ink3", "ink4"]) {
          for (const bg of TEXT_BGS) {
            expect(ratioOf(v, role, bg)).toBeGreaterThanOrEqual(4.5);
          }
        }
      });

      it(`${system}/${mode}: High reaches AAA body + firmer faint/borders`, () => {
        const v = themeVars(system, mode, accent, false, "high");
        for (const bg of TEXT_BGS) {
          for (const role of ["ink", "ink2", "ink3"]) {
            expect(ratioOf(v, role, bg)).toBeGreaterThanOrEqual(7);
          }
          for (const role of ["ink4", "faint"]) {
            expect(ratioOf(v, role, bg)).toBeGreaterThanOrEqual(4.5);
          }
        }
        // 1.4.11 non-text: the border edges vs the base background.
        for (const role of ["border", "border2"]) {
          expect(ratioOf(v, role, "bg")).toBeGreaterThanOrEqual(3);
        }
      });

      // 1.4.11 again, for the one control with no text of its own to fall back on: a switch.
      // Toggle.tsx outlines its track in `--ink4`, and this is the measurement behind that choice.
      //
      // Both obvious picks fail. `--border2` is the ramp's "strong edge" and lands at 1.42–1.84:1
      // against `--panel` at the default Contrast — even `high` only reaches 2.82–4.29:1, so a
      // switch outlined in it is invisible at every level the app offers, which is exactly what
      // shipping it proved. `--accent` fails from the other side: on a light theme with a pale
      // accent the ON fill drops to 1.36:1 against the row behind it, so an accent-coloured outline
      // is a line drawn in the colour it sits on. The fill alone is never a boundary either
      // (`--surface` on `--panel` is 1.03–1.16:1).
      //
      // Asserted at BOTH levels, not just `high`: an outline that only appears at High contrast is
      // an outline the default install does not have. And across every accent, because the accent
      // hue tints the very backgrounds the outline is measured against.
      it(`${system}/${mode}: the switch outline clears 3:1 on both its fills, at every level`, () => {
        for (const contrast of ["aa", "high"] as const) {
          for (const anyAccent of ACCENTS[system]) {
            const v = themeVars(system, mode, anyAccent, false, contrast);
            // The outline is painted over the track's own edge, so it is read against the fill it
            // rims (`--surface` when OFF) and against the row the switch sits on (`--panel`).
            for (const against of ["surface", "panel"] as const) {
              expect(ratioOf(v, "ink4", against)).toBeGreaterThanOrEqual(3);
            }
          }
        }
      });

      // The event colour picker (EventColorPicker, #884 C13). A swatch's fill is anyone's colour, so
      // its edge is `--ink4`, which the switch case above already measures on both backgrounds. What
      // is new is the chosen swatch's ring: 2px of `--ink` with a 2px gap in `--panel` between it
      // and the fill, so it is read against the popover's own `--panel` and never against the
      // colour. Swept over every accent and BOTH levels (the AA case above takes only the default
      // accent), since the accent tints the panel.
      it(`${system}/${mode}: the colour picker's chosen ring clears 3:1 on its panel, at every level`, () => {
        for (const contrast of ["aa", "high"] as const) {
          for (const anyAccent of ACCENTS[system]) {
            const v = themeVars(system, mode, anyAccent, false, contrast);
            expect(ratioOf(v, "ink", "panel")).toBeGreaterThanOrEqual(3);
          }
        }
      });

      // The chat Thinking toggle (ThinkingToggle.tsx) is a bordered text button that fills with the
      // accent when on. Same choice as the switch above, for the same reason: its edge is `--ink4` in
      // BOTH states, because an accent edge is drawn in the colour of a pale-accent light theme's
      // row (1.36:1). The edge sits on the composer (`--bg` in the global chat, `--panel` beside a
      // project), and the on state's label is `--accent-ink` on the `--accent` fill — text-xs, so
      // the full 4.5:1. Measured minimums over this sweep: 4.89:1 for the edge (terminal/light,
      // #7fe0b0, aa, on --panel) and 5.43:1 for the label (editorial/dark, #c96f4c, aa).
      it(`${system}/${mode}: the chat Thinking toggle's edge and label clear their floors at every level`, () => {
        for (const contrast of ["aa", "high"] as const) {
          for (const anyAccent of ACCENTS[system]) {
            const v = themeVars(system, mode, anyAccent, false, contrast);
            for (const against of ["bg", "panel"] as const) {
              expect(ratioOf(v, "ink4", against)).toBeGreaterThanOrEqual(3);
            }
            expect(ratioOf(v, "accent-ink", "accent")).toBeGreaterThanOrEqual(4.5);
          }
        }
      });

      // The segmented controls (SegmentedControl, the Calendar's RangeControl, the pressed IconButton,
      // the Pinboard's list/timeline switch, the Map's By project / Semantic switch). Two things
      // there failed 1.4.11, measured with the real themeVars over every System x Mode x accent x
      // Contrast level (76 combinations):
      //   - the group's edge in `--border2` (or `--border`): 1.33-1.84:1 against bg/panel/surface at
      //     the default Contrast, and still 2.73-2.95:1 on light themes at High;
      //   - the active segment's accent fill on a light theme with a pale accent: 1.32-3.03:1 (only
      //     one of 18 coloured accents reaches 3:1), at BOTH levels because boost() never touches
      //     --accent. The fill is the accent's own business, so the segment carries a 1px inset rim
      //     in `--ink4` (Toggle's pattern) and the rim is the boundary.
      // So the edge and the rim are both `--ink4`, read against all three surfaces the controls sit
      // on. Measured minimum over the sweep: 4.73:1 (terminal/light, #7fe0b0, on --surface). The
      // label on the fill is `--accent-ink` on `--accent` (text-xs, so the full 4.5:1; minimum 5.44).
      it(`${system}/${mode}: a segmented control's edge and active rim clear 3:1 on every surface, at every level`, () => {
        for (const contrast of ["aa", "high"] as const) {
          for (const anyAccent of ACCENTS[system]) {
            const v = themeVars(system, mode, anyAccent, false, contrast);
            for (const against of TEXT_BGS) {
              expect(ratioOf(v, "ink4", against)).toBeGreaterThanOrEqual(3);
            }
            expect(ratioOf(v, "accent-ink", "accent")).toBeGreaterThanOrEqual(4.5);
          }
        }
      });

      // The semantic status colours are TEXT: the error banner's message, Field's role="alert", the
      // "Due soon" chip, every connector failure string. They render at text-xs, so the 3:1
      // large-text exemption never applies. They also sit OUTSIDE the contrast axis — boost() only
      // touches the neutral ramp and themeVars emits --st-* verbatim, so `aa` and `high` are
      // byte-identical here and High contrast rescues nothing. Both levels are asserted anyway, so
      // that the day someone routes --st-* through boost(), this test is already watching.
      //
      // Every accent, not just the default: the accent hue tints bg/panel/surface, and the light
      // table is calibrated on the WORST of them. `mono` stays in the loop — it is not a colour, but
      // it is a real theme state producing real (chroma-0) backgrounds, and it happens to be the
      // worst case for slate. Both palettes, because the colour-blind axis swaps the whole row.
      for (const contrast of ["aa", "high"] as const) {
        for (const colorblind of [false, true]) {
          const name = `${system}/${mode}/${contrast}${colorblind ? "/cvd" : ""}`;
          it(`${name}: every --st-* clears 4.5:1 as text, on every background`, () => {
            for (const anyAccent of ACCENTS[system]) {
              const v = themeVars(system, mode, anyAccent, colorblind, contrast);
              for (const key of STATUS_KEYS) {
                for (const bg of TEXT_BGS) {
                  expect(ratioOf(v, `st-${key}`, bg)).toBeGreaterThanOrEqual(4.5);
                }
              }
            }
          });
        }
      }
    }
  }

  it("defaults to AA, and High lifts strictly more of the ramp than AA does", () => {
    const implicit = themeVars("slate", "dark", "mono", false);
    const aa = themeVars("slate", "dark", "mono", false, "aa");
    // The default param is the compliant baseline — no caller can land below AA by omitting it.
    expect(implicit).toEqual(aa);

    // AA moves ONLY the lowest text tier; High also firms up ink3, faint and the borders. Pinning
    // both halves keeps "AA is the light touch, High is the firm one" true rather than assumed.
    // (ink4 is deliberately NOT in this list — both levels lift it by the same calibrated amount,
    // because 4.5:1 is where that tier needs to land either way.)
    const high = themeVars("slate", "dark", "mono", false, "high");
    for (const role of ["ink3", "faint", "border", "border2"]) {
      expect(high[`--${role}`]).not.toBe(aa[`--${role}`]);
    }
    // …and AA leaves those roles exactly where the System's ramp put them (boost is identity there).
    const lc: readonly [number, number] = [0.62, 0.01];
    expect(boost(lc, "ink3", "dark", "aa")).toEqual([...lc]);
    expect(boost(lc, "ink4", "dark", "aa")).not.toEqual([...lc]);
  });
});

// The tokens measured above only protect the controls that actually use them. This reads the five
// segmented-control sources (the way designGuards.test.ts reads the tree) and pins the classes the
// measurement was made for: the group edge in `--ink4`, never `--border2`/`--border`, and the active
// segment's inset `--ink4` rim.
describe("segmented controls use the measured edge and rim", () => {
  const RAW = import.meta.glob(
    [
      "../components/ui/SegmentedControl.tsx",
      "../components/ui/IconButton.tsx",
      "../components/calendar/RangeControl.tsx",
      "../components/PinboardView.tsx",
      "../components/GraphView.tsx",
      "../components/calendar/edit/CustomRecurrenceDialog.tsx",
      "../components/calendar/edit/GuestField.tsx",
    ],
    { query: "?raw", import: "default", eager: true },
  ) as Record<string, string>;
  const src = (name: string): string => {
    const hit = Object.entries(RAW).find(([k]) => k.endsWith(name));
    if (!hit) throw new Error(`${name} not found`);
    return hit[1];
  };
  const RIM = "shadow-[inset_0_0_0_1px_var(--ink4)]";

  it("SegmentedControl: ink4 edge, rimmed enabled active segment, no border2 edge", () => {
    const t = src("ui/SegmentedControl.tsx");
    expect(t).toContain("border border-ink4 p-0.5");
    expect(t).not.toContain("border border-border2");
    expect(t).toContain(`"bg-accent text-accent-ink font-medium ${RIM}"`);
  });

  it("RangeControl: the hand-copied group has the same edge and rim", () => {
    const t = src("calendar/RangeControl.tsx");
    expect(t).toContain("border border-ink4 p-0.5");
    expect(t).not.toContain("border-border2");
    expect(t).toContain(`"bg-accent ${RIM}"`);
  });

  it("IconButton: the pressed variant is rimmed", () => {
    expect(src("ui/IconButton.tsx")).toContain(`"bg-accent text-accent-ink ${RIM} enabled:hover`);
  });

  it("Pinboard's list/timeline switch: the chosen icon is rimmed", () => {
    expect(src("PinboardView.tsx")).toContain(`"bg-accent text-accent-ink ${RIM}"`);
  });

  it("Map's By project / Semantic switch: ink4 edge and a rimmed active button", () => {
    const t = src("GraphView.tsx");
    expect(t).toContain("rounded-[var(--radius-sm)] border border-ink4 p-0.5 text-xs");
    expect(t).toContain(`"bg-accent text-accent-ink ${RIM}"`);
  });

  it("Custom repeat's weekday buttons: the rim is there on and off", () => {
    const t = src("calendar/edit/CustomRecurrenceDialog.tsx");
    expect(t).toContain(`"${RIM}",`);
    expect(t).toContain(`on ? "bg-accent font-medium text-accent-ink"`);
  });

  it("A guest's Optional button: the rim is there pressed and not", () => {
    const t = src("calendar/edit/GuestField.tsx");
    expect(t).toContain(`bg-accent px-1.5 text-xs font-medium text-accent-ink ${RIM}`);
    expect(t).toContain(`px-1.5 text-xs text-ink2 ${RIM}`);
  });
});

// ===== BEGIN form-control edges (#1105) =====================================================
// Own block, own describe: other PRs add cases to the file above and below this one.
//
// WCAG 1.4.11: an interactive control's edge is its visual boundary when nothing else marks it out,
// so it owes 3:1 against whatever it sits on. A text field, dropdown, secondary button or chip is
// exactly that case (`--surface` on `--panel` is 1.03-1.16:1, so the fill adds no boundary). The
// ramp's "strong edge" `--border2` measures 1.33-1.84:1 at the default Contrast and 2.73-3.29:1
// even at `high` in a light theme, so it was never an edge a person could see. Every control now
// uses `--ink4`: 4.73:1 or better on every surface (measured minimum: terminal/light/#7fe0b0 on
// --surface).
//
// The pairs are the ones the sweep found, by class:
//   rest edge     border-ink4          on --bg / --panel / --surface  (fields sit on all three)
//   hover edge    border-ink2          never weaker than rest (DocumentFolders, GraphView, RemovePmData)
//   focus/hover   border-accent-text   not `--accent`: a pale accent in a light theme is 1.4:1
//   invalid edge  border-st-due        already the text-status colour, so it clears 4.5:1 too
// Disabled controls drop back to `--border2` on purpose (exempt from 1.4.11, and they should recede).
// Swept over every accent and BOTH Contrast levels: the accent hue tints the surfaces.
describe("form-control edges clear 3:1 on every surface (1.4.11)", () => {
  const SURFACES = ["bg", "panel", "surface"] as const;
  for (const system of SYSTEMS) {
    for (const mode of MODES) {
      it(`${system}/${mode}: rest, hover, focus and invalid edges, both Contrast levels`, () => {
        for (const contrast of ["aa", "high"] as const) {
          for (const anyAccent of ACCENTS[system]) {
            // The invalid edge is --st-due, which the colour-blind axis swaps for its own palette.
            for (const colorblind of [false, true]) {
              const v = themeVars(system, mode, anyAccent, colorblind, contrast);
              for (const against of SURFACES) {
                for (const role of ["ink4", "ink2", "accent-text", "st-due"]) {
                  expect(ratioOf(v, role, against)).toBeGreaterThanOrEqual(3);
                }
                // Hover is never a step down from rest.
                expect(ratioOf(v, "ink2", against)).toBeGreaterThanOrEqual(
                  ratioOf(v, "ink4", against),
                );
              }
            }
          }
        }
      });
    }
  }

  it("the old edge really did fail, so this block is measuring something", () => {
    // If --border2 ever reached 3:1 at the default Contrast this guard would be vacuous: say so.
    let worst = Infinity;
    for (const system of SYSTEMS) {
      for (const mode of MODES) {
        for (const anyAccent of ACCENTS[system]) {
          const v = themeVars(system, mode, anyAccent, false, "aa");
          for (const against of SURFACES) worst = Math.min(worst, ratioOf(v, "border2", against));
        }
      }
    }
    expect(worst).toBeLessThan(3);
  });
});
// ===== END form-control edges (#1105) =======================================================
