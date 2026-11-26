// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The one archive path: archiving or restoring writes, then regenerates the briefing, whichever
// surface asked (the Focus card, the project page, the sidebar entry's menu). The project page
// used to write alone and leave the briefing naming an archived project.

import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const refresh = vi.hoisted(() => vi.fn(async () => {}));
vi.mock("./briefing", () => ({ useBriefing: () => ({ briefing: null, busy: false, refresh }) }));
vi.mock("./ipc", () => ({ setProjectArchived: vi.fn(async () => {}) }));

import { useArchiveProject } from "./archiveProject";
import { setProjectArchived } from "./ipc";

beforeEach(() => vi.clearAllMocks());

describe("useArchiveProject", () => {
  it("archives, then regenerates the briefing and runs the caller's own re-read", async () => {
    const alongside = vi.fn(async () => {});
    const { result } = renderHook(() => useArchiveProject());
    await result.current("Atlas", true, alongside);
    expect(setProjectArchived).toHaveBeenCalledWith("Atlas", true);
    expect(refresh).toHaveBeenCalledOnce();
    expect(alongside).toHaveBeenCalledOnce();
  });

  it("regenerates on an unarchive too", async () => {
    const { result } = renderHook(() => useArchiveProject());
    await result.current("Atlas", false);
    expect(setProjectArchived).toHaveBeenCalledWith("Atlas", false);
    expect(refresh).toHaveBeenCalledOnce();
  });

  it("rejects with the write's error and leaves the briefing alone", async () => {
    vi.mocked(setProjectArchived).mockRejectedValueOnce(new Error("locked"));
    const { result } = renderHook(() => useArchiveProject());
    await expect(result.current("Atlas", true)).rejects.toThrow("locked");
    expect(refresh).not.toHaveBeenCalled();
  });
});
