// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What App does with a link from the pop-out Calendar. The pop-out is another webview and its
// `main://navigate` payload is a string, so the main window must run it through `navigateTarget` and
// open only what that returns (the pure function is tested in nav.test.ts). App has no test of its
// own, so this source scan is what keeps the handler from becoming `setView(to as View)`, which would
// let the pop-out put `dev`, `mods` or a malformed `mod:` id into the main window's view state.

import { describe, expect, it } from "vitest";

import appSource from "../App.tsx?raw";

/** The body of the `main://navigate` handler: from its assignment to the first line that closes it. */
function navigateHandler(): string {
  const start = appSource.indexOf("navigateRef.current = (to, project) => {");
  expect(
    start,
    "the handler is no longer assigned as `navigateRef.current = (to, project) => {`",
  ).toBeGreaterThan(-1);
  const end = appSource.indexOf("\n  };", start);
  expect(end).toBeGreaterThan(start);
  return appSource.slice(start, end);
}

describe("App's handling of the pop-out Calendar's links", () => {
  it("opens only what navigateTarget returns", () => {
    const handler = navigateHandler();
    expect(handler).toContain("navigateTarget(to, project)");
    expect(handler).toContain("setView(target.view)");
    expect(handler).toContain("openProject(target.project)");
  });

  it("never sets the view from the pop-out's own string", () => {
    expect(navigateHandler()).not.toMatch(/setView\(\s*to\b/);
    expect(appSource).not.toMatch(/setView\(\s*to\b/);
  });

  it("hands every event from the pop-out to that handler", () => {
    expect(appSource).toContain("onMainNavigate((to) => navigateRef.current(to.view, to.project))");
  });

  it("opens the Pinboard's mod tab from a Calendar dot, and has no core `pinboard` view left", () => {
    expect(appSource).toContain("onOpenPinboard={() => setView(PINBOARD_VIEW)}");
    expect(appSource).not.toMatch(/setView\(\s*["'`]pinboard["'`]/);
  });
});
