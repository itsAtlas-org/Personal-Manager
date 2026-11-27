// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The timeline and meeting wrappers send the argument names the Rust commands take. The iCal id goes
// as `eventIcal` (the Rust parameter is `event_ical`, so no identifier there reads as a private id).

import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invokeMock(cmd, args),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const ipc = await import("./ipc");

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe("the timeline wrappers", () => {
  it("page with the cursor the last page gave, and default the limit to the backend's", async () => {
    await ipc.listTimeline(4);
    await ipc.listTimeline(4, { on_date: "2026-05-09", at: "", id: 9 }, 50);
    expect(invokeMock).toHaveBeenNthCalledWith(1, "list_timeline", {
      personId: 4,
      before: null,
      limit: null,
    });
    expect(invokeMock).toHaveBeenNthCalledWith(2, "list_timeline", {
      personId: 4,
      before: { on_date: "2026-05-09", at: "", id: 9 },
      limit: 50,
    });
  });

  it("send a draft whole to add and update", async () => {
    const draft = {
      kind: "call" as const,
      on_date: "2026-11-02",
      at: null,
      note: "",
      people: [1, 2],
    };
    await ipc.addInteraction(draft);
    await ipc.updateInteraction(7, draft);
    await ipc.deleteInteraction(7);
    expect(invokeMock.mock.calls).toEqual([
      ["add_interaction", { draft }],
      ["update_interaction", { id: 7, draft }],
      ["delete_interaction", { id: 7 }],
    ]);
  });
});

describe("the meeting wrappers", () => {
  it("ask for what is owed with no arguments", async () => {
    await ipc.owedMeetings();
    expect(invokeMock).toHaveBeenCalledWith("owed_meetings", undefined);
  });

  it("answer with the iCal id as eventIcal and optional words as null", async () => {
    await ipc.answerMeeting(
      "evt-1@example.com",
      "2026-11-26T08:00:00Z",
      [{ id: 3 }, { new: { name: "Newt Nobody", email: "newt@example.com" } }],
      "logged",
      "Went well",
    );
    expect(invokeMock).toHaveBeenCalledWith("answer_meeting", {
      eventIcal: "evt-1@example.com",
      start: "2026-11-26T08:00:00Z",
      people: [{ id: 3 }, { new: { name: "Newt Nobody", email: "newt@example.com" } }],
      answer: "logged",
      note: "Went well",
      title: null,
    });
    await ipc.answerMeeting("evt-2@example.com", "2026-11-26", [], "series_skipped");
    expect(invokeMock).toHaveBeenLastCalledWith("answer_meeting", {
      eventIcal: "evt-2@example.com",
      start: "2026-11-26",
      people: [],
      answer: "series_skipped",
      note: null,
      title: null,
    });
  });
});

describe("the card's other wrappers", () => {
  it("send an event's guests as they came, for the backend to sort", async () => {
    const guests = [
      {
        name: "Ann A.",
        email: "ann@example.com",
        response: "accepted",
        optional: false,
        organizer: false,
        self: false,
      },
    ];
    await ipc.eventGuests(guests);
    expect(invokeMock).toHaveBeenCalledWith("event_guests", { guests });
  });

  it("carry a person to the main window beside the view, and null where there is none", async () => {
    await ipc.showInMainWindow({ view: "people", person: "0123456789abcdef0123456789abcdef" });
    expect(invokeMock).toHaveBeenLastCalledWith("show_in_main_window", {
      view: "people",
      project: null,
      person: "0123456789abcdef0123456789abcdef",
    });
    await ipc.showInMainWindow({ view: "project", project: "Atlas" });
    expect(invokeMock).toHaveBeenLastCalledWith("show_in_main_window", {
      view: "project",
      project: "Atlas",
      person: null,
    });
  });
});
