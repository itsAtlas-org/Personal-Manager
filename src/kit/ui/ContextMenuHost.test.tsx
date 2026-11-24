// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The kit's own host, the one a mod's frame mounts (a frame is a page of its own, so it draws its own
// menu and answers its own right-clicks). It holds no way to open a link: the host is given one, and
// with none a link's menu offers Copy link alone. Core's host (components/ui/ContextMenuHost.test.tsx)
// is this one given `openLink`, and keeps its own tests.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { closeContextMenu, openContextMenu } from "../lib/contextMenu";
import { ContextMenuHost } from "./ContextMenuHost";

afterEach(() => {
  act(() => closeContextMenu());
  cleanup();
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
