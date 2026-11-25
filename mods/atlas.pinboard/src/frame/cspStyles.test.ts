// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The mirror of a library's <style> elements into adopted sheets (cspStyles.ts). jsdom has no
// constructable stylesheets, so a stand-in `CSSStyleSheet` (text kept by `replaceSync`) and an
// `adoptedStyleSheets` list on the document are put in place and taken away after each test. That the
// real engine applies such a sheet under the frame's policy is measured in WebKitGTK, not here
// (the C1 CSP harness).

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { layered, MIRROR_LAYER, startCspStyles } from "./cspStyles";

class FakeSheet {
  static made = 0;
  text = "";
  media: string | null;
  replaced = 0;
  constructor(options?: { media?: string }) {
    this.media = options?.media ?? null;
    FakeSheet.made += 1;
  }
  replaceSync(text: string) {
    this.text = text;
    this.replaced += 1;
  }
}

const win = window as unknown as Record<string, unknown>;
let savedSheet: unknown;
let stop: (() => void) | null = null;

const adopted = () =>
  (document as unknown as { adoptedStyleSheets: FakeSheet[] }).adoptedStyleSheets;

/** Mutation records are delivered as a microtask; one macrotask later they have all been handled. */
const settle = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

function style(text: string, parent: Element = document.head): HTMLStyleElement {
  const el = document.createElement("style");
  el.textContent = text;
  parent.appendChild(el);
  return el;
}

beforeEach(() => {
  savedSheet = win.CSSStyleSheet;
  win.CSSStyleSheet = FakeSheet;
  FakeSheet.made = 0;
  Object.defineProperty(document, "adoptedStyleSheets", {
    configurable: true,
    writable: true,
    value: [],
  });
  document.head.innerHTML = "";
  document.body.innerHTML = "";
});

afterEach(() => {
  stop?.();
  stop = null;
  win.CSSStyleSheet = savedSheet;
  delete (document as unknown as Record<string, unknown>).adoptedStyleSheets;
  document.head.innerHTML = "";
});

describe("startCspStyles", () => {
  it("mirrors a <style> added after it started, in the layer", async () => {
    stop = startCspStyles(document);
    expect(adopted()).toHaveLength(0);
    style(".a { color: red }");
    await settle();
    expect(adopted()).toHaveLength(1);
    expect(adopted()[0].text).toBe(layered(".a { color: red }"));
    expect(adopted()[0].text.startsWith(`@layer ${MIRROR_LAYER} {`)).toBe(true);
  });

  it("mirrors a <style> that was already there", () => {
    style(".early { color: blue }");
    stop = startCspStyles(document);
    expect(adopted()).toHaveLength(1);
    expect(adopted()[0].text).toContain(".early");
  });

  it("updates the sheet when the element's text is replaced", async () => {
    stop = startCspStyles(document);
    const el = style(".a { color: red }");
    await settle();
    const sheet = adopted()[0];
    el.textContent = ".a { color: green } .b { color: blue }";
    await settle();
    expect(adopted()).toHaveLength(1);
    expect(adopted()[0]).toBe(sheet);
    expect(sheet.text).toBe(layered(".a { color: green } .b { color: blue }"));
  });

  it("updates the sheet when the text node's data is edited in place", async () => {
    stop = startCspStyles(document);
    const el = style(".a { color: red }");
    await settle();
    (el.firstChild as Text).data = ".a { color: purple }";
    await settle();
    expect(adopted()[0].text).toBe(layered(".a { color: purple }"));
  });

  it("drops the sheet of a removed element, and takes it back if the element returns", async () => {
    stop = startCspStyles(document);
    const keep = style(".keep {}");
    const gone = style(".gone {}");
    await settle();
    expect(adopted()).toHaveLength(2);
    gone.remove();
    await settle();
    expect(adopted()).toHaveLength(1);
    expect(adopted()[0].text).toContain(".keep");
    document.head.appendChild(gone);
    await settle();
    expect(adopted()).toHaveLength(2);
    expect(keep.isConnected).toBe(true);
  });

  it("keeps the sheets in the order of the elements, after any sheet someone else adopted", async () => {
    const foreign = new FakeSheet();
    adopted().push(foreign);
    stop = startCspStyles(document);
    const first = style(".one {}");
    style(".two {}");
    await settle();
    // style-mod puts its element FIRST in the head, so a later mount lands before the earlier ones
    const third = document.createElement("style");
    third.textContent = ".three {}";
    document.head.insertBefore(third, first);
    await settle();
    expect(adopted()[0]).toBe(foreign);
    expect(
      adopted()
        .slice(1)
        .map((s) => s.text),
    ).toEqual([layered(".three {}"), layered(".one {}"), layered(".two {}")]);
  });

  it("leaves the mod's own <link> stylesheet, and anything outside the head, alone", async () => {
    stop = startCspStyles(document);
    const link = document.createElement("link");
    link.rel = "stylesheet";
    link.href = "./assets/style.css";
    document.head.appendChild(link);
    style(".in-body {}", document.body);
    await settle();
    expect(adopted()).toHaveLength(0);
    expect(FakeSheet.made).toBe(0);
    expect(link.isConnected).toBe(true);
  });

  it("passes the element's media on to its sheet", async () => {
    stop = startCspStyles(document);
    const el = style(".a {}");
    el.media = "print";
    await settle();
    expect(adopted()).toHaveLength(1);
    expect(adopted()[0].media).toBe("print");
  });

  it("never throws into the page when the engine rejects a text, and mirrors the rest", async () => {
    stop = startCspStyles(document);
    const original = FakeSheet.prototype.replaceSync;
    FakeSheet.prototype.replaceSync = function (this: FakeSheet, text: string) {
      if (text.includes("BAD")) throw new SyntaxError("nope");
      original.call(this, text);
    };
    try {
      style(".BAD {}");
      style(".fine {}");
      await settle();
    } finally {
      FakeSheet.prototype.replaceSync = original;
    }
    expect(adopted().map((s) => s.text)).toContain(layered(".fine {}"));
  });

  it("stops: disconnects and takes its sheets away, leaving others' alone", async () => {
    const foreign = new FakeSheet();
    adopted().push(foreign);
    const end = startCspStyles(document);
    style(".a {}");
    await settle();
    expect(adopted()).toHaveLength(2);
    end();
    expect(adopted()).toEqual([foreign]);
    style(".late {}");
    await settle();
    expect(adopted()).toEqual([foreign]);
  });

  it("does nothing, and says nothing, where constructable sheets are not supported", async () => {
    // no CSSStyleSheet at all
    win.CSSStyleSheet = undefined;
    const noClass = startCspStyles(document);
    // a CSSStyleSheet without replaceSync (an older engine)
    win.CSSStyleSheet = class {};
    const noReplace = startCspStyles(document);
    style(".a {}");
    await settle();
    expect(adopted()).toHaveLength(0);
    // a document without adoptedStyleSheets
    win.CSSStyleSheet = FakeSheet;
    Object.defineProperty(document, "adoptedStyleSheets", { configurable: true, value: undefined });
    const noAdopt = startCspStyles(document);
    style(".b {}");
    await settle();
    expect(FakeSheet.made).toBe(0);
    for (const end of [noClass, noReplace, noAdopt]) expect(() => end()).not.toThrow();
  });
});
