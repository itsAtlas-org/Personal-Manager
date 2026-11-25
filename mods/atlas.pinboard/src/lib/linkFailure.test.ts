// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Opening a link from the Pinboard: the address goes to PM as it is, a refusal is shown in PM's own
// sentence (never the address), and the notice of one click is gone at the next.

import { afterEach, describe, expect, it } from "vitest";
import { act, cleanup, renderHook } from "@testing-library/react";
import { dismissLinkFailure, openLink, showLinkFailure, useLinkFailure } from "./linkFailure";
import { connectPm } from "./pm";
import { memoryPm, refuse } from "../testing";

afterEach(() => {
  dismissLinkFailure();
  connectPm(null);
});

afterEach(cleanup);

const notice = () => renderHook(() => useLinkFailure());

describe("openLink", () => {
  it("hands PM the address as it is, and shows nothing when PM opens it", async () => {
    const pm = await memoryPm({ answers: { "shell.openLink": { opened: true } } });
    const { result } = notice();
    await act(async () => {
      openLink("https://example.org/a?b=1");
      await pm.settle();
    });
    expect(pm.of("shell.openLink").map((c) => c.args)).toEqual([
      { url: "https://example.org/a?b=1" },
    ]);
    expect(result.current).toBeNull();
  });

  it("shows PM's own sentence when PM refuses, and the next click takes it down first", async () => {
    const pm = await memoryPm({
      answers: {
        "shell.openLink": refuse(
          "MOD-1003",
          "A mod may open only an https:// address to a website.",
        ),
      },
    });
    const { result } = notice();
    await act(async () => {
      openLink("http://example.org");
      await pm.settle();
    });
    expect(result.current).toBe("A mod may open only an https:// address to a website.");
    pm.answer("shell.openLink", { opened: true });
    act(() => openLink("https://example.org"));
    // Down at once, before the second answer: a notice left by an earlier link never sits beside this one.
    expect(result.current).toBeNull();
    await act(async () => void (await pm.settle()));
    expect(result.current).toBeNull();
  });

  it("a PM that is not there says so plainly", async () => {
    const { result } = notice();
    await act(async () => {
      openLink("https://example.org");
      await Promise.resolve();
    });
    expect(result.current).toMatch(/connected/);
  });

  it("showLinkFailure and dismissLinkFailure are the notice's two hands", () => {
    const { result } = notice();
    act(() => showLinkFailure("one"));
    expect(result.current).toBe("one");
    act(() => showLinkFailure("two"));
    expect(result.current).toBe("two");
    act(() => dismissLinkFailure());
    expect(result.current).toBeNull();
  });
});
