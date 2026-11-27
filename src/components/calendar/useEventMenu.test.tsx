// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The right-click menu of a calendar event: one delegated handler finds the event by the id on the
// element (and leaves the ghost of an event being made alone), the items are the ones the details
// popover offers and are hidden where the popover hides them, and each calls the handler the
// popover's own button calls. Delete… opens the question; the menu never deletes.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Calendar, CalendarEvent, Milestone, Task } from "../../lib/types";
import {
  MILESTONE_CALENDAR_ID,
  PINBOARD_CALENDAR_ID,
  TASK_CALENDAR_ID,
} from "../../kit/lib/calendar-layout";

const ipc = vi.hoisted(() => ({
  openUrl: vi.fn(async () => {}),
  changeTaskStatus: vi.fn(async () => {}),
}));
vi.mock("../../lib/ipc", () => ipc);

import {
  closeContextMenu,
  currentContextMenu,
  setFallbackActions,
} from "../../kit/lib/contextMenu";
import { ContextMenuHost } from "../../kit/ui/ContextMenuHost";
import type { PopoverEditing } from "./parts/CalendarEventPopover";
import { eventMenu, useEventMenu, type EventMenuDeps } from "./useEventMenu";

const ev = (over: Partial<CalendarEvent> = {}): CalendarEvent => ({
  id: "e1",
  calendar_id: "cal",
  summary: "Standup",
  description: null,
  location: null,
  start: "2026-11-25T09:00:00",
  end: "2026-11-25T09:30:00",
  all_day: false,
  html_link: null,
  uid: "u1",
  ...over,
});

const calendar = { id: "cal", provider: "google" } as unknown as Calendar;
const milestone = {
  id: 4,
  project_name: "Garden",
  label: "Plant bulbs",
  event_uid: "u1",
} as unknown as Milestone;
const task = (over: Partial<Task> = {}) =>
  ({ id: 9, title: "Write the brief", status: "todo", project_name: "Atlas", ...over }) as Task;

const rect = new DOMRect(1, 2, 3, 4);

function editing(over: Partial<PopoverEditing> = {}): PopoverEditing {
  return {
    canEdit: true,
    canDelete: true,
    canDuplicate: true,
    reason: null,
    onEdit: vi.fn(),
    onDelete: vi.fn(),
    onDuplicate: vi.fn(),
    ...over,
  };
}

function deps(over: Partial<EventMenuDeps> = {}): EventMenuDeps {
  const ed = editing();
  return {
    events: [],
    calendars: [calendar],
    milestones: [],
    tasks: [],
    editing: () => ed,
    openDetails: vi.fn(),
    editTask: vi.fn(),
    setTaskStatus: vi.fn(),
    onOpenProject: vi.fn(),
    onOpenPinboard: vi.fn(),
    ...over,
  };
}

const menu = (e: CalendarEvent, d: EventMenuDeps) => eventMenu(e, d, () => rect)!;
type Built = NonNullable<ReturnType<typeof eventMenu>>;
const labels = (m: Built) =>
  m.items.map((i) => (i.kind === "separator" ? "-" : i.kind === "swatches" ? i.label : i.label));
const pick = (m: Built, label: string) => {
  const hit = m.items.find(
    (i) => i.kind !== "separator" && i.kind !== "swatches" && i.label === label,
  );
  if (!hit || hit.kind === "separator" || hit.kind === "swatches") throw new Error(`no ${label}`);
  hit.onSelect();
};

beforeEach(() => vi.clearAllMocks());
afterEach(() => {
  act(() => closeContextMenu());
  setFallbackActions(null);
  cleanup();
});

describe("a synced event's menu", () => {
  it("offers what the popover offers, in its order, the delete last and ending in …", () => {
    const e = ev({ html_link: "https://cal/e1", conference_url: "https://meet/x" });
    const m = menu(e, deps({ milestones: [milestone] }));
    expect(m.label).toBe("Event Standup");
    expect(labels(m)).toEqual([
      "Open details",
      "Edit…",
      "Duplicate",
      "-",
      "Open in Google Calendar",
      "Copy event link",
      "Join the call",
      "Copy call link",
      "Open project Garden",
      "-",
      "Delete…",
    ]);
    const del = m.items[m.items.length - 1];
    expect(del?.kind !== "separator" && del?.kind !== "swatches" && del?.danger).toBe(true);
  });

  it("hides what the event does not offer: no rights, no link, no call, no project", () => {
    const none = editing({ canEdit: false, canDelete: false, canDuplicate: false });
    const m = menu(ev(), deps({ editing: () => none }));
    // Open details carries the reason text, so it is the one item left.
    expect(labels(m)).toEqual(["Open details"]);
  });

  it("hides Edit and Delete on a birthday-like row and Delete alone where only Edit is allowed", () => {
    const m = menu(ev(), deps({ editing: () => editing({ canDelete: false }) }));
    expect(labels(m)).toEqual(["Open details", "Edit…", "Duplicate"]);
  });

  it("says Remove… where the delete only takes the user's copy off", () => {
    const m = menu(ev(), deps({ editing: () => editing({ removes: true }) }));
    expect(labels(m)[labels(m).length - 1]).toBe("Remove…");
  });

  it("has no source link, no project and no call when there is none, and names Outlook", () => {
    const outlook = { id: "cal", provider: "outlook" } as unknown as Calendar;
    const m = menu(ev({ html_link: "https://o/e1" }), deps({ calendars: [outlook] }));
    expect(labels(m)).toContain("Open in Outlook");
    expect(labels(m).some((l) => l.startsWith("Open project") || l.includes("call"))).toBe(false);
  });

  it("calls the handler the popover's own button calls", () => {
    const ed = editing();
    const d = deps({ editing: () => ed });
    const e = ev();
    const m = menu(e, d);
    pick(m, "Open details");
    expect(d.openDetails).toHaveBeenCalledWith(e, rect);
    pick(m, "Edit…");
    expect(ed.onEdit).toHaveBeenCalledTimes(1);
    pick(m, "Duplicate");
    expect(ed.onDuplicate).toHaveBeenCalledTimes(1);
    // Delete… is the popover's delete: it asks, and does nothing else.
    pick(m, "Delete…");
    expect(ed.onDelete).toHaveBeenCalledTimes(1);
  });

  it("opens the source and the call through the link opener, and copies through the host", async () => {
    const copy = vi.fn();
    setFallbackActions({ copy });
    const e = ev({ html_link: "https://cal/e1", conference_url: "https://meet/x" });
    const m = menu(e, deps());
    pick(m, "Open in Google Calendar");
    expect(ipc.openUrl).toHaveBeenCalledWith("https://cal/e1");
    pick(m, "Join the call");
    expect(ipc.openUrl).toHaveBeenCalledWith("https://meet/x");
    // Said aloud by the host, not written to the clipboard by the menu.
    pick(m, "Copy event link");
    expect(copy).toHaveBeenCalledWith("https://cal/e1", "Link copied");
    pick(m, "Copy call link");
    expect(copy).toHaveBeenCalledWith("https://meet/x", "Link copied");
  });

  it("opens the linked milestone's project", () => {
    const d = deps({ milestones: [milestone] });
    pick(menu(ev(), d), "Open project Garden");
    expect(d.onOpenProject).toHaveBeenCalledWith("Garden");
  });

  describe("Log who you met", () => {
    const over = { start: "2020-03-02T10:00:00Z", end: "2020-03-02T11:00:00Z" };
    const later = { start: "2099-03-02T10:00:00Z", end: "2099-03-02T11:00:00Z" };

    it("is offered on a past event with or without guests, after Duplicate, and calls the card", () => {
      const logMeeting = vi.fn();
      const e = ev({ ...over, attendees: [] });
      const m = menu(e, deps({ logMeeting }));
      expect(labels(m)).toEqual([
        "Open details",
        "Edit…",
        "Duplicate",
        "Log who you met",
        "-",
        "Delete…",
      ]);
      pick(m, "Log who you met");
      expect(logMeeting).toHaveBeenCalledWith(e);
    });

    it("is left out for an event that has not happened yet, and where nothing can log", () => {
      expect(labels(menu(ev(later), deps({ logMeeting: vi.fn() })))).not.toContain(
        "Log who you met",
      );
      expect(labels(menu(ev(over), deps()))).not.toContain("Log who you met");
    });

    it("is not offered while quick create is open: the menu keeps to the way back", () => {
      const m = menu(ev(over), deps({ logMeeting: vi.fn(), quickOpen: true }));
      expect(labels(m)).toEqual(["Open details"]);
    });
  });
});

describe("the overlays' menus", () => {
  const taskEv = ev({ id: "task:9", calendar_id: TASK_CALENDAR_ID, uid: null });

  it("a task chip: Edit task…, its status as radios, and its project", () => {
    const t = task({ status: "doing" });
    const d = deps({ tasks: [t] });
    const m = menu(taskEv, d);
    expect(labels(m)).toEqual(["Edit task…", "To do", "Doing", "Done", "-", "Open project Atlas"]);
    const checked = m.items.flatMap((i) =>
      i.kind !== "separator" && i.kind !== "swatches" && i.checked === true ? [i.label] : [],
    );
    expect(checked).toEqual(["Doing"]);
    pick(m, "Edit task…");
    expect(d.editTask).toHaveBeenCalledWith(t);
    pick(m, "Done");
    expect(d.setTaskStatus).toHaveBeenCalledWith(t, "done");
    // The status it already has sends nothing.
    pick(m, "Doing");
    expect(d.setTaskStatus).toHaveBeenCalledTimes(1);
    pick(m, "Open project Atlas");
    expect(d.onOpenProject).toHaveBeenCalledWith("Atlas");
  });

  it("a task with no project has no project item, and one not in the list has no menu", () => {
    expect(labels(menu(taskEv, deps({ tasks: [task({ project_name: null })] })))).not.toContain(
      "Open project Atlas",
    );
    expect(eventMenu(taskEv, deps({ tasks: [] }), () => rect)).toBeNull();
  });

  it("a milestone chip opens its project; a pinboard chip opens the Pinboard", () => {
    const d = deps({ milestones: [milestone] });
    const mEv = ev({ id: "milestone:4", calendar_id: MILESTONE_CALENDAR_ID, uid: null });
    pick(menu(mEv, d), "Open project Garden");
    expect(d.onOpenProject).toHaveBeenCalledWith("Garden");
    const pEv = ev({ id: "pinboard:2", calendar_id: PINBOARD_CALENDAR_ID, uid: null });
    const pm = menu(pEv, d);
    expect(labels(pm)).toEqual(["Open in Pinboard"]);
    pick(pm, "Open in Pinboard");
    expect(d.onOpenPinboard).toHaveBeenCalledTimes(1);
  });
});

describe("the delegated handler", () => {
  function Surface({ d, status }: { d: EventMenuDeps; status?: (s: string) => void }) {
    const onMenu = useEventMenu({ ...d, announce: status ?? (() => {}) });
    return (
      <div onContextMenu={onMenu} data-testid="root">
        <button data-piece-of="e1">
          <span>Standup</span>
        </button>
        <button data-piece-of="e2">Retro</button>
        <button data-piece-of="e1" data-draft-event="">
          ghost
        </button>
        <div data-draft-event="">
          <button data-piece-of="e2">ghost's child</button>
        </div>
        <button data-piece-of="nope">Unknown</button>
        <p>Nothing here</p>
      </div>
    );
  }
  const events = [ev({ id: "e1" }), ev({ id: "e2", summary: "Retro" })];

  it("finds the event by the id on the element, from anywhere inside it", () => {
    render(<Surface d={deps({ events })} />);
    fireEvent.contextMenu(screen.getByText("Standup"));
    expect(currentContextMenu()?.label).toBe("Event Standup");
    act(() => closeContextMenu());
    fireEvent.contextMenu(screen.getByRole("button", { name: "Retro" }));
    expect(currentContextMenu()?.label).toBe("Event Retro");
  });

  it("leaves the ghost of an event being made, an unknown id and empty space alone", () => {
    render(<Surface d={deps({ events })} />);
    fireEvent.contextMenu(screen.getByRole("button", { name: "ghost" }));
    expect(currentContextMenu()).toBeNull();
    fireEvent.contextMenu(screen.getByRole("button", { name: "ghost's child" }));
    expect(currentContextMenu()).toBeNull();
    fireEvent.contextMenu(screen.getByRole("button", { name: "Unknown" }));
    expect(currentContextMenu()).toBeNull();
    fireEvent.contextMenu(screen.getByText("Nothing here"));
    expect(currentContextMenu()).toBeNull();
  });

  it("leaves Shift+right-click to the system menu", () => {
    render(<Surface d={deps({ events })} />);
    fireEvent.contextMenu(screen.getByText("Standup"), { shiftKey: true });
    expect(currentContextMenu()).toBeNull();
  });

  it("anchors Open details at the element the menu was asked on", () => {
    const d = deps({ events });
    render(<Surface d={d} />);
    const piece = screen.getByRole("button", { name: "Standup" });
    piece.getBoundingClientRect = () => rect;
    fireEvent.contextMenu(screen.getByText("Standup"));
    const open = currentContextMenu()!.items[0];
    act(() => (open as { onSelect: () => void }).onSelect());
    expect(d.openDetails).toHaveBeenCalledWith(events[0], rect);
  });

  it("reports a failed status change the way the calendar reports its errors", async () => {
    ipc.changeTaskStatus.mockRejectedValueOnce(new Error("no"));
    const said = vi.fn();
    const taskEv = ev({ id: "task:9", calendar_id: TASK_CALENDAR_ID, uid: null });
    function TaskSurface() {
      const onMenu = useEventMenu({
        ...deps({ events: [taskEv], tasks: [task()] }),
        announce: said,
      });
      return (
        <div onContextMenu={onMenu}>
          <button data-piece-of="task:9">chip</button>
        </div>
      );
    }
    render(<TaskSurface />);
    fireEvent.contextMenu(screen.getByText("chip"));
    const done = currentContextMenu()!.items.find(
      (i) => i.kind !== "separator" && i.kind !== "swatches" && i.label === "Done",
    );
    await act(async () => (done as { onSelect: () => void }).onSelect());
    expect(ipc.changeTaskStatus).toHaveBeenCalledWith(9, "done");
    expect(said).toHaveBeenCalledWith("Error: no");
  });

  it("opens from the keyboard on a focused event, with the first item focused", async () => {
    render(
      <>
        <Surface d={deps({ events })} />
        <ContextMenuHost />
      </>,
    );
    const piece = screen.getByRole("button", { name: "Standup" });
    piece.focus();
    fireEvent.keyDown(piece, { key: "ContextMenu" });
    const first = await screen.findByRole("menuitem", { name: "Open details" });
    expect(document.activeElement).toBe(first);
    expect(currentContextMenu()?.label).toBe("Event Standup");
  });
});
