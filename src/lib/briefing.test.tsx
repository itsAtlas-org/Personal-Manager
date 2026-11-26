// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The briefing provider's failure handling. A Refresh the user clicks that fails says why and keeps
// the briefing it had; the automatic runs (launch check, hourly, nudges) and the refresh after a
// flag is resolved fail silently, because the briefing is optional and nobody asked for those.

import type { ReactNode } from "react";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DailyBriefing } from "./types";

const ipc = vi.hoisted(() => ({
  getDailyBriefing: vi.fn(),
  refreshDailyBriefing: vi.fn(),
  syncDailyBriefing: vi.fn(),
  onBriefingUpdated: vi.fn(),
}));
vi.mock("./ipc", () => ipc);

import { BriefingProvider, useBriefing } from "./briefing";

const row = (text: string, updated_at = "2026-11-20T09:00:00Z"): DailyBriefing =>
  ({ briefing: text, updated_at, stale: false }) as DailyBriefing;

const OLD = row("Old briefing.");
const NEW = row("New briefing.", "2026-11-20T10:30:00Z");
const NO_PROVIDER =
  "No AI provider is set up yet — add an OpenRouter key, or set up a local model.";

let announce: () => void;

beforeEach(() => {
  vi.resetAllMocks();
  ipc.getDailyBriefing.mockResolvedValue(OLD);
  ipc.syncDailyBriefing.mockResolvedValue(OLD);
  ipc.onBriefingUpdated.mockImplementation((cb: () => void) => {
    announce = cb;
    return Promise.resolve(() => {});
  });
});
afterEach(cleanup);

function mount(autoRefresh = true) {
  const wrapper = ({ children }: { children: ReactNode }) => (
    <BriefingProvider autoRefresh={autoRefresh}>{children}</BriefingProvider>
  );
  return renderHook(() => useBriefing(), { wrapper });
}

async function loaded(autoRefresh = true) {
  const hook = mount(autoRefresh);
  await waitFor(() => expect(hook.result.current.briefing).toEqual(OLD));
  // Let the launch check finish, so a test starts from rest.
  await waitFor(() => expect(hook.result.current.busy).toBe(false));
  return hook;
}

describe("the Refresh button's call", () => {
  it("shows the backend's message, keeps the old briefing and frees the button", async () => {
    const { result } = await loaded();
    ipc.refreshDailyBriefing.mockRejectedValueOnce(NO_PROVIDER);

    let returned: string | null = null;
    await act(async () => {
      returned = await result.current.requestRefresh();
    });

    expect(returned).toBe(NO_PROVIDER);
    expect(result.current.error).toBe(NO_PROVIDER);
    expect(result.current.briefing).toEqual(OLD);
    expect(result.current.busy).toBe(false);
  });

  it("says something even when the rejection carries no words", async () => {
    const { result } = await loaded();
    ipc.refreshDailyBriefing.mockRejectedValueOnce(undefined);
    await act(async () => void (await result.current.requestRefresh()));
    expect(result.current.error).toBeTruthy();
  });

  it("clears the message when the next click starts, and on success", async () => {
    const { result } = await loaded();
    ipc.refreshDailyBriefing.mockRejectedValueOnce(NO_PROVIDER);
    await act(async () => void (await result.current.requestRefresh()));
    expect(result.current.error).toBe(NO_PROVIDER);

    let release: (b: DailyBriefing) => void = () => {};
    ipc.refreshDailyBriefing.mockReturnValueOnce(new Promise<DailyBriefing>((r) => (release = r)));
    let pending: Promise<string | null> = Promise.resolve(null);
    act(() => {
      pending = result.current.requestRefresh();
    });
    // The old message goes as soon as the user tries again, not when the answer comes back.
    expect(result.current.error).toBeNull();
    expect(result.current.busy).toBe(true);

    await act(async () => {
      release(NEW);
      expect(await pending).toBeNull();
    });
    expect(result.current.error).toBeNull();
    expect(result.current.briefing).toEqual(NEW);
    expect(result.current.busy).toBe(false);
  });

  it("clears the message when a fresh briefing arrives from elsewhere", async () => {
    const { result } = await loaded();
    ipc.refreshDailyBriefing.mockRejectedValueOnce(NO_PROVIDER);
    await act(async () => void (await result.current.requestRefresh()));
    expect(result.current.error).toBe(NO_PROVIDER);

    // The hourly check (or the other window's Refresh) wrote a new briefing.
    ipc.getDailyBriefing.mockResolvedValue(NEW);
    await act(async () => announce());
    await waitFor(() => expect(result.current.briefing).toEqual(NEW));
    expect(result.current.error).toBeNull();
  });
});

describe("the runs nobody clicked", () => {
  it("stay silent when the forced refresh after a flag is resolved fails", async () => {
    const { result } = await loaded();
    ipc.refreshDailyBriefing.mockRejectedValueOnce(NO_PROVIDER);
    await act(async () => void (await result.current.refresh()));
    expect(result.current.error).toBeNull();
    expect(result.current.briefing).toEqual(OLD);
    expect(result.current.busy).toBe(false);
  });

  it("stay silent when the launch check fails", async () => {
    ipc.syncDailyBriefing.mockRejectedValue("the model call failed");
    const { result } = mount();
    await waitFor(() => expect(ipc.syncDailyBriefing).toHaveBeenCalledOnce());
    await waitFor(() => expect(result.current.busy).toBe(false));
    expect(result.current.error).toBeNull();
    expect(result.current.briefing).toEqual(OLD);
  });

  it("do not clear a message the user is still looking at, when they start and fail", async () => {
    const { result } = await loaded();
    ipc.refreshDailyBriefing.mockRejectedValueOnce(NO_PROVIDER);
    await act(async () => void (await result.current.requestRefresh()));
    ipc.refreshDailyBriefing.mockRejectedValueOnce("a different failure");
    await act(async () => void (await result.current.refresh()));
    expect(result.current.error).toBe(NO_PROVIDER);
  });
});
