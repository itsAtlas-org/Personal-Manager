// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { THEME_FALLBACK } from "./palette";
import {
  applyTheme,
  canvasColors,
  DEFAULT_THEME,
  readTheme,
  type MapTheme,
  type ThemeTarget,
} from "./theme";

/** An element's style as a plain record, so a test sees exactly what was set and removed. */
function target(): ThemeTarget & { props: Map<string, string>; log: string[] } {
  const props = new Map<string, string>();
  const log: string[] = [];
  return {
    props,
    log,
    dataset: {},
    style: {
      colorScheme: "",
      setProperty(name, value) {
        log.push(`set ${name}`);
        props.set(name, value);
      },
      removeProperty(name) {
        log.push(`remove ${name}`);
        const old = props.get(name) ?? "";
        props.delete(name);
        return old;
      },
    },
  };
}

const theme = (over: Partial<MapTheme> = {}): MapTheme => ({
  vars: { "--bg": "oklch(0.2 0 0)", "--ink": "oklch(0.9 0 0)" },
  mode: "light",
  colorblind: true,
  depth: "power",
  ...over,
});

describe("readTheme", () => {
  it("reads what PM's theme.get answers", () => {
    const read = readTheme({
      vars: { "--bg": "oklch(0.2 0 0)", "--radius": "12px" },
      mode: "light",
      system: "slate",
      accent: "mono",
      colorblind: true,
      depth: "power",
    });
    expect(read).toEqual({
      vars: { "--bg": "oklch(0.2 0 0)", "--radius": "12px" },
      mode: "light",
      colorblind: true,
      depth: "power",
    });
  });

  it.each([
    ["null", null],
    ["a string", "theme"],
    ["a number", 3],
    ["an array", []],
    ["no vars", { mode: "dark" }],
    ["vars that are not an object", { vars: "x" }],
    ["vars that are a list", { vars: ["--bg"] }],
  ])("is no theme for %s", (_name, raw) => {
    expect(readTheme(raw)).toBeNull();
  });

  it("is dark unless PM says light (PM resolves system and auto itself)", () => {
    expect(readTheme({ vars: {}, mode: "light" })?.mode).toBe("light");
    for (const mode of ["dark", "system", "auto", "", 3, null, undefined]) {
      expect(readTheme({ vars: {}, mode })?.mode).toBe("dark");
    }
  });

  it("takes the colour-blind switch only when it is exactly true", () => {
    expect(readTheme({ vars: {}, colorblind: true })?.colorblind).toBe(true);
    for (const colorblind of [false, "true", 1, null, undefined]) {
      expect(readTheme({ vars: {}, colorblind })?.colorblind).toBe(false);
    }
  });

  it("knows three depths and calls anything else the middle one", () => {
    expect(readTheme({ vars: {}, depth: "min" })?.depth).toBe("min");
    expect(readTheme({ vars: {}, depth: "standard" })?.depth).toBe("standard");
    expect(readTheme({ vars: {}, depth: "power" })?.depth).toBe("power");
    for (const depth of ["minimal", "POWER", 2, null, undefined]) {
      expect(readTheme({ vars: {}, depth })?.depth).toBe("standard");
    }
  });

  it("leaves out a property that is not a plain custom property with a short text value", () => {
    const long = "x".repeat(513);
    const read = readTheme({
      vars: {
        "--ok": "1px",
        "--Upper": "1px",
        "-single": "1px",
        color: "red",
        "--": "1px",
        "--has space": "1px",
        "--number": 3,
        "--object": { a: 1 },
        "--null": null,
        "--long": long,
        ["--" + "n".repeat(64)]: "1px",
        "--edge": "x".repeat(512),
      },
    });
    expect(Object.keys(read?.vars ?? {}).sort()).toEqual(["--edge", "--ok"]);
  });

  it("reads at most 200 properties", () => {
    const vars: Record<string, string> = {};
    for (let i = 0; i < 300; i++) vars[`--v${i}`] = "1";
    expect(Object.keys(readTheme({ vars })?.vars ?? {})).toHaveLength(200);
  });
});

describe("applyTheme", () => {
  it("sets every property, the colour scheme for native controls, and the mode and depth hooks", () => {
    const root = target();
    const names = applyTheme(root, theme());
    expect(root.props.get("--bg")).toBe("oklch(0.2 0 0)");
    expect(root.props.get("--ink")).toBe("oklch(0.9 0 0)");
    expect(root.style.colorScheme).toBe("light");
    expect(root.dataset.mode).toBe("light");
    expect(root.dataset.depth).toBe("power");
    expect([...names].sort()).toEqual(["--bg", "--ink"]);
  });

  it("removes a property an earlier theme had and this one does not, and only that", () => {
    const root = target();
    const first = applyTheme(root, theme({ vars: { "--a": "1", "--b": "2" } }));
    root.log.length = 0;
    const second = applyTheme(root, theme({ vars: { "--b": "3", "--c": "4" } }), first);
    expect(root.props.get("--a")).toBeUndefined();
    expect(root.props.get("--b")).toBe("3");
    expect(root.props.get("--c")).toBe("4");
    expect(root.log).toContain("remove --a");
    expect(root.log).not.toContain("remove --b");
    expect([...second].sort()).toEqual(["--b", "--c"]);
  });

  it("follows a change of mode", () => {
    const root = target();
    applyTheme(root, theme({ mode: "light" }));
    applyTheme(root, theme({ mode: "dark" }));
    expect(root.style.colorScheme).toBe("dark");
    expect(root.dataset.mode).toBe("dark");
  });

  it("applies the default theme without a property", () => {
    const root = target();
    applyTheme(root, DEFAULT_THEME);
    expect(root.props.size).toBe(0);
    expect(root.style.colorScheme).toBe("dark");
  });
});

describe("canvasColors", () => {
  it("takes the six colours the canvas needs from the snapshot", () => {
    const colors = canvasColors(
      theme({
        vars: {
          "--border": "b",
          "--bg": "g",
          "--ink": "i",
          "--ink2": "i2",
          "--st-look": "l",
          "--ui": "'Hanken Grotesk',system-ui,sans-serif",
        },
      }),
    );
    expect(colors).toEqual({
      border: "b",
      bg: "g",
      ink: "i",
      ink2: "i2",
      stLook: "l",
      uiFont: "'Hanken Grotesk',system-ui,sans-serif",
    });
  });

  it("uses a mid-grey for a missing or blank colour and the system face for a missing font", () => {
    const colors = canvasColors(theme({ vars: { "--ink": "  ", "--bg": "g" } }));
    expect(colors.ink).toBe(THEME_FALLBACK);
    expect(colors.border).toBe(THEME_FALLBACK);
    expect(colors.bg).toBe("g");
    expect(colors.uiFont).toBe("system-ui, sans-serif");
  });

  it("trims what it takes", () => {
    expect(canvasColors(theme({ vars: { "--ink": "  red " } })).ink).toBe("red");
  });
});
