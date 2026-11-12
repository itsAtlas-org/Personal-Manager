// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The header's one control that changes a calendar (#884, C10): "+ New event" shows once a Google
// account is connected, and when no calendar can take an event it stays focusable and says why
// rather than going grey and silent.

import { createRef, type ReactNode, type RefObject } from "react";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { CalendarHeader } from "./CalendarHeader";
import type { AgendaMode, CalendarViewMode } from "../../lib/calendarPrefs";
import type { EventKinds } from "../../lib/calendarFilters";
import { refreshDescription, refreshTip } from "../../lib/calendarRefresh";
import { formatClock } from "../../lib/format";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// The tooltip shows on KEYBOARD focus only (`:focus-visible`: a click's focus doesn't count), and
// jsdom's idea of that is its own, so a test that Tabs onto a control says so.
function tabbing() {
  const real = Element.prototype.matches;
  vi.spyOn(Element.prototype, "matches").mockImplementation(function (this: Element, sel: string) {
    return sel === ":focus-visible" ? true : real.call(this, sel);
  });
}

const REASON = "None of your calendars can take new events.";

interface Over {
  onCreate?: () => void;
  createReason?: string | null;
  createRef?: RefObject<HTMLButtonElement | null>;
  view?: CalendarViewMode;
  agendaMode?: AgendaMode;
  onAgendaModeChange?: (mode: AgendaMode) => void;
  runwaySlotRef?: (el: HTMLDivElement | null) => void;
  onCreateTask?: () => void;
  hideDone?: boolean;
  hiddenCount?: number;
  onToggleHideDone?: () => void;
  eventKinds?: EventKinds;
  onEventKindsChange?: (kinds: EventKinds) => void;
  onPopOut?: () => void;
  roster?: ReactNode;
  onRefresh?: () => void;
  syncing?: boolean;
  lastSync?: string | null;
}

function header(over: Over = {}) {
  return render(element(over));
}

function element(over: Over = {}) {
  return (
    <CalendarHeader
      view={over.view ?? "week"}
      availableViews={["month", "week", "day", "agenda"]}
      onViewChange={vi.fn()}
      range="day"
      onRangeChange={vi.fn()}
      dayCount={3}
      onDayCountChange={vi.fn()}
      customBounds={{}}
      onBoundsChange={vi.fn()}
      coords={null}
      label="October 2026"
      cursor={new Date(2026, 9, 12)}
      onPickDate={vi.fn()}
      onPrev={vi.fn()}
      onNext={vi.fn()}
      onToday={vi.fn()}
      onRefresh={vi.fn()}
      syncing={false}
      lastSync={null}
      {...over}
    />
  );
}

describe("CalendarHeader", () => {
  it("New event is disabled with its reason when no calendar can take one", () => {
    const onCreate = vi.fn();
    header({ onCreate, createReason: REASON });
    const button = screen.getByRole<HTMLButtonElement>("button", { name: "+ New event" });
    // Still focusable, so a keyboard reaches the reason; marked, not disabled.
    expect(button.disabled).toBe(false);
    expect(button.getAttribute("aria-disabled")).toBe("true");
    const described = document.getElementById(button.getAttribute("aria-describedby") ?? "");
    expect(described?.textContent).toBe(REASON);
    expect(screen.getByRole("status").textContent).toBe("");
    // A click says why, and starts nothing.
    fireEvent.click(button);
    expect(onCreate).not.toHaveBeenCalled();
    expect(screen.getByRole("status").textContent).toBe(REASON);
  });

  it("New event starts one when a calendar can take it", () => {
    const onCreate = vi.fn();
    header({ onCreate, createReason: null });
    const button = screen.getByRole("button", { name: "+ New event" });
    expect(button.getAttribute("aria-disabled")).toBeNull();
    expect(button.getAttribute("aria-keyshortcuts")).toBe("c");
    fireEvent.click(button);
    expect(onCreate).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("status")).toBeNull();
  });

  // One primary button on screen: the editor's or quick create's Save (C1-L5).
  it("New event is a secondary button", () => {
    for (const createReason of [null, REASON]) {
      header({ onCreate: vi.fn(), createReason });
      const button = screen.getByRole("button", { name: "+ New event" });
      expect(button.getAttribute("data-variant")).toBe("secondary");
      cleanup();
    }
  });

  it("says why for six seconds; another click starts over and says it anew", () => {
    vi.useFakeTimers();
    try {
      header({ onCreate: vi.fn(), createReason: REASON });
      const button = screen.getByRole("button", { name: "+ New event" });
      const status = () => screen.getByRole("status");
      const pass = (ms: number) =>
        act(() => {
          vi.advanceTimersByTime(ms);
        });
      fireEvent.click(button);
      const first = status().firstElementChild;
      expect(status().textContent).toBe(REASON);
      pass(4000);
      fireEvent.click(button);
      // New text for a screen reader to say, not the same text left in place.
      expect(status().firstElementChild).not.toBeNull();
      expect(status().firstElementChild).not.toBe(first);
      // Six seconds from the second click, not the first.
      pass(4000);
      expect(status().textContent).toBe(REASON);
      pass(2000);
      expect(status().textContent).toBe("");
    } finally {
      vi.useRealTimers();
    }
  });

  it("forgets a reason said once a calendar can take an event, so the next waits for a click", () => {
    const onCreate = vi.fn();
    const view = header({ onCreate, createReason: REASON });
    fireEvent.click(screen.getByRole("button", { name: "+ New event" }));
    expect(screen.getByRole("status").textContent).toBe(REASON);
    view.rerender(element({ onCreate, createReason: null }));
    view.rerender(element({ onCreate, createReason: REASON }));
    expect(screen.getByRole("status").textContent).toBe("");
  });

  it("hands quick create the New event button, to open under and give focus back to", () => {
    const button = createRef<HTMLButtonElement>();
    header({ onCreate: vi.fn(), createReason: null, createRef: button });
    expect(button.current).toBe(screen.getByRole("button", { name: "+ New event" }));
  });

  it("has no New event without a Google account", () => {
    header();
    expect(screen.queryByRole("button", { name: "+ New event" })).toBeNull();
  });
});

// The Agenda's two layouts (PA-2): the switch sits beside the view switcher in Agenda alone, and
// the Runway, which always runs from today, has no paging or date label to show.
describe("CalendarHeader: the Agenda layout", () => {
  const layout = () => screen.queryByRole("group", { name: "Agenda layout" });

  it("shows the Runway | List switch in Agenda only", () => {
    const onAgendaModeChange = vi.fn();
    const view = header({ view: "week", onAgendaModeChange });
    expect(layout()).toBeNull();
    view.rerender(element({ view: "agenda", agendaMode: "list", onAgendaModeChange }));
    const group = layout();
    expect(group).not.toBeNull();
    const runway = within(group!).getByRole("button", { name: "Runway" });
    expect(within(group!).getByRole("button", { name: "List" }).getAttribute("aria-pressed")).toBe(
      "true",
    );
    fireEvent.click(runway);
    expect(onAgendaModeChange).toHaveBeenCalledWith("runway");
  });

  it("the Runway hides the paging and the date label, and says it runs from today", () => {
    header({ view: "agenda", agendaMode: "runway", onAgendaModeChange: vi.fn() });
    expect(screen.queryByRole("button", { name: "Previous" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Next" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Today" })).toBeNull();
    expect(screen.queryByText("October 2026")).toBeNull();
    expect(screen.getByText("From today")).toBeTruthy();
  });

  it("the List keeps them", () => {
    header({ view: "agenda", agendaMode: "list", onAgendaModeChange: vi.fn() });
    expect(screen.getByRole("button", { name: "Previous" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Today" })).toBeTruthy();
    expect(screen.queryByText("From today")).toBeNull();
  });

  it("the Runway gives the board a slot beside From today, the List none", () => {
    const slotRef = vi.fn();
    const view = header({
      view: "agenda",
      agendaMode: "runway",
      onAgendaModeChange: vi.fn(),
      runwaySlotRef: slotRef,
    });
    const lastSlot = () =>
      slotRef.mock.calls[slotRef.mock.calls.length - 1][0] as HTMLElement | null;
    const slot = lastSlot()!;
    expect(slot).toBeInstanceOf(HTMLElement);
    // Next to the label, inside the header's left group, and empty (hidden) until the board draws.
    expect(slot.previousElementSibling?.textContent).toBe("From today");
    expect(slot.closest('[data-help="calendar-header"]')).not.toBeNull();
    expect(screen.getByText("From today")).toBeTruthy();
    // Back to the List: the slot goes, and the board's ref hears null.
    view.rerender(
      element({
        view: "agenda",
        agendaMode: "list",
        onAgendaModeChange: vi.fn(),
        runwaySlotRef: slotRef,
      }),
    );
    expect(lastSlot()).toBeNull();
    expect(screen.queryByText("From today")).toBeNull();
  });

  it("+ Task shows where onCreateTask is given, just before + New event, and not in List", () => {
    const onCreateTask = vi.fn();
    const view = header({
      view: "agenda",
      agendaMode: "runway",
      onAgendaModeChange: vi.fn(),
      onCreate: vi.fn(),
      onCreateTask,
    });
    const task = screen.getByRole("button", { name: "+ Task" });
    // The Runway's help names it, as it names Hide events and + Window.
    expect(task.getAttribute("data-help")).toBe("calendar-runway");
    const buttons = screen.getAllByRole("button");
    expect(buttons[buttons.indexOf(task) + 1]).toBe(
      screen.getByRole("button", { name: "+ New event" }),
    );
    fireEvent.click(task);
    expect(onCreateTask).toHaveBeenCalledOnce();
    // CalendarView hands it over only on the Runway.
    view.rerender(
      element({
        view: "agenda",
        agendaMode: "list",
        onAgendaModeChange: vi.fn(),
        onCreate: vi.fn(),
      }),
    );
    expect(screen.queryByRole("button", { name: "+ Task" })).toBeNull();
  });

  it("offers Pop out where it's given, and not in the pop-out itself", () => {
    const onPopOut = vi.fn();
    const { unmount } = header({ onPopOut });
    fireEvent.click(screen.getByRole("button", { name: "Pop out" }));
    expect(onPopOut).toHaveBeenCalledOnce();
    unmount();
    header();
    expect(screen.queryByRole("button", { name: "Pop out" })).toBeNull();
  });

  it("Pop out is an icon named Pop out, just before Refresh", () => {
    header({ onPopOut: vi.fn() });
    const popOut = screen.getByRole("button", { name: "Pop out" });
    // An icon: no words on it, and the drawing is hidden from assistive technology.
    expect(popOut.textContent).toBe("");
    expect(popOut.querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");
    expect(popOut.getAttribute("data-help")).toBe("calendar-popout");
    expect(popOut.nextElementSibling).toBeNull();
    const buttons = screen.getAllByRole("button");
    expect(buttons[buttons.indexOf(popOut) + 1]).toBe(
      screen.getByRole("button", { name: "Refresh" }),
    );
  });

  it("the Calendars slot sits just before Refresh, and only where it is given", () => {
    header({ roster: <button type="button">Calendars 2/3</button> });
    const buttons = screen.getAllByRole("button");
    const at = buttons.indexOf(screen.getByRole("button", { name: "Calendars 2/3" }));
    expect(buttons[at + 1]).toBe(screen.getByRole("button", { name: "Refresh" }));
    cleanup();
    header();
    expect(screen.queryByRole("button", { name: /Calendars/ })).toBeNull();
  });
});

// The Agenda's Hide done and the All | All-day filter (#1090): each is drawn only where CalendarView
// hands it a handler, so which views carry them is CalendarView's test; this one is how they read.
describe("CalendarHeader: the filters", () => {
  const hideDone = () => screen.queryByRole("button", { name: /^(Hide|Show) done/ });

  it("Hide done sits beside Runway | List in the Agenda, and flips its label with the count", () => {
    const onToggleHideDone = vi.fn();
    const view = header({
      view: "agenda",
      agendaMode: "runway",
      onAgendaModeChange: vi.fn(),
      onToggleHideDone,
      hideDone: false,
      hiddenCount: 0,
    });
    const button = screen.getByRole("button", { name: "Hide done" });
    // No aria-pressed: the label flips instead, as Hide events does.
    expect(button.getAttribute("aria-pressed")).toBeNull();
    const layout = screen.getByRole("group", { name: "Agenda layout" });
    expect(layout.compareDocumentPosition(button) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    fireEvent.click(button);
    expect(onToggleHideDone).toHaveBeenCalledTimes(1);

    view.rerender(
      element({
        view: "agenda",
        agendaMode: "runway",
        onAgendaModeChange: vi.fn(),
        onToggleHideDone,
        hideDone: true,
        hiddenCount: 4,
      }),
    );
    expect(hideDone()?.textContent).toBe("Show done (4)");
    // Nothing hidden now: plain "Show done", with no "(0)".
    view.rerender(
      element({
        view: "agenda",
        agendaMode: "list",
        onAgendaModeChange: vi.fn(),
        onToggleHideDone,
        hideDone: true,
        hiddenCount: 0,
      }),
    );
    expect(hideDone()?.textContent).toBe("Show done");
  });

  it("Hide done names what it hides in its tooltip, and is there only where it is given", () => {
    header({ view: "agenda", agendaMode: "list", onToggleHideDone: vi.fn() });
    expect(hideDone()?.getAttribute("title")).toMatch(
      /tasks you've finished.*events that have ended/,
    );
    cleanup();
    for (const view of ["month", "year", "week", "day"] as const) {
      header({ view });
      expect(hideDone()).toBeNull();
      cleanup();
    }
  });

  it("All | All-day is a two-option control named Which events, where it is given", () => {
    const onEventKindsChange = vi.fn();
    const view = header({ view: "month", eventKinds: "all", onEventKindsChange });
    const group = screen.getByRole("group", { name: "Which events" });
    const all = within(group).getByRole("button", { name: "All" });
    const allDay = within(group).getByRole("button", { name: "All-day" });
    expect(all.getAttribute("aria-pressed")).toBe("true");
    expect(allDay.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(allDay);
    expect(onEventKindsChange).toHaveBeenCalledWith("all-day");
    view.rerender(element({ view: "month", eventKinds: "all-day", onEventKindsChange }));
    expect(
      within(screen.getByRole("group", { name: "Which events" }))
        .getByRole("button", { name: "All-day" })
        .getAttribute("aria-pressed"),
    ).toBe("true");
    // Not given (Week, Day, the Runway with its events hidden): not drawn.
    view.rerender(element({ view: "week" }));
    expect(screen.queryByRole("group", { name: "Which events" })).toBeNull();
  });

  it("both sit before the view switcher, and the header keeps Refresh last", () => {
    header({
      view: "agenda",
      agendaMode: "list",
      onAgendaModeChange: vi.fn(),
      onToggleHideDone: vi.fn(),
      onEventKindsChange: vi.fn(),
    });
    const hide = screen.getByRole("button", { name: "Hide done" });
    const kinds = screen.getByRole("group", { name: "Which events" });
    const views = screen.getByRole("group", { name: "Calendar view" });
    const refresh = screen.getByRole("button", { name: "Refresh" });
    const follows = (a: HTMLElement, b: HTMLElement) =>
      !!(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
    expect(follows(hide, kinds)).toBe(true);
    expect(follows(kinds, views)).toBe(true);
    expect(follows(views, refresh)).toBe(true);
  });
});

// Refresh is an icon in the header's corner (Bobby: "i instinctively click top right for refresh"),
// with a constant name and the last sync as a description and a tooltip.
describe("CalendarHeader: Refresh", () => {
  const refresh = () => screen.getByRole("button", { name: "Refresh" });
  const SYNCED = new Date().toISOString();

  it("is the header's last control, named Refresh, and described by the last sync", () => {
    header({ onCreate: vi.fn(), onPopOut: vi.fn(), lastSync: SYNCED });
    const buttons = screen.getAllByRole("button");
    expect(buttons[buttons.length - 1]).toBe(refresh());
    // Icon only, at every Depth: no words on it, and no native title beside the JS tooltip.
    expect(refresh().textContent).toBe("");
    expect(refresh().getAttribute("title")).toBeNull();
    const described = document.getElementById(refresh().getAttribute("aria-describedby") ?? "");
    expect(described?.textContent).toBe(refreshDescription(SYNCED, false));
    expect(described?.textContent).toMatch(/^Last synced /);
  });

  it("sits in a cell of its own, first row, apart from the controls that wrap", () => {
    // jsdom has no layout, so this pins the structure that keeps the corner: Refresh's cell is
    // pinned to row 1 and shares no wrapping container with the view switcher, + New event,
    // Pop out or Calendars.
    header({
      onCreate: vi.fn(),
      onPopOut: vi.fn(),
      roster: <button type="button">Calendars 1/2</button>,
    });
    const cell = refresh().closest("span")?.parentElement as HTMLElement;
    expect(cell.className).toContain("row-start-1");
    expect(within(cell).getAllByRole("button")).toEqual([refresh()]);
    const controls = screen.getByRole("group", { name: "Calendar view" })
      .parentElement as HTMLElement;
    expect(controls.contains(refresh())).toBe(false);
    expect(controls.className).toContain("flex-wrap");
  });

  it("keeps its name when it has never synced, and says so as the description", () => {
    header({ lastSync: null });
    const described = document.getElementById(refresh().getAttribute("aria-describedby") ?? "");
    expect(described?.textContent).toBe("Never synced");
  });

  it("calls onRefresh when pressed", () => {
    const onRefresh = vi.fn();
    header({ onRefresh });
    fireEvent.click(refresh());
    expect(onRefresh).toHaveBeenCalledOnce();
  });

  it("while syncing is aria-busy and aria-disabled, stays focused, and ignores clicks", () => {
    const onRefresh = vi.fn();
    header({ onRefresh, syncing: true });
    const button = refresh();
    button.focus();
    // Marked, not disabled: a focused button that turns disabled would drop keyboard focus.
    expect(button.getAttribute("aria-busy")).toBe("true");
    expect(button.getAttribute("aria-disabled")).toBe("true");
    expect((button as HTMLButtonElement).disabled).toBe(false);
    expect(document.activeElement).toBe(button);
    fireEvent.click(button);
    expect(onRefresh).not.toHaveBeenCalled();
    // The icon turns, but not under reduced motion; the description still says why.
    expect(button.querySelector("svg")?.getAttribute("class")).toContain(
      "motion-reduce:animate-none",
    );
    expect(button.querySelector("svg")?.getAttribute("class")).toContain("animate-spin");
    const described = document.getElementById(button.getAttribute("aria-describedby") ?? "");
    expect(described?.textContent).toBe("Refreshing…");
  });

  it("is neither busy nor disabled while idle", () => {
    header({ syncing: false });
    expect(refresh().getAttribute("aria-busy")).toBeNull();
    expect(refresh().getAttribute("aria-disabled")).toBeNull();
    expect(refresh().querySelector("svg")?.getAttribute("class")).not.toContain("animate-spin");
  });

  // The JS tooltip, which a keyboard user gets on focus as a mouse user does on hover.
  it("shows 'Refresh · last synced …' on focus and 'Refreshing…' while syncing", () => {
    vi.useFakeTimers();
    try {
      const { rerender } = header({ lastSync: SYNCED });
      tabbing();
      fireEvent.focus(refresh());
      act(() => void vi.advanceTimersByTime(200));
      expect(screen.getByRole("tooltip").textContent).toBe(refreshTip(SYNCED, false));
      expect(screen.getByRole("tooltip").textContent).toBe(
        `Refresh · last synced ${formatClock(new Date(SYNCED))}`,
      );
      rerender(element({ lastSync: SYNCED, syncing: true }));
      expect(screen.getByRole("tooltip").textContent).toBe("Refreshing…");
    } finally {
      vi.useRealTimers();
    }
  });
});

// The keys that page the calendar are findable: ‹ › and Today say them in the tooltip (the repo's
// "label · key" form, which shows on focus too) and in aria-keyshortcuts, while their names stay
// "Previous", "Next" and "Today".
describe("CalendarHeader: the keys", () => {
  const prev = () => screen.getByRole("button", { name: "Previous" });
  const next = () => screen.getByRole("button", { name: "Next" });
  const today = () => screen.getByRole("button", { name: "Today" });

  it("names the arrows and Google's letters on ‹ › and t on Today", () => {
    header({ view: "week" });
    expect(prev().getAttribute("aria-keyshortcuts")).toBe("ArrowLeft k p");
    expect(next().getAttribute("aria-keyshortcuts")).toBe("ArrowRight j n");
    expect(today().getAttribute("aria-keyshortcuts")).toBe("t");
  });

  it("adds ↑ and ↓ in Month and Year only", () => {
    for (const view of ["month", "year"] as const) {
      header({ view });
      expect(prev().getAttribute("aria-keyshortcuts")).toBe("ArrowLeft ArrowUp k p");
      expect(next().getAttribute("aria-keyshortcuts")).toBe("ArrowRight ArrowDown j n");
      cleanup();
    }
    for (const view of ["week", "day", "agenda"] as const) {
      header({ view });
      expect(prev().getAttribute("aria-keyshortcuts")).not.toContain("ArrowUp");
      expect(next().getAttribute("aria-keyshortcuts")).not.toContain("ArrowDown");
      cleanup();
    }
  });

  it("keeps the buttons' names, whatever the tooltip says", () => {
    header({ view: "month" });
    expect(prev().getAttribute("aria-label")).toBe("Previous");
    expect(next().getAttribute("aria-label")).toBe("Next");
    expect(today().textContent).toBe("Today");
  });

  // The tooltip is the JS one, which a keyboard user gets on focus as a mouse user does on hover.
  it("shows the keys in the tooltip on focus", () => {
    vi.useFakeTimers();
    try {
      header({ view: "month" });
      tabbing();
      const shown = (button: HTMLElement) => {
        fireEvent.focus(button);
        act(() => void vi.advanceTimersByTime(200));
        const text = screen.getByRole("tooltip").textContent;
        fireEvent.blur(button);
        return text;
      };
      expect(shown(prev())).toBe("Previous · ←, ↑, k or p");
      expect(shown(next())).toBe("Next · →, ↓, j or n");
      expect(shown(today())).toBe("Jump to today · t");
      cleanup();
      header({ view: "week" });
      expect(shown(prev())).toBe("Previous · ←, k or p");
      expect(shown(next())).toBe("Next · →, j or n");
    } finally {
      vi.useRealTimers();
    }
  });
});
