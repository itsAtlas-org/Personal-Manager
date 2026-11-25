// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// A mod's tab outlives the view change that leaves it, just long enough for its frame to save (K5): it
// stays mounted, hidden, until PM's suspend wait is over, then goes; and a return before that keeps it.

import { act, cleanup, render, screen } from "@testing-library/react";
import { useEffect } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ModTabHost } from "./ModTabHost";

const suspendModFrames = vi.fn();
vi.mock("../../lib/ipc", () => ({
  suspendModFrames: (reason: string, modId?: string) => suspendModFrames(reason, modId),
}));

const lives: string[] = [];
vi.mock("./ModTab", () => ({
  ModTab: ({ modId }: { modId: string }) => {
    useEffect(() => {
      lives.push(`mount:${modId}`);
      return () => {
        lives.push(`unmount:${modId}`);
      };
    }, [modId]);
    return <p>tab of {modId}</p>;
  },
}));

/** A suspend the test finishes itself. */
function held() {
  let done: () => void = () => {};
  suspendModFrames.mockImplementationOnce(
    () =>
      new Promise<void>((resolve) => {
        done = resolve;
      }),
  );
  return () => done();
}

const noop = () => {};
const host = (view: string) => (
  <ModTabHost view={view as never} onOpenMods={noop} onOpenSettings={noop} />
);
const tabOf = (id: string) =>
  document.querySelector(`[data-mod-tab="${id}"]`) as HTMLElement | null;

beforeEach(() => {
  lives.length = 0;
  suspendModFrames.mockReset();
  suspendModFrames.mockResolvedValue(undefined);
});
afterEach(cleanup);

describe("ModTabHost", () => {
  it("draws the mod's tab when the view is on it, and nothing for any other view", () => {
    const { rerender } = render(host("focus"));
    expect(document.querySelector("[data-mod-tab]")).toBeNull();
    rerender(host("mod:acme.tool"));
    expect(screen.getByText("tab of acme.tool")).toBeTruthy();
    expect(tabOf("acme.tool")!.style.display).not.toBe("none");
  });

  it("keeps the tab mounted and hidden after leaving it, until the frame has answered, then lets it go", async () => {
    const finish = held();
    const { rerender } = render(host("mod:acme.tool"));
    rerender(host("focus"));
    // Told (reason hidden, this mod's frames), and still there, off the screen.
    expect(suspendModFrames).toHaveBeenCalledWith("hidden", "acme.tool");
    expect(tabOf("acme.tool")).not.toBeNull();
    expect(tabOf("acme.tool")!.style.display).toBe("none");
    expect(lives).toEqual(["mount:acme.tool"]);
    await act(async () => {
      finish();
    });
    expect(tabOf("acme.tool")).toBeNull();
    expect(lives).toEqual(["mount:acme.tool", "unmount:acme.tool"]);
  });

  it("still goes when the wait ended without an answer (the wait is the deadline's, not the frame's)", async () => {
    // `suspendModFrames` resolves at the deadline whatever the backend does; the host only follows it.
    suspendModFrames.mockResolvedValue(undefined);
    const { rerender } = render(host("mod:acme.tool"));
    rerender(host("documents"));
    await act(async () => {});
    expect(tabOf("acme.tool")).toBeNull();
  });

  it("keeps the same frame when the person comes back before the wait is over", async () => {
    const finish = held();
    const { rerender } = render(host("mod:acme.tool"));
    rerender(host("focus"));
    rerender(host("mod:acme.tool"));
    await act(async () => {
      finish();
    });
    // Not unmounted by the wait that was overtaken, and shown again.
    expect(tabOf("acme.tool")).not.toBeNull();
    expect(tabOf("acme.tool")!.style.display).not.toBe("none");
    expect(lives).toEqual(["mount:acme.tool"]);
  });

  it("a switch from one mod to another retires the first and shows the second", async () => {
    const finish = held();
    const { rerender } = render(host("mod:acme.one"));
    rerender(host("mod:acme.two"));
    expect(suspendModFrames).toHaveBeenCalledWith("hidden", "acme.one");
    expect(suspendModFrames).not.toHaveBeenCalledWith("hidden", "acme.two");
    expect(tabOf("acme.one")!.style.display).toBe("none");
    expect(tabOf("acme.two")!.style.display).not.toBe("none");
    await act(async () => {
      finish();
    });
    expect(tabOf("acme.one")).toBeNull();
    expect(tabOf("acme.two")).not.toBeNull();
  });

  it("opens a fresh tab, and so a fresh frame, after the old one went", async () => {
    const { rerender } = render(host("mod:acme.tool"));
    rerender(host("focus"));
    await act(async () => {});
    rerender(host("mod:acme.tool"));
    expect(lives).toEqual(["mount:acme.tool", "unmount:acme.tool", "mount:acme.tool"]);
  });
});
