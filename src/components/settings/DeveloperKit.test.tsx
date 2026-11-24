// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Developer kit's rail sections (Mods spec §1.15): what is drawn is PM's own answer
// (`devKitInfo`), asked only while developer mode is on, kept in step on `mods://changed`, and
// every limit the backend sends has a label so a new one can't ship unnamed.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { DevKitInfo, ModsChanged } from "../../lib/modsTypes";
import { devKit, endpoint } from "../mods/modFixtures";

const devKitInfo = vi.fn<() => Promise<DevKitInfo>>();
const changedHandlers = new Set<(e: ModsChanged) => void>();
const changed = (e: ModsChanged) => changedHandlers.forEach((h) => h(e));

vi.mock("../../lib/ipc", () => ({
  devKitInfo: () => devKitInfo(),
  onModsChanged: (h: (e: ModsChanged) => void) => {
    changedHandlers.add(h);
    return Promise.resolve(() => changedHandlers.delete(h));
  },
}));
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { HELP } from "../../lib/help";
import { DeveloperKit } from "./DeveloperKit";
import { CLASSIC_SCRIPT_NOTE, LIMIT_KEYS, LIMIT_ROWS, OFF_HINT } from "./kitWords";
import { SETTINGS_TABS } from "./registry";

const section = (id: string) => within(document.getElementById(id)!);

beforeEach(() => {
  devKitInfo.mockReset();
  changedHandlers.clear();
});
afterEach(cleanup);

describe("while developer mode is off", () => {
  it("asks for nothing and says so in every section, so the rail's links still land", () => {
    render(<DeveloperKit on={false} />);
    expect(devKitInfo).not.toHaveBeenCalled();
    for (const id of ["sec-dev-api", "sec-dev-endpoints", "sec-dev-limits", "sec-dev-running"]) {
      expect(document.getElementById(id)?.hasAttribute("data-settings-section")).toBe(true);
      expect(section(id).getByText(OFF_HINT)).toBeTruthy();
    }
    expect(changedHandlers.size).toBe(0);
  });
});

describe("the API version", () => {
  it("shows the version, the backend's deprecation window and the experimental methods", async () => {
    devKitInfo.mockResolvedValue(
      devKit({
        api: {
          version: "1.4",
          oldestMajor: 1,
          window: "PM serves mod API 1.4 and, for older majors, 1 up to 1.",
          experimental: ["ai.complete", "net.fetch"],
        },
      }),
    );
    render(<DeveloperKit on />);
    const api = section("sec-dev-api");
    expect(await api.findByText("1.4")).toBeTruthy();
    expect(api.getByText(/PM serves mod API 1.4 and, for older majors/)).toBeTruthy();
    expect(api.getByText("ai.complete")).toBeTruthy();
    expect(api.getByText("net.fetch")).toBeTruthy();
  });

  it("tells a mod's developer that only classic scripts load, even with developer mode off", () => {
    render(<DeveloperKit on={false} />);
    expect(section("sec-dev-api").getByText(CLASSIC_SCRIPT_NOTE)).toBeTruthy();
    expect(CLASSIC_SCRIPT_NOTE).toMatch(/module script/);
    expect(CLASSIC_SCRIPT_NOTE).toMatch(/classic/);
  });

  it("says plainly when nothing is experimental", async () => {
    devKitInfo.mockResolvedValue(devKit({ api: { ...devKit().api, experimental: [] } }));
    render(<DeveloperKit on />);
    expect(await section("sec-dev-api").findByText("None.")).toBeTruthy();
  });
});

describe("the endpoints", () => {
  it("lists every row of the table the backend sent, calls and events apart, nothing added", async () => {
    devKitInfo.mockResolvedValue(
      devKit({
        endpoints: [
          endpoint({ name: "documents.list" }),
          endpoint({ name: "theme.get", permission: null, class: "local" }),
          endpoint({ name: "ai.complete", experimental: true, permission: "ai" }),
          endpoint({ name: "old.call", added: "1.0", deprecated: "1.2", removed: "2.0" }),
          endpoint({ name: "documents.added", kind: "event" }),
        ],
      }),
    );
    render(<DeveloperKit on />);
    const endpoints = section("sec-dev-endpoints");
    fireEvent.click(await endpoints.findByRole("button", { name: /Calls a mod makes \(4\)/ }));
    fireEvent.click(endpoints.getByRole("button", { name: /Events PM sends \(1\)/ }));

    const calls = endpoints.getByRole("table", { name: "Calls a mod makes" });
    const names = within(calls)
      .getAllByRole("row")
      .slice(1)
      .map((r) => r.querySelector("td")?.textContent);
    expect(names).toEqual(["documents.list", "theme.get", "ai.complete", "old.call"]);
    // A call any session may make says so, rather than showing an empty cell.
    expect(within(calls).getByText("any session")).toBeTruthy();
    expect(within(calls).getByText("since 1.0, experimental")).toBeTruthy();
    expect(within(calls).getByText("since 1.0, deprecated in 1.2, removed in 2.0")).toBeTruthy();
    const events = endpoints.getByRole("table", { name: "Events PM sends" });
    expect(within(events).getByText("documents.added")).toBeTruthy();
  });

  it("puts each table in a named scroll region the keyboard can reach, so a hidden column isn't lost at a large text size", async () => {
    devKitInfo.mockResolvedValue(devKit({ endpoints: [endpoint(), endpoint({ kind: "event" })] }));
    render(<DeveloperKit on />);
    const endpoints = section("sec-dev-endpoints");
    fireEvent.click(await endpoints.findByRole("button", { name: /Calls a mod makes/ }));
    const region = endpoints.getByRole("region", { name: "Calls a mod makes, scrollable" });
    // A scroll container with nothing focusable inside can't be scrolled from the keyboard unless it
    // is itself a tab stop.
    expect(region.getAttribute("tabindex")).toBe("0");
    expect(region.className).toContain("overflow-x-auto");
    expect(within(region).getByRole("table", { name: "Calls a mod makes" })).toBeTruthy();
  });

  it("starts folded, so a long table doesn't push the rest away", async () => {
    devKitInfo.mockResolvedValue(devKit());
    render(<DeveloperKit on />);
    const button = await section("sec-dev-endpoints").findByRole("button", {
      name: /Calls a mod makes/,
    });
    expect(button.getAttribute("aria-expanded")).toBe("false");
  });
});

describe("the limits", () => {
  it("shows every limit the backend sends, with a label and its unit", async () => {
    devKitInfo.mockResolvedValue(devKit());
    render(<DeveloperKit on />);
    const limits = section("sec-dev-limits");
    expect(await limits.findByText("Calls per second, sustained")).toBeTruthy();
    expect(limits.getAllByText("50")).toHaveLength(2); // calls per second, additions after a quiet spell
    expect(limits.getByText("1 MB")).toBeTruthy(); // largest call
    expect(limits.getByText("8 MB")).toBeTruthy(); // largest answer
    expect(limits.getByText("50 MB")).toBeTruthy(); // a mod's own storage
    expect(limits.getAllByText("64 KB")).toHaveLength(2); // largest event, largest AI prompt
    expect(limits.getByText("250 ms")).toBeTruthy();
    expect(limits.getByText("100000")).toBeTruthy();
  });

  it("says the experimental calls' limits, times in seconds", async () => {
    devKitInfo.mockResolvedValue(devKit());
    render(<DeveloperKit on />);
    const limits = section("sec-dev-limits");
    await limits.findByText("Calls per second, sustained");
    const said = (label: string) => limits.getByText(label).parentElement?.textContent ?? "";
    expect(said("Longest a call may wait before it is stopped")).toContain("180 s");
    expect(said("Longest a whole web request may take")).toContain("30 s");
    expect(said("Longest wait to connect to a website")).toContain("10 s");
    expect(said("Largest web answer")).toContain("4 MB");
    expect(said("Largest web request body")).toContain("256 KB");
    expect(said("Notices per mod per day")).toContain("5");
  });

  it("says PR C's limits: the lists, the document saves, the links, the quota and the question's minute", async () => {
    devKitInfo.mockResolvedValue(devKit());
    render(<DeveloperKit on />);
    const limits = section("sec-dev-limits");
    await limits.findByText("Calls per second, sustained");
    const said = (label: string) => limits.getByText(label).parentElement?.textContent ?? "";
    expect(said("Additions to your lists per second, sustained")).toContain("10");
    expect(said("Additions to your lists running at the same time")).toContain("2");
    expect(said("Tasks and milestones one mod may ever make")).toContain("10000");
    expect(said("Document saves per minute, sustained")).toContain("6");
    expect(said("Document saves at once after a quiet spell")).toContain("10");
    expect(said("Links opened per minute, sustained")).toContain("6");
    expect(said("Links opened at once after a quiet spell")).toContain("3");
    expect(said("How long PM's question to you stays open")).toContain("60 s");
  });

  it("has a label for exactly the limits the backend sends (a new one can't ship unnamed)", () => {
    expect([...LIMIT_KEYS].sort()).toEqual(Object.keys(devKit().limits).sort());
    for (const key of LIMIT_KEYS) expect(LIMIT_ROWS[key].label.trim()).not.toBe("");
  });
});

describe("what is running now", () => {
  it("says the engine is sealed and that no frame is open", async () => {
    devKitInfo.mockResolvedValue(devKit());
    render(<DeveloperKit on />);
    const running = section("sec-dev-running");
    expect(await running.findByText(/The web engine is sealed/)).toBeTruthy();
    expect(running.getByText("No mod frames are open.")).toBeTruthy();
  });

  it("gives the backend's reasons when the engine is not sealed", async () => {
    devKitInfo.mockResolvedValue(
      devKit({
        engine: {
          sealed: false,
          unsealedReasons: ["The sinkhole could not be started."],
          sinkholeAttempts: 3,
        },
      }),
    );
    render(<DeveloperKit on />);
    const running = section("sec-dev-running");
    expect(await running.findByText(/The web engine is not sealed/)).toBeTruthy();
    expect(running.getByText("The sinkhole could not be started.")).toBeTruthy();
    expect(running.getByText("3")).toBeTruthy();
  });

  it("lists each open frame with its event counters, and marks one that ended", async () => {
    devKitInfo.mockResolvedValue(
      devKit({
        sessions: [
          { modId: "acme.map", sent: 12, acked: 10, dropped: 2, closed: false },
          { modId: "acme.map", sent: 3, acked: 3, dropped: 0, closed: true },
        ],
      }),
    );
    render(<DeveloperKit on />);
    const list = await section("sec-dev-running").findByRole("list", { name: "Open mod frames" });
    const items = within(list).getAllByRole("listitem");
    expect(items).toHaveLength(2);
    expect(items[0].textContent).toContain("events sent 12, acknowledged 10, dropped 2");
    expect(items[0].textContent).not.toContain("ended");
    expect(items[1].textContent).toContain("ended");
  });

  it("reads again when Refresh is pressed", async () => {
    devKitInfo.mockResolvedValueOnce(devKit());
    render(<DeveloperKit on />);
    await section("sec-dev-running").findByText("No mod frames are open.");
    devKitInfo.mockResolvedValueOnce(
      devKit({ sessions: [{ modId: "acme.map", sent: 1, acked: 1, dropped: 0, closed: false }] }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(
      await section("sec-dev-running").findByRole("list", { name: "Open mod frames" }),
    ).toBeTruthy();
    expect(devKitInfo).toHaveBeenCalledTimes(2);
  });
});

describe("keeping in step", () => {
  it("reads again on every mods://changed, whatever its cause", async () => {
    devKitInfo.mockResolvedValue(devKit());
    render(<DeveloperKit on />);
    await section("sec-dev-api").findByText("1.0");
    await waitFor(() => expect(changedHandlers.size).toBe(1));
    act(() => changed({ cause: "paused" }));
    await waitFor(() => expect(devKitInfo).toHaveBeenCalledTimes(2));
  });

  it("stops listening when developer mode goes off and shows the hint again", async () => {
    devKitInfo.mockResolvedValue(devKit());
    const { rerender } = render(<DeveloperKit on />);
    await section("sec-dev-api").findByText("1.0");
    rerender(<DeveloperKit on={false} />);
    await waitFor(() => expect(changedHandlers.size).toBe(0));
    expect(section("sec-dev-api").getByText(OFF_HINT)).toBeTruthy();
    expect(section("sec-dev-api").queryByText("1.0")).toBeNull();
  });

  it("starts from a fresh read, not what an earlier time left, when it goes on again", async () => {
    devKitInfo.mockResolvedValueOnce(devKit({ api: { ...devKit().api, version: "1.1" } }));
    const { rerender } = render(<DeveloperKit on />);
    await section("sec-dev-api").findByText("1.1");
    rerender(<DeveloperKit on={false} />);
    devKitInfo.mockReturnValueOnce(new Promise(() => {}));
    rerender(<DeveloperKit on />);
    expect(section("sec-dev-api").queryByText("1.1")).toBeNull();
    expect(section("sec-dev-api").getByText("Reading…")).toBeTruthy();
  });

  it("drops an answer to an older question when a newer one has been asked", async () => {
    let first!: (k: DevKitInfo) => void;
    devKitInfo.mockReturnValueOnce(new Promise<DevKitInfo>((r) => (first = r)));
    render(<DeveloperKit on />);
    await waitFor(() => expect(changedHandlers.size).toBe(1));
    devKitInfo.mockResolvedValueOnce(devKit({ api: { ...devKit().api, version: "1.9" } }));
    act(() => changed({ cause: "loaded" }));
    await section("sec-dev-api").findByText("1.9");
    await act(async () => first(devKit({ api: { ...devKit().api, version: "1.0" } })));
    expect(section("sec-dev-api").getByText("1.9")).toBeTruthy();
    expect(section("sec-dev-api").queryByText("1.0")).toBeNull();
  });

  it("says why a read failed, in the backend's words, and keeps what it had", async () => {
    devKitInfo.mockResolvedValueOnce(devKit());
    render(<DeveloperKit on />);
    await section("sec-dev-api").findByText("1.0");
    await waitFor(() => expect(changedHandlers.size).toBe(1));
    devKitInfo.mockRejectedValueOnce({ code: "locked", message: "PM is locked." });
    act(() => changed({ cause: "paused" }));
    expect(await section("sec-dev-running").findByText("PM is locked.")).toBeTruthy();
    expect(section("sec-dev-api").getByText("1.0")).toBeTruthy();
  });

  it("shows the failure in place of the readout when there is nothing to keep", async () => {
    devKitInfo.mockRejectedValue({ code: "dev-mode-off", message: "Developer mode is off." });
    render(<DeveloperKit on />);
    expect(await section("sec-dev-api").findByText("Developer mode is off.")).toBeTruthy();
  });
});

describe("help", () => {
  it("every section has a help entry, and so does every rail anchor of the tab", async () => {
    devKitInfo.mockResolvedValue(devKit());
    render(<DeveloperKit on />);
    await section("sec-dev-api").findByText("1.0");
    for (const el of document.querySelectorAll("[data-help]")) {
      const id = el.getAttribute("data-help")!;
      expect(HELP[id], `no help entry for ${id}`).toBeDefined();
    }
    const dev = SETTINGS_TABS.find((t) => t.id === "developer")!;
    expect(dev.sections.map((s) => s.id)).toEqual([
      "sec-dev-mode",
      "sec-dev-api",
      "sec-dev-endpoints",
      "sec-dev-limits",
      "sec-dev-running",
      "sec-dev-testmods",
      "sec-dev-testkeys",
    ]);
  });
});
