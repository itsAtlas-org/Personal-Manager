// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { buildShell, fillCard, fillLegend, setStageMessage } from "./shell";
import { last } from "./testing";
import type { MapDoc } from "./types";

const doc = (over: Partial<MapDoc> = {}): MapDoc => ({
  id: 1,
  title: "Quarterly plan",
  project: "Atlas",
  importance: "high",
  chunkCount: 12,
  reviewed: true,
  sourceType: "vault",
  tags: ["finance", "q3"],
  // Local noon, so the date shown is the 4th in whatever zone the tests run in.
  ingestedAt: new Date(2020, 2, 4, 12).toISOString(),
  ...over,
});

const text = (el: Element) => el.textContent ?? "";

describe("buildShell", () => {
  it("builds the header, strips, stage and card, with every strip and the card hidden", () => {
    const shell = buildShell(document);
    expect(text(shell.root.querySelector("h1") as Element)).toBe("Map");
    for (const el of [
      shell.computing,
      shell.notice,
      shell.error,
      shell.card,
      shell.settingsPanel,
    ]) {
      expect(el.hidden).toBe(true);
    }
    expect(shell.cohesionLabel.hidden).toBe(true);
    expect(shell.canvas.hidden).toBe(true);
    expect(shell.nav.hidden).toBe(true);
  });

  it("writes no style attribute and no inline event handler anywhere (the frame's CSP forbids both)", () => {
    const shell = buildShell(document);
    fillLegend(shell, document, [{ name: "A", color: "#60a5fa" }]);
    fillCard(shell, document, doc(), true);
    const everything = Array.from(shell.root.querySelectorAll("*"));
    expect(everything.length).toBeGreaterThan(30);
    for (const el of everything) {
      expect(el.hasAttribute("style")).toBe(false);
      for (const attr of Array.from(el.attributes)) expect(attr.name.startsWith("on")).toBe(false);
    }
  });

  it("offers the cohesion menu, the names switch, both arrangements and the settings popover", () => {
    const shell = buildShell(document);
    expect(Array.from(shell.cohesion.options).map((o) => [o.value, o.text])).toEqual([
      ["0", "Off"],
      ["0.15", "Low"],
      ["0.3", "Medium"],
      ["0.5", "High"],
    ]);
    expect(text(shell.names)).toBe("Names");
    expect(text(shell.byProject)).toBe("By project");
    expect(text(shell.semantic)).toBe("Semantic");
    expect(shell.nodeCap.type).toBe("number");
    expect(shell.nodeCap.min).toBe("200");
    expect(shell.nodeCap.max).toBe("5000");
  });

  it("labels what has no visible text: the zoom buttons, the canvas and the legend", () => {
    const shell = buildShell(document);
    expect(shell.zoomIn.getAttribute("aria-label")).toBe("Zoom in");
    expect(shell.zoomOut.getAttribute("aria-label")).toBe("Zoom out");
    expect(shell.fit.getAttribute("aria-label")).toBe("Fit to view");
    expect(shell.canvas.getAttribute("role")).toBe("img");
    expect(shell.legend.getAttribute("aria-label")).toBe("Projects");
  });

  it("points the settings button at its popover and announces the strips", () => {
    const shell = buildShell(document);
    expect(shell.settingsButton.getAttribute("aria-controls")).toBe(shell.settingsPanel.id);
    expect(shell.settingsButton.getAttribute("aria-expanded")).toBe("false");
    expect(shell.computing.getAttribute("role")).toBe("status");
    expect(shell.notice.getAttribute("role")).toBe("status");
    expect(shell.error.getAttribute("role")).toBe("alert");
  });

  it("ties each label to its control", () => {
    const shell = buildShell(document);
    const cohesion = shell.cohesionLabel as HTMLLabelElement;
    expect(cohesion.htmlFor).toBe(shell.cohesion.id);
    expect(shell.settingsPanel.querySelector(`label[for="${shell.nodeCap.id}"]`)).not.toBeNull();
  });
});

describe("setStageMessage", () => {
  it("shows the canvas and its controls for none, and no message", () => {
    const shell = buildShell(document);
    setStageMessage(shell, "none");
    expect(shell.canvas.hidden).toBe(false);
    expect(shell.nav.hidden).toBe(false);
    expect(shell.message.hidden).toBe(true);
  });

  it("shows the shimmer alone while loading", () => {
    const shell = buildShell(document);
    setStageMessage(shell, "loading");
    expect(shell.message.hidden).toBe(false);
    expect(shell.skeleton.hidden).toBe(false);
    expect(text(shell.messageText)).toBe("");
    expect(shell.canvas.hidden).toBe(true);
    expect(shell.nav.hidden).toBe(true);
  });

  it("shows the shimmer with a sentence while preparing", () => {
    const shell = buildShell(document);
    setStageMessage(shell, "preparing");
    expect(shell.skeleton.hidden).toBe(false);
    expect(text(shell.messageText)).toBe("Preparing the map…");
  });

  it("shows a sentence and no shimmer for an empty library and for nothing to place", () => {
    const shell = buildShell(document);
    setStageMessage(shell, "empty");
    expect(shell.skeleton.hidden).toBe(true);
    expect(text(shell.messageText)).toBe(
      "No documents yet. Ingest some in the Documents view and they'll appear here.",
    );
    setStageMessage(shell, "nothing-to-place");
    expect(text(shell.messageText)).toBe(
      "Nothing to place by meaning yet. Documents appear here once PM has read them.",
    );
    expect(shell.canvas.hidden).toBe(true);
  });

  it("shows nothing but the empty stage for a failure (the strip above says it)", () => {
    const shell = buildShell(document);
    setStageMessage(shell, "failed");
    expect(shell.skeleton.hidden).toBe(true);
    expect(text(shell.messageText)).toBe("");
    expect(shell.canvas.hidden).toBe(true);
  });
});

describe("fillLegend", () => {
  it("lists each project with a dot in its colour, set as an SVG attribute", () => {
    const shell = buildShell(document);
    fillLegend(shell, document, [
      { name: "Atlas", color: "#60a5fa" },
      { name: "Home", color: "#34d399" },
    ]);
    const items = Array.from(shell.legend.querySelectorAll("li"));
    expect(items.map(text)).toEqual(["Atlas", "Home"]);
    expect(items.map((li) => li.querySelector("circle")?.getAttribute("fill"))).toEqual([
      "#60a5fa",
      "#34d399",
    ]);
    expect(items[0].querySelector("svg")?.namespaceURI).toBe("http://www.w3.org/2000/svg");
    expect(items[0].querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");
  });

  it("replaces the old list, and empties it for no projects", () => {
    const shell = buildShell(document);
    fillLegend(shell, document, [{ name: "Atlas", color: "red" }]);
    fillLegend(shell, document, [{ name: "Home", color: "blue" }]);
    expect(shell.legend.querySelectorAll("li")).toHaveLength(1);
    expect(text(shell.legend)).toBe("Home");
    fillLegend(shell, document, []);
    expect(shell.legend.children).toHaveLength(0);
  });

  it("draws a project name as text, never as markup", () => {
    const shell = buildShell(document);
    const hostile = '<img src=x onerror="alert(1)"><b>x</b>';
    fillLegend(shell, document, [{ name: hostile, color: "red" }]);
    expect(shell.legend.querySelector("img")).toBeNull();
    expect(shell.legend.querySelector("b")).toBeNull();
    expect(text(shell.legend)).toBe(hostile);
  });
});

describe("fillCard", () => {
  const rows = (shell: ReturnType<typeof buildShell>) =>
    Array.from(shell.card.querySelectorAll(".card-row")).map((r) => [
      text(r.querySelector("dt") as Element),
      text(r.querySelector("dd") as Element),
    ]);

  it("shows the title, the facts and the tags", () => {
    const shell = buildShell(document);
    fillCard(shell, document, doc(), false);
    expect(shell.card.hidden).toBe(false);
    expect(text(shell.card.querySelector(".card-title") as Element)).toBe("Quarterly plan");
    expect(rows(shell)).toEqual([
      ["Project", "Atlas"],
      ["Importance", "high"],
      ["Chunks", "12"],
      ["Reviewed", "yes"],
    ]);
    expect(Array.from(shell.card.querySelectorAll(".tag")).map(text)).toEqual(["finance", "q3"]);
  });

  it("adds the date ingested at Power depth, DD-MM-YYYY, and nothing else", () => {
    const shell = buildShell(document);
    fillCard(shell, document, doc(), true);
    expect(last(rows(shell))).toEqual(["Ingested", "04-03-2020"]);
    expect(rows(shell)).toHaveLength(5);
  });

  it("never shows a path: a mod is not given one, and the card has no row for it", () => {
    const shell = buildShell(document);
    fillCard(shell, document, { ...doc(), sourcePath: "/home/me/secret.pdf" } as MapDoc, true);
    expect(text(shell.card)).not.toContain("secret.pdf");
    expect(rows(shell).map((r) => r[0])).not.toContain("Path");
  });

  it("says awaiting review for an unreviewed document, a dash for no importance, Unsorted for no project", () => {
    const shell = buildShell(document);
    fillCard(shell, document, doc({ reviewed: false, importance: null, project: "" }), false);
    expect(rows(shell)).toEqual([
      ["Project", "Unsorted"],
      ["Importance", "—"],
      ["Chunks", "12"],
      ["Reviewed", "awaiting review"],
    ]);
  });

  it("leaves out the tags row for a document with none", () => {
    const shell = buildShell(document);
    fillCard(shell, document, doc({ tags: [] }), false);
    expect(shell.card.querySelector(".card-tags")).toBeNull();
  });

  it("hides and empties for no document", () => {
    const shell = buildShell(document);
    fillCard(shell, document, doc(), false);
    fillCard(shell, document, null, false);
    expect(shell.card.hidden).toBe(true);
    expect(shell.card.children).toHaveLength(0);
  });

  it("replaces the previous document's card", () => {
    const shell = buildShell(document);
    fillCard(shell, document, doc({ title: "First" }), false);
    fillCard(shell, document, doc({ title: "Second", tags: [] }), false);
    expect(shell.card.querySelectorAll(".card-title")).toHaveLength(1);
    expect(text(shell.card)).toContain("Second");
    expect(text(shell.card)).not.toContain("First");
  });

  it("draws every document-supplied string as text, never as markup", () => {
    const shell = buildShell(document);
    const hostile = '<img src=x onerror="alert(1)">';
    fillCard(
      shell,
      document,
      doc({ title: hostile, project: hostile, importance: hostile, tags: [hostile] }),
      false,
    );
    expect(shell.card.querySelector("img")).toBeNull();
    expect(text(shell.card.querySelector(".card-title") as Element)).toBe(hostile);
    expect(text(shell.card.querySelector(".tag") as Element)).toBe(hostile);
  });
});
