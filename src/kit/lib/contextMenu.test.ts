// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  closeContextMenu,
  currentContextMenu,
  fallbackItems,
  inEditable,
  isMenuKey,
  keyboardAnchor,
  openContextMenu,
  openMenuFromKeyboard,
  placeMenu,
  setFallbackActions,
  tidy,
  wantsAppMenu,
  type MenuItem,
} from "./contextMenu";

afterEach(() => {
  closeContextMenu();
  setFallbackActions(null);
  vi.restoreAllMocks();
});

const at = { clientX: 10, clientY: 20, preventDefault: () => {} };

const item = (label: string): MenuItem => ({ label, onSelect: () => {} });
const sep: MenuItem = { kind: "separator" };
const labels = (items: MenuItem[]) => items.map((i) => (i.kind === "separator" ? "—" : i.label));

function el(html: string): Element {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host.querySelector("[data-t]")!;
}

describe("tidy", () => {
  it("keeps separators only between groups", () => {
    expect(labels(tidy([sep, item("a"), sep, sep, item("b"), sep]))).toEqual(["a", "—", "b"]);
    expect(tidy([sep, sep])).toEqual([]);
  });
});

describe("inEditable", () => {
  it("is true in text you type into, and false elsewhere", () => {
    expect(inEditable(el('<input data-t type="text">'))).toBe(true);
    expect(inEditable(el("<input data-t>"))).toBe(true);
    expect(inEditable(el("<textarea data-t></textarea>"))).toBe(true);
    expect(inEditable(el('<div contenteditable="true"><p><span data-t>x</span></p></div>'))).toBe(
      true,
    );
    expect(inEditable(el('<input data-t type="checkbox">'))).toBe(false);
    expect(inEditable(el("<button data-t>x</button>"))).toBe(false);
    // A widget ProseMirror draws inside its editor (a checkbox marker) is not text.
    expect(
      inEditable(
        el('<div contenteditable="true"><span contenteditable="false" data-t></span></div>'),
      ),
    ).toBe(false);
    expect(inEditable(null)).toBe(false);
  });
});

describe("wantsAppMenu", () => {
  const target = el("<p data-t>words</p>");
  it("leaves the system menu to Shift, to text fields and to a surface that took it", () => {
    expect(wantsAppMenu({ defaultPrevented: false, shiftKey: false, target })).toBe(true);
    expect(wantsAppMenu({ defaultPrevented: false, shiftKey: true, target })).toBe(false);
    expect(wantsAppMenu({ defaultPrevented: true, shiftKey: false, target })).toBe(false);
    expect(
      wantsAppMenu({ defaultPrevented: false, shiftKey: false, target: el("<input data-t>") }),
    ).toBe(false);
  });
});

describe("placeMenu", () => {
  it("opens at the pointer when it fits", () => {
    expect(placeMenu(100, 100, 150, 200, 1000, 800)).toEqual({ left: 100, top: 100 });
  });
  it("flips left and up at the window's right and bottom edges", () => {
    expect(placeMenu(950, 700, 150, 200, 1000, 800)).toEqual({ left: 800, top: 500 });
  });
  it("stays inside a window smaller than the flip needs", () => {
    expect(placeMenu(50, 50, 150, 200, 120, 120)).toEqual({ left: 4, top: 4 });
  });
});

describe("fallbackItems", () => {
  const actions = () => ({ openUrl: vi.fn(), copy: vi.fn() });
  it("offers Copy for selected text", () => {
    const a = actions();
    const items = fallbackItems(el("<p data-t>hi</p>"), "hello", a);
    expect(labels(items)).toEqual(["Copy"]);
    (items[0] as { onSelect: () => void }).onSelect();
    expect(a.copy).toHaveBeenCalledWith("hello");
  });
  it("offers a web link's Open and Copy, with Copy for a selection after them", () => {
    const a = actions();
    const items = fallbackItems(el('<a href="https://example.com/x"><b data-t>x</b></a>'), "x", a);
    expect(labels(items)).toEqual(["Open link", "Copy link", "—", "Copy"]);
    (items[0] as { onSelect: () => void }).onSelect();
    expect(a.openUrl).toHaveBeenCalledWith("https://example.com/x");
  });
  it("offers a link's Copy alone where nothing can open a link (a mod's frame gives none)", () => {
    const copy = vi.fn();
    const items = fallbackItems(el('<a href="https://example.com/x"><b data-t>x</b></a>'), "", {
      copy,
    });
    expect(labels(items)).toEqual(["Copy link"]);
    (items[0] as { onSelect: () => void }).onSelect();
    // With the words a screen reader hears afterwards, since the menu closes with nothing to see.
    expect(copy).toHaveBeenCalledWith("https://example.com/x", "Link copied");
  });
  it("offers nothing for a click on nothing in particular, or for a link that isn't a web address", () => {
    expect(fallbackItems(el("<p data-t>hi</p>"), "  ", actions())).toEqual([]);
    expect(fallbackItems(el('<a data-t href="javascript:void 0">x</a>'), "", actions())).toEqual(
      [],
    );
  });
});

describe("isMenuKey", () => {
  const key = (
    k: string,
    mods: Partial<Record<"shiftKey" | "ctrlKey" | "altKey" | "metaKey", boolean>> = {},
  ) =>
    isMenuKey({ key: k, shiftKey: false, ctrlKey: false, altKey: false, metaKey: false, ...mods });
  it("is the Menu key, or Shift+F10 with nothing else held", () => {
    expect(key("ContextMenu")).toBe(true);
    // Shift is how the system menu is asked for, with a mouse and so with the Menu key.
    expect(key("ContextMenu", { shiftKey: true })).toBe(false);
    expect(key("ContextMenu", { ctrlKey: true })).toBe(false);
    expect(key("F10", { shiftKey: true })).toBe(true);
    expect(key("F10")).toBe(false);
    expect(key("F10", { shiftKey: true, ctrlKey: true })).toBe(false);
    expect(key("F10", { shiftKey: true, altKey: true })).toBe(false);
    expect(key("F10", { shiftKey: true, metaKey: true })).toBe(false);
    expect(key("F9", { shiftKey: true })).toBe(false);
  });
});

describe("keyboardAnchor", () => {
  const rect = (left: number, top: number, right: number, bottom: number) => ({
    left,
    top,
    right,
    bottom,
  });
  it("sits a little in from the top-left, no lower than a short element's bottom edge", () => {
    expect(keyboardAnchor(rect(100, 200, 180, 230), 1000, 800)).toEqual({ x: 104, y: 230 });
  });
  it("sits near the top of a tall element rather than in its middle", () => {
    expect(keyboardAnchor(rect(100, 200, 500, 700), 1000, 800)).toEqual({ x: 104, y: 248 });
  });
  it("stays inside the window for an element that is partly or wholly off it", () => {
    expect(keyboardAnchor(rect(-50, -40, 30, 20), 1000, 800)).toEqual({ x: 4, y: 20 });
    expect(keyboardAnchor(rect(990, 790, 1100, 900), 1000, 800)).toEqual({ x: 994, y: 800 });
    expect(keyboardAnchor(rect(0, 0, 0, 0), 1000, 800)).toEqual({ x: 0, y: 0 });
  });
});

describe("openMenuFromKeyboard", () => {
  function surface() {
    const wrap = document.createElement("div");
    const btn = document.createElement("button");
    wrap.append(btn);
    document.body.append(wrap);
    btn.getBoundingClientRect = () => ({ left: 100, top: 200, right: 180, bottom: 230 }) as DOMRect;
    return { wrap, btn };
  }

  it("sends the surface a right-click with no Shift, at the element, and says it was taken", () => {
    const { wrap, btn } = surface();
    let seen: MouseEvent | undefined;
    wrap.addEventListener("contextmenu", (e) => {
      seen = e;
      e.preventDefault();
    });
    expect(openMenuFromKeyboard(btn)).toBe(true);
    expect(seen).toMatchObject({ shiftKey: false, button: 2, clientX: 104, clientY: 230 });
    expect(seen!.bubbles && seen!.cancelable && seen!.composed).toBe(true);
    wrap.remove();
  });

  it("says nothing took it when no surface did, and marks only that open as the keyboard's", () => {
    const { wrap, btn } = surface();
    wrap.addEventListener("contextmenu", (e) => openContextMenu(e, "Card", [item("One")]));
    expect(openMenuFromKeyboard(btn)).toBe(true);
    expect(currentContextMenu()).toMatchObject({ viaKeyboard: true, x: 104, y: 230 });
    closeContextMenu();
    openContextMenu(at, "Card", [item("One")]);
    expect(currentContextMenu()!.viaKeyboard).toBe(false);
    wrap.remove();
    expect(openMenuFromKeyboard(null)).toBe(false);
  });

  // A stuck flag would make the next mouse click open its menu as if a key had.
  it("clears the keyboard mark even when a handler throws", () => {
    const { wrap, btn } = surface();
    vi.spyOn(btn, "dispatchEvent").mockImplementation(() => {
      throw new Error("boom");
    });
    expect(() => openMenuFromKeyboard(btn)).toThrow("boom");
    openContextMenu(at, "Card", [item("One")]);
    expect(currentContextMenu()!.viaKeyboard).toBe(false);
    wrap.remove();
  });

  // VoiceOver's "show menu" and WebKit's own Menu key send one with no position.
  it("puts a keyboard menu the engine made at 0,0 at the focused element instead", () => {
    const { wrap, btn } = surface();
    btn.focus();
    openContextMenu({ ...at, clientX: 0, clientY: 0, target: wrap }, "Card", [item("One")]);
    expect(currentContextMenu()).toMatchObject({ viaKeyboard: true, x: 104, y: 230 });
    closeContextMenu();
    // A click that merely lands on 0,0 of something that doesn't hold focus is left alone.
    btn.blur();
    openContextMenu({ ...at, clientX: 0, clientY: 0, target: wrap }, "Card", [item("One")]);
    expect(currentContextMenu()).toMatchObject({ viaKeyboard: false, x: 0, y: 0 });
    wrap.remove();
  });
});

describe("openContextMenu with the fallback", () => {
  const selecting = (text: string) =>
    vi.spyOn(window, "getSelection").mockReturnValue({ toString: () => text } as Selection);

  it("adds Copy for selected text after the surface's own items, only when asked", () => {
    const copy = vi.fn();
    setFallbackActions({ copy });
    selecting("some words");
    openContextMenu(at, "Card", [item("Duplicate")]);
    expect(labels(currentContextMenu()!.items)).toEqual(["Duplicate"]);
    openContextMenu(at, "Card", [item("Duplicate")], { withFallback: true });
    expect(labels(currentContextMenu()!.items)).toEqual(["Duplicate", "—", "Copy"]);
    (currentContextMenu()!.items[2] as { onSelect: () => void }).onSelect();
    expect(copy).toHaveBeenCalledWith("some words");
  });

  it("adds a link's Open and Copy through the opener the host registered", () => {
    const openUrl = vi.fn();
    setFallbackActions({ copy: vi.fn(), openUrl });
    selecting("");
    const target = el('<a href="https://example.com/x"><b data-t>x</b></a>');
    openContextMenu({ ...at, target }, "Card", [item("Duplicate")], { withFallback: true });
    expect(labels(currentContextMenu()!.items)).toEqual([
      "Duplicate",
      "—",
      "Open link",
      "Copy link",
    ]);
    (currentContextMenu()!.items[2] as { onSelect: () => void }).onSelect();
    expect(openUrl).toHaveBeenCalledWith("https://example.com/x");
  });

  it("leaves the surface's own items alone when there is nothing to add, or no host to ask", () => {
    selecting("");
    setFallbackActions({ copy: vi.fn() });
    openContextMenu(at, "Card", [item("Duplicate")], { withFallback: true });
    expect(labels(currentContextMenu()!.items)).toEqual(["Duplicate"]);
    setFallbackActions(null);
    selecting("words");
    openContextMenu(at, "Card", [item("Duplicate")], { withFallback: true });
    expect(labels(currentContextMenu()!.items)).toEqual(["Duplicate"]);
  });
});
