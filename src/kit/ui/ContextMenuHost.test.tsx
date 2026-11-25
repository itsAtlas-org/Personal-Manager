// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The kit's own host, the one a mod's frame mounts (a frame is a page of its own, so it draws its own
// menu and answers its own right-clicks). It holds no way to open a link: the host is given one, and
// with none a link's menu offers Copy link alone. Core's host (components/ui/ContextMenuHost.test.tsx)
// is this one given `openLink`, and keeps its own tests.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  closeContextMenu,
  copyViaHost,
  currentContextMenu,
  openContextMenu,
  wantsAppMenu,
} from "../lib/contextMenu";
import { ContextMenuHost } from "./ContextMenuHost";

afterEach(() => {
  act(() => closeContextMenu());
  cleanup();
  vi.restoreAllMocks();
  Reflect.deleteProperty(navigator, "clipboard");
});

function rightClickLink() {
  render(
    <>
      <a href="https://example.com/x">a link</a>
    </>,
  );
}

describe("the kit's ContextMenuHost", () => {
  it("draws a surface's menu and runs a choice", () => {
    const onSelect = vi.fn();
    render(<ContextMenuHost />);
    act(() =>
      openContextMenu({ clientX: 1, clientY: 2, preventDefault: () => {} }, "Card", [
        { label: "Duplicate", onSelect },
      ]),
    );
    fireEvent.click(screen.getByRole("menuitem", { name: "Duplicate" }));
    expect(onSelect).toHaveBeenCalledOnce();
  });

  it("offers Open link through the opener it is given, and Copy link", () => {
    const openUrl = vi.fn();
    rightClickLink();
    render(<ContextMenuHost openUrl={openUrl} />);
    fireEvent.contextMenu(screen.getByText("a link"));
    const labels = screen.getAllByRole("menuitem").map((i) => i.textContent);
    expect(labels).toEqual(["Open link", "Copy link"]);
    fireEvent.click(screen.getByRole("menuitem", { name: "Open link" }));
    expect(openUrl).toHaveBeenCalledWith("https://example.com/x");
  });

  it("offers Copy link alone where it was given no opener, and still no system menu", () => {
    rightClickLink();
    render(<ContextMenuHost />);
    const ev = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    act(() => {
      screen.getByText("a link").dispatchEvent(ev);
    });
    expect(screen.getAllByRole("menuitem").map((i) => i.textContent)).toEqual(["Copy link"]);
    expect(ev.defaultPrevented).toBe(true);
  });
});

const at = { clientX: 10, clientY: 20, preventDefault: () => {} };
const noop = () => {};

/** A card that opens a menu from its own `onContextMenu`, as the Pinboard and the sidebar do, with a
 *  button inside it to hold focus and a text field that must keep the system menu. */
function Card({ onSelect = noop }: { onSelect?: () => void }) {
  return (
    <div
      onContextMenu={(e) => {
        if (!wantsAppMenu(e)) return;
        openContextMenu(e, "Card", [
          { label: "Duplicate", onSelect },
          { label: "Delete", onSelect: noop },
        ]);
      }}
    >
      <button>card</button>
      <input aria-label="title" />
    </div>
  );
}

function stubRect(el: HTMLElement) {
  el.getBoundingClientRect = () => ({ left: 100, top: 200, right: 180, bottom: 230 }) as DOMRect;
}

describe("opening the menu from the keyboard", () => {
  it.each([
    ["the Menu key", { key: "ContextMenu" }],
    ["Shift+F10", { key: "F10", shiftKey: true }],
  ])("%s on a focused element opens the surface's menu there, on its first item", (_n, key) => {
    render(
      <>
        <Card />
        <ContextMenuHost />
      </>,
    );
    const btn = screen.getByRole("button", { name: "card" });
    stubRect(btn);
    btn.focus();
    const events: MouseEvent[] = [];
    document.addEventListener("contextmenu", (e) => events.push(e), { once: true });
    // fireEvent returns false when something prevented the key: no system menu on top of ours.
    expect(fireEvent.keyDown(btn, key)).toBe(false);
    screen.getByRole("menu", { name: "Card" });
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Duplicate" }));
    expect(currentContextMenu()).toMatchObject({ x: 104, y: 230, viaKeyboard: true });
    // Shift+F10 must reach the surface without its Shift, or the surface gives the system menu.
    expect(events[0].shiftKey).toBe(false);
  });

  it("does nothing in a text field, which keeps the system menu on those keys", () => {
    render(
      <>
        <Card />
        <ContextMenuHost />
      </>,
    );
    const input = screen.getByRole("textbox", { name: "title" });
    input.focus();
    expect(fireEvent.keyDown(input, { key: "ContextMenu" })).toBe(true);
    expect(fireEvent.keyDown(input, { key: "F10", shiftKey: true })).toBe(true);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("leaves other keys, F10 alone and held repeats be", () => {
    render(
      <>
        <Card />
        <ContextMenuHost />
      </>,
    );
    const btn = screen.getByRole("button", { name: "card" });
    btn.focus();
    expect(fireEvent.keyDown(btn, { key: "F10" })).toBe(true);
    expect(fireEvent.keyDown(btn, { key: "ContextMenu", repeat: true })).toBe(true);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("puts the menu away again on the same key, giving focus back", () => {
    render(
      <>
        <Card />
        <ContextMenuHost />
      </>,
    );
    const btn = screen.getByRole("button", { name: "card" });
    btn.focus();
    fireEvent.keyDown(btn, { key: "ContextMenu" });
    const first = screen.getByRole("menuitem", { name: "Duplicate" });
    expect(document.activeElement).toBe(first);
    fireEvent.keyDown(first, { key: "ContextMenu" });
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(btn);
  });

  // A surface that claims every right-click without asking (no `wantsAppMenu`) must not get one from
  // a key pressed in text: the key is for the system menu there, as the mouse is.
  it("does not even ask a surface when focus is in a text field", () => {
    const claim = vi.fn((e: { preventDefault: () => void }) => e.preventDefault());
    render(
      <>
        <div onContextMenu={claim}>
          <input aria-label="title" />
        </div>
        <ContextMenuHost />
      </>,
    );
    const input = screen.getByRole("textbox");
    input.focus();
    expect(fireEvent.keyDown(input, { key: "ContextMenu" })).toBe(true);
    expect(claim).not.toHaveBeenCalled();
  });

  it("falls to the link menu where no surface claims the element", () => {
    render(
      <>
        <a href="https://example.com/x">a link</a>
        <ContextMenuHost />
      </>,
    );
    const link = screen.getByRole("link");
    link.focus();
    expect(fireEvent.keyDown(link, { key: "ContextMenu" })).toBe(false);
    expect(screen.getAllByRole("menuitem").map((i) => i.textContent)).toEqual(["Copy link"]);
  });

  // An engine may send its own contextmenu at key-up; opened on top of ours it is the system menu.
  // jsdom won't let a test make an event `isTrusted`, so take the host's own listener and hand it one.
  function swallowRig() {
    const captured: ((e: MouseEvent) => void)[] = [];
    const add = document.addEventListener.bind(document);
    vi.spyOn(document, "addEventListener").mockImplementation((type, fn, opts) => {
      if (type === "contextmenu" && opts === true) captured.push(fn as (e: MouseEvent) => void);
      add(type, fn, opts);
    });
    render(
      <>
        <Card />
        <ContextMenuHost />
      </>,
    );
    const btn = screen.getByRole("button", { name: "card" });
    btn.focus();
    const native = (isTrusted: boolean) =>
      ({
        isTrusted,
        preventDefault: vi.fn(),
        stopImmediatePropagation: vi.fn(),
      }) as unknown as MouseEvent;
    return { btn, native, native0: captured };
  }

  // Windows sends its own contextmenu at key-UP, however long the key was held.
  it("still throws it away when the key was held past the grace period", () => {
    const { btn, native, native0 } = swallowRig();
    fireEvent.keyDown(btn, { key: "ContextMenu" });
    vi.spyOn(performance, "now").mockReturnValue(performance.now() + 3000);
    const held = native(true);
    native0[0](held);
    expect(held.preventDefault).toHaveBeenCalled();
  });

  it("lets a real right-click through when the mouse was pressed after the key", () => {
    const { btn, native, native0 } = swallowRig();
    fireEvent.keyDown(btn, { key: "ContextMenu" });
    fireEvent.mouseDown(btn, { button: 2 });
    const click = native(true);
    native0[0](click);
    expect(click.preventDefault).not.toHaveBeenCalled();
  });

  // The press that puts the menu away is a press too, and Windows follows it with its own event.
  it("throws away the contextmenu that follows the key closing the menu", () => {
    const { btn, native, native0 } = swallowRig();
    fireEvent.keyDown(btn, { key: "ContextMenu" });
    fireEvent.keyUp(btn, { key: "ContextMenu" });
    // Long after the opening press: its own guard has run out.
    vi.spyOn(performance, "now").mockReturnValue(performance.now() + 3000);
    fireEvent.keyDown(document.activeElement as HTMLElement, { key: "ContextMenu" });
    expect(screen.queryByRole("menu")).toBeNull();
    const followUp = native(true);
    native0[0](followUp);
    expect(followUp.preventDefault).toHaveBeenCalled();
  });

  it("throws away a real contextmenu that follows the key, but not a later one", () => {
    const { btn, native, native0: captured } = swallowRig();
    const early = native(true);
    fireEvent.keyDown(btn, { key: "ContextMenu" });
    captured[0](early);
    expect(early.preventDefault).toHaveBeenCalled();
    expect(early.stopImmediatePropagation).toHaveBeenCalled();
    // Our own synthetic one is not trusted, and must get through.
    const own = native(false);
    captured[0](own);
    expect(own.preventDefault).not.toHaveBeenCalled();
    // The key comes up: a contextmenu right then is still its own, a real right-click later is not.
    fireEvent.keyUp(btn, { key: "ContextMenu" });
    const atKeyUp = native(true);
    captured[0](atKeyUp);
    expect(atKeyUp.preventDefault).toHaveBeenCalled();
    const now = performance.now();
    vi.spyOn(performance, "now").mockReturnValue(now + 600);
    const late = native(true);
    captured[0](late);
    expect(late.preventDefault).not.toHaveBeenCalled();
  });
});

describe("focus", () => {
  // A browser will not focus an element that is `visibility: hidden`, which the menu is until it has
  // been measured and placed; jsdom doesn't know that, so look at the style at the moment of focus.
  it("is given to the menu only once it is showing", () => {
    const real = HTMLElement.prototype.focus;
    const seen: string[] = [];
    vi.spyOn(HTMLElement.prototype, "focus").mockImplementation(function (this: HTMLElement) {
      if (this.getAttribute("role") === "menu") seen.push(this.style.visibility);
      real.call(this);
    });
    render(<ContextMenuHost />);
    act(() => openContextMenu(at, "Board", [{ label: "One", onSelect: noop }]));
    expect(seen).toEqual([""]);
    expect(document.activeElement).toBe(screen.getByRole("menu"));
  });

  it("goes back to where it was after a choice that didn't move it", async () => {
    const onSelect = vi.fn();
    render(
      <>
        <Card onSelect={onSelect} />
        <ContextMenuHost />
      </>,
    );
    const btn = screen.getByRole("button", { name: "card" });
    btn.focus();
    fireEvent.contextMenu(btn);
    fireEvent.click(screen.getByRole("menuitem", { name: "Duplicate" }));
    await act(async () => {});
    expect(onSelect).toHaveBeenCalledOnce();
    expect(document.activeElement).toBe(btn);
  });

  // A dialog the action opens takes the focused element as its opener when it renders, which is
  // before the row it was on is gone: that has to be the element the user came from.
  it("is back where it was by the time the action runs", () => {
    let seen: Element | null = null;
    render(
      <>
        <Card onSelect={() => void (seen = document.activeElement)} />
        <ContextMenuHost />
      </>,
    );
    const btn = screen.getByRole("button", { name: "card" });
    btn.focus();
    fireEvent.contextMenu(btn);
    fireEvent.click(screen.getByRole("menuitem", { name: "Duplicate" }));
    expect(seen).toBe(btn);
  });

  it("goes back to where it was when the action left it on nothing", async () => {
    let drop: () => void = noop;
    render(
      <>
        <Card onSelect={() => drop()} />
        <input aria-label="elsewhere" />
        <ContextMenuHost />
      </>,
    );
    const btn = screen.getByRole("button", { name: "card" });
    const other = screen.getByRole("textbox", { name: "elsewhere" });
    btn.focus();
    fireEvent.contextMenu(btn);
    // The action focuses something and then blurs it again (a field that came and went).
    drop = () => {
      other.focus();
      other.blur();
    };
    fireEvent.click(screen.getByRole("menuitem", { name: "Duplicate" }));
    await act(async () => {});
    expect(document.activeElement).toBe(btn);
  });

  it("is not taken from a dialog or field the action focused", async () => {
    let other!: HTMLElement;
    render(
      <>
        <Card onSelect={() => other.focus()} />
        <input aria-label="dialog field" ref={(el) => void (other = el!)} />
        <ContextMenuHost />
      </>,
    );
    const btn = screen.getByRole("button", { name: "card" });
    btn.focus();
    fireEvent.contextMenu(btn);
    fireEvent.click(screen.getByRole("menuitem", { name: "Duplicate" }));
    await act(async () => {});
    expect(document.activeElement).toBe(other);
  });
});

describe("rows", () => {
  it("lists a disabled item, passes over it with the arrow keys, and never runs it", () => {
    const onSelect = vi.fn();
    render(<ContextMenuHost />);
    act(() =>
      openContextMenu(at, "Board", [
        { label: "Undo", disabled: true, onSelect },
        { label: "Paste", onSelect: noop },
      ]),
    );
    // The native attribute for now: the Pinboard mod's own test reads `.disabled` (see ROWS).
    const undo = screen.getByRole("menuitem", { name: "Undo" }) as HTMLButtonElement;
    expect(undo.disabled).toBe(true);
    const menu = screen.getByRole("menu");
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Paste" }));
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Paste" }));
    fireEvent.click(undo);
    expect(onSelect).not.toHaveBeenCalled();
    // Still open: a refused row is not a choice.
    expect(screen.getByRole("menu")).toBe(menu);
  });

  it("opens by key on the first item you can use", () => {
    render(
      <>
        <div
          onContextMenu={(e) =>
            openContextMenu(e, "Card", [
              { label: "Undo", disabled: true, onSelect: noop },
              { label: "Paste", onSelect: noop },
            ])
          }
        >
          <button>card</button>
        </div>
        <ContextMenuHost />
      </>,
    );
    const btn = screen.getByRole("button", { name: "card" });
    btn.focus();
    fireEvent.keyDown(btn, { key: "ContextMenu" });
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Paste" }));
  });

  it("draws an item with `checked` as a radio, ticked or not, and still runs it", () => {
    const onSelect = vi.fn();
    render(<ContextMenuHost />);
    act(() =>
      openContextMenu(at, "Task", [
        { label: "To do", checked: false, onSelect: noop },
        { label: "Done", checked: true, onSelect },
        { label: "Edit", onSelect: noop },
      ]),
    );
    expect(screen.getByRole("menuitemradio", { name: "To do" }).getAttribute("aria-checked")).toBe(
      "false",
    );
    const done = screen.getByRole("menuitemradio", { name: "Done" });
    expect(done.getAttribute("aria-checked")).toBe("true");
    screen.getByRole("menuitem", { name: "Edit" });
    fireEvent.click(done);
    expect(onSelect).toHaveBeenCalledOnce();
  });

  it("jumps to the next item starting with the letter typed, over the ones you can use", () => {
    render(<ContextMenuHost />);
    act(() =>
      openContextMenu(at, "Board", [
        { label: "Duplicate", onSelect: noop },
        { label: "Delete", disabled: true, onSelect: noop },
        { label: "Dismiss", onSelect: noop },
      ]),
    );
    const menu = screen.getByRole("menu");
    fireEvent.keyDown(menu, { key: "d" });
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Duplicate" }));
    fireEvent.keyDown(menu, { key: "d" });
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Dismiss" }));
    fireEvent.keyDown(menu, { key: "d" });
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Duplicate" }));
  });

  it("wraps a long label and never truncates it", () => {
    render(<ContextMenuHost />);
    act(() => openContextMenu(at, "Board", [{ label: "A very long label", onSelect: noop }]));
    const label = screen.getByText("A very long label");
    expect(label.className).toContain("break-words");
    expect(label.className).not.toContain("truncate");
  });

  it("spaces the colour dots 24px apart and keeps every row at least 24px tall", () => {
    render(<ContextMenuHost />);
    act(() =>
      openContextMenu(at, "Note", [
        { label: "Edit", onSelect: noop },
        {
          kind: "swatches",
          label: "Colour",
          options: [
            { name: "Sand", color: "a", active: true, onSelect: noop },
            { name: "Sky", color: "b", active: false, onSelect: noop },
          ],
        },
      ]),
    );
    expect(screen.getByRole("group", { name: "Colour" }).className).toContain("gap-2");
    expect(screen.getByRole("menuitem", { name: "Edit" }).className).toContain("min-h-6");
  });
});

describe("the fallback's Copy and Open on a surface's own menu", () => {
  function copiesTo(writeText: (t: string) => Promise<void>) {
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
  }
  const live = (c: HTMLElement) => c.querySelector("[aria-live]")!.textContent;

  it("adds Copy for selected text and says 'Copied' once it has", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    copiesTo(writeText);
    vi.spyOn(window, "getSelection").mockReturnValue({ toString: () => "some words" } as Selection);
    const { container } = render(
      <>
        <div
          onContextMenu={(e) =>
            openContextMenu(e, "Card", [{ label: "Duplicate", onSelect: noop }], {
              withFallback: true,
            })
          }
        >
          <p>some words</p>
        </div>
        <ContextMenuHost />
      </>,
    );
    fireEvent.contextMenu(screen.getByText("some words"));
    expect(screen.getAllByRole("menuitem").map((i) => i.textContent)).toEqual([
      "Duplicate",
      expect.stringMatching(/^Copy/),
    ]);
    expect(live(container)).toBe("");
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitem", { name: "Copy" }));
    });
    expect(writeText).toHaveBeenCalledWith("some words");
    expect(live(container)).toBe("Copied");
  });

  // A reader says nothing for text that did not change, so the second "Copied" must be a new node.
  it("announces the same words again on a second copy", async () => {
    copiesTo(() => Promise.resolve());
    const { container } = render(
      <>
        <a href="https://example.com/x">a link</a>
        <ContextMenuHost />
      </>,
    );
    const copy = async () => {
      fireEvent.contextMenu(screen.getByText("a link"));
      await act(async () => {
        fireEvent.click(screen.getByRole("menuitem", { name: "Copy link" }));
      });
      return container.querySelector("[aria-live] span");
    };
    const first = await copy();
    const second = await copy();
    expect(second?.textContent).toBe("Link copied");
    expect(second).not.toBe(first);
  });

  it("says 'Link copied' for a link, and says nothing where nothing was copied", async () => {
    const { container } = render(
      <>
        <a href="https://example.com/x">a link</a>
        <ContextMenuHost />
      </>,
    );
    // No clipboard at all: the click does nothing and nothing is announced.
    fireEvent.contextMenu(screen.getByText("a link"));
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitem", { name: "Copy link" }));
    });
    expect(live(container)).toBe("");
    copiesTo(() => Promise.resolve());
    fireEvent.contextMenu(screen.getByText("a link"));
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitem", { name: "Copy link" }));
    });
    expect(live(container)).toBe("Link copied");
  });

  it("copies for a surface's own item through the host, which says it aloud", async () => {
    // A menu's Copy item calls `copyViaHost`: it is spoken ("Link copied"), where a bare clipboard
    // write from the item would be silent, the menu having closed by then.
    const { container } = render(<ContextMenuHost />);
    copiesTo(() => Promise.resolve());
    await act(async () => copyViaHost("https://example.com/e", "Link copied"));
    expect(live(container)).toBe("Link copied");
    // Nothing is said where the write was refused.
    copiesTo(() => Promise.reject(new Error("no")));
    await act(async () => copyViaHost("x", "Copied twice"));
    expect(live(container)).toBe("Link copied");
  });
});
