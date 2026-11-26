// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A drag in a mod's frame (Mods plan K7, pointer capture). Listeners on `window` stop at the frame's
// edge: a drag that leaves the iframe would lose its moves and its release and hang until the pointer
// came back. Capturing the pointer on the grabbed element keeps the events coming.

import { act, renderHook } from "@testing-library/react";
import type { PointerEvent as ReactPointerEvent } from "react";
import { describe, expect, it, vi } from "vitest";
import { CELL } from "./grid";
import { useBoardDrag } from "./useBoardDrag";
import type { Widget } from "./types";

const widget: Widget = { id: "a", kind: "note", rect: { x: 2, y: 2, w: 7, h: 5 } };

function setup() {
  const onMoveEnd = vi.fn();
  const hook = renderHook(() =>
    useBoardDrag({
      boundsRef: { current: { cols: 40, rows: 30 } },
      surfaceRef: { current: null },
      onMoveEnd,
      onResizeEnd: vi.fn(),
    }),
  );
  return { ...hook, onMoveEnd };
}

const grab = (capture: ((id: number) => void) | undefined, pointerId = 7) =>
  ({
    preventDefault: () => {},
    clientX: 100,
    clientY: 100,
    pointerId,
    currentTarget: { setPointerCapture: capture },
  }) as unknown as ReactPointerEvent;

describe("a drag captures the pointer", () => {
  it("on the element that was grabbed, by the pointer that grabbed it", () => {
    const capture = vi.fn();
    const { result } = setup();
    act(() => result.current.startDrag(grab(capture, 42), widget, "move"));
    expect(capture).toHaveBeenCalledTimes(1);
    expect(capture).toHaveBeenCalledWith(42);
  });

  it("a pointer the browser does not know is not an error: the drag still runs and ends inside the frame", () => {
    const { result, onMoveEnd } = setup();
    const refuse = () => {
      throw new DOMException("No such pointer.", "NotFoundError");
    };
    act(() => result.current.startDrag(grab(refuse), widget, "move"));
    expect(result.current.draggingId).toBe("a");
    act(
      () => void window.dispatchEvent(new MouseEvent("pointerup", { clientX: 140, clientY: 100 })),
    );
    expect(onMoveEnd).toHaveBeenCalledTimes(1);
    expect(result.current.draggingId).toBeNull();
  });

  it("an element with no capture method (an old engine) still drags", () => {
    const { result } = setup();
    act(() => result.current.startDrag(grab(undefined), widget, "move"));
    expect(result.current.draggingId).toBe("a");
  });
});

describe("the drop-target ring agrees with the drop", () => {
  const surface = { getBoundingClientRect: () => ({ left: 0, top: 0 }) } as unknown as HTMLElement;
  function aim() {
    const onPointerCell = vi.fn();
    const hook = renderHook(() =>
      useBoardDrag({
        boundsRef: { current: { cols: 40, rows: 30 } },
        surfaceRef: { current: surface },
        onMoveEnd: vi.fn(),
        onResizeEnd: vi.fn(),
        onPointerCell,
      }),
    );
    return { ...hook, onPointerCell };
  }
  const moveTo = (x: number, y: number) =>
    act(() => void window.dispatchEvent(new MouseEvent("pointermove", { clientX: x, clientY: y })));

  it("a card still at its own rect aims at no folder, however the pointer wobbles; a moved card aims at the cell under the pointer", () => {
    const { result, onPointerCell } = aim();
    act(() => result.current.startDrag(grab(vi.fn()), widget, "move"));
    moveTo(101, 102);
    expect(onPointerCell).toHaveBeenLastCalledWith("a", null);
    moveTo(100 + CELL * 6, 100);
    const calls = onPointerCell.mock.calls;
    expect(calls[calls.length - 1][1]).not.toBeNull();
    // and back to where it started: no ring again
    moveTo(100, 100);
    expect(onPointerCell).toHaveBeenLastCalledWith("a", null);
  });
});
