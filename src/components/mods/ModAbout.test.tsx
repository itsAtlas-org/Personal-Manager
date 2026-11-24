// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// "About this mod" (B7) with the first real content's shape: the Map's own help entries, drawn
// through the same component the frame uses. ModFrame.test holds the panel's behaviour over a frame
// and the hostile-text cases; this holds that the Map's words render whole and as written.

import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import ModAboutSource from "./ModAbout.tsx?raw";
import { ModAbout } from "./ModAbout";
import { hasAbout } from "./modText";

// Button reads the theme (the real provider pulls in IPC), so the same stub the other component
// tests use.
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    modePref: "system",
    modeSource: "system",
    accent: "mono",
    colorblind: false,
    depth: "standard",
    autoLocation: "",
  }),
}));

afterEach(cleanup);

const HELP = [
  {
    id: "map-overview",
    title: "Document Map",
    body: "Your documents laid out on one map, in one of two arrangements. By project puts each project in the middle of its documents. Semantic puts documents that are about similar things close together.\nHover a document to see its details, or click it to read it.",
  },
  {
    id: "map-canvas",
    title: "What the dots mean",
    body: "Each dot is a document. Its size is how much content it holds (its number of chunks) and its colour is its project, matching the legend.\nA dashed ring means the document is still awaiting review, or is index-only (it lives in a connected source like Drive rather than on this device).\nIn the Semantic arrangement a faded dot has not been placed yet: it waits at its project's spot until the map is worked out again.\nHover a dot for its details, or click it to open the document in PM's reader.",
  },
  {
    id: "map-settings",
    title: "Map settings",
    body: "Most documents placed by meaning caps how many documents the Semantic arrangement places one by one, from 200 to 5,000. Above it, the rest are gathered at their project's spot. Raise it for a fuller picture, lower it on a slower computer.\nYour settings are kept with your vault, so they travel with a backup and with a shared vault.",
  },
];
const DESCRIPTION = "Shows your documents.";

// Entries shaped like the Map's own (`mods/atlas.map/manifest.json`, B6): one paragraph per line,
// plain sentences with a few characters Markdown could take for syntax (colons, parentheses,
// apostrophes). PM's code never imports a mod's source (I-25), so the real manifest is checked on the
// Rust side (`listing::tests::the_committed_map_manifests_help_reaches_the_listing_whole`) and these
// are verbatim copies of three of its entries for the render.
describe("a Map-shaped mod's help in the About panel", () => {
  it("shows every entry, each title as a heading and every line of its body as written", () => {
    render(<ModAbout description={DESCRIPTION} help={HELP} onClose={() => {}} />);
    const panel = document.querySelector('[data-help="mod-about"]') as HTMLElement;
    const headings = within(panel)
      .getAllByRole("heading", { level: 3 })
      .map((h) => h.textContent);
    expect(headings).toEqual(HELP.map((h) => h.title));
    // Every line the maker wrote is on the page, as written: a paragraph per line, none lost to
    // Markdown reading a character as syntax.
    const shown = [...panel.querySelectorAll("p")].map((p) => p.textContent ?? "");
    for (const h of HELP) {
      for (const line of h.body.split("\n")) {
        expect(
          shown.some((p) => p.split("\n").includes(line)),
          `${h.id}: ${line}`,
        ).toBe(true);
      }
    }
    expect(
      panel.querySelector(
        ".pm-mod-help :is(a, img, script, ul, ol, h1, h2, h3, h4, blockquote, pre)",
      ),
    ).toBeNull();
  });

  it("is there to open: with help and a description hasAbout says yes, with neither no", () => {
    expect(hasAbout(DESCRIPTION, HELP)).toBe(true);
    expect(hasAbout("", [])).toBe(false);
  });
});

describe("ModAbout", () => {
  it("says who made the mod, in PM's own small line above the maker's words", () => {
    render(
      <ModAbout
        description={DESCRIPTION}
        help={HELP.slice(0, 1)}
        credit="Atlas · author: Bobby Yu"
        onClose={() => {}}
      />,
    );
    const panel = document.querySelector('[data-help="mod-about"]') as HTMLElement;
    expect(within(panel).getByText("Made by Atlas · author: Bobby Yu")).toBeTruthy();
  });

  it("draws no credit line when the mod has none to show", () => {
    render(<ModAbout description={DESCRIPTION} help={HELP.slice(0, 1)} onClose={() => {}} />);
    expect(screen.queryByText(/^Made by/)).toBeNull();
  });

  it("calls onClose from its button", () => {
    const onClose = vi.fn();
    render(<ModAbout description="" help={HELP.slice(0, 1)} onClose={onClose} />);
    screen.getByRole("button", { name: "Close" }).click();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("gives an untitled entry a plain heading and drops an entry with no body", () => {
    render(
      <ModAbout
        description=""
        help={[
          { id: "a", title: " ​ ", body: "Words." },
          { id: "b", title: "Empty", body: "​" },
        ]}
        onClose={() => {}}
      />,
    );
    const headings = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
    expect(headings).toEqual(["Help"]);
  });

  it("never builds markup from a mod's words: no HTML sink in its code", () => {
    // Comments left out: the file's header names the sinks it refuses.
    const code = ModAboutSource.replace(/^\s*\/\/.*$/gm, "");
    expect(code).not.toMatch(/dangerouslySetInnerHTML|innerHTML|outerHTML|insertAdjacentHTML/);
    // The body goes through PM's one sanitizing component, with links off.
    expect(code).toMatch(/<Markdown links=\{false\}>/);
  });
});
