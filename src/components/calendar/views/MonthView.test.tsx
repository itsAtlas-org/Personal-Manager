// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The Month view draws the event being made (#884, C10c) as its ghost: a chip, or a band across days,
// dashed and in its calendar's colour, never greyed as past, named as the new event, beside synced
// events drawn as ever; never folded into "+N more" on a busy day; and at Minimal depth, a dashed
// ring among the dots. On a calendar that drags (C16c) it marks what the drag measures and what a
// press must never create on, draws the ghost over the days it would land on, and stops snapping
// while a drag scrolls it; without that, nothing changes.

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { CalendarEvent } from "../../../lib/types";
import { dayKey, DRAFT_EVENT_ID } from "../../../lib/calendar-layout";
import { ghostEvent } from "../../../lib/calendarDrag";
import { formatClock } from "../../../lib/format";
import type { MonthInteraction } from "./MonthView";

let depth = "standard";

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth }),
}));

import { MonthView } from "./MonthView";

// jsdom lays nothing out and ships no ResizeObserver, and the view draws its weeks only once it has
// measured its pane: give every element a height, and the pane a scrollTo, then put them back.
let restore: (() => void) | null = null;
beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
  const height = Object.getOwnPropertyDescriptor(Element.prototype, "clientHeight");
  const scrollTo = Object.getOwnPropertyDescriptor(Element.prototype, "scrollTo");
  Object.defineProperty(Element.prototype, "clientHeight", { configurable: true, get: () => 600 });
  Object.defineProperty(Element.prototype, "scrollTo", { configurable: true, value: () => {} });
  restore = () => {
    if (height) Object.defineProperty(Element.prototype, "clientHeight", height);
    if (scrollTo) Object.defineProperty(Element.prototype, "scrollTo", scrollTo);
    else delete (Element.prototype as { scrollTo?: unknown }).scrollTo;
  };
});
afterAll(() => restore?.());

afterEach(cleanup);

const ev = (over: Partial<CalendarEvent>): CalendarEvent => ({
  id: "e1",
  calendar_id: "home",
  summary: "Synced",
  description: null,
  location: null,
  start: "2026-10-12T09:00:00",
  end: "2026-10-12T10:00:00",
  all_day: false,
  html_link: null,
  uid: null,
  ...over,
});

// A chip says its time in the reader's own clock (09:00 here, 09:00 AM on CI's runner), so the names
// are built with the helper the view uses and hold in any locale.
const clock = (h: number) => formatClock(new Date(2026, 9, 13, h));

function month(events: CalendarEvent[], interaction?: MonthInteraction) {
  const view = (i?: MonthInteraction) => (
    <MonthView
      cursor={new Date(2026, 9, 1)}
      events={events}
      colorOf={(id) => (id === "work" ? "#336699" : "#993366")}
      onFocusDate={() => {}}
      // Every event here is over by now: a synced one greys, the ghost doesn't.
      now={new Date(2026, 9, 20)}
      onEventClick={vi.fn()}
      interaction={i}
    />
  );
  const utils = render(view(interaction));
  return { rerender: (i?: MonthInteraction) => utils.rerender(view(i)) };
}

const still: MonthInteraction = {
  originId: null,
  ghost: null,
  pendingIds: new Set(),
  live: false,
};
const stream = () => document.querySelector<HTMLElement>(".overflow-y-auto")!;
// The stream's one child holds the drawn weeks, each placed at its own row.
const drawnRows = () => [...stream().firstElementChild!.children] as HTMLElement[];

describe("MonthView", () => {
  it("draws the new event's ghost dashed in its calendar's colour, as a chip or a band", () => {
    month([
      ev({}),
      ev({
        id: DRAFT_EVENT_ID,
        calendar_id: "work",
        summary: "Dentist",
        start: "2026-10-13T11:00:00",
      }),
    ]);
    // Named as the new event, not yet saved, so it isn't heard as one that's in the calendar.
    const chip = screen.getByRole("button", {
      name: `New event, not saved yet: ${clock(11)}, Dentist`,
    });
    // Dashed inside its edge: a cell's list clips its chips, and would take an outer dash away.
    expect(chip.className).toContain("outline-dashed");
    expect(chip.className).toContain("-outline-offset-1");
    expect(chip.className).not.toContain("opacity-60");
    expect(chip.hasAttribute("data-draft-event")).toBe(true);
    expect(chip.style.borderLeftColor).toBe("rgb(51, 102, 153)");
    const synced = screen.getByRole("button", { name: `${clock(9)}, Synced` });
    expect(synced.className).not.toContain("outline-dashed");
    expect(synced.className).toContain("opacity-60");
    cleanup();
    month([
      {
        ...ev({
          id: DRAFT_EVENT_ID,
          calendar_id: "work",
          summary: "Away",
          start: "2026-10-13",
          end: "2026-10-16",
          all_day: true,
        }),
        draft_state: "saving",
      } as CalendarEvent,
    ]);
    const band = screen.getByRole("button", { name: "New event, saving: Away" });
    expect(band.className).toContain("outline-dashed");
    expect(band.className).toContain("-outline-offset-1");
    expect(band.className).not.toContain("opacity-60");
    expect(band.hasAttribute("data-draft-event")).toBe(true);
  });

  // Three events earlier in the day fill the cell: the ghost, later than all of them, still shows as
  // it's typed, in the last place, and the count stays honest.
  it("keeps the ghost on a busy day, in the last place, rather than in +N more", () => {
    month([
      ev({ id: "a", summary: "One", start: "2026-10-13T08:00:00" }),
      ev({ id: "b", summary: "Two", start: "2026-10-13T09:00:00" }),
      ev({ id: "c", summary: "Three", start: "2026-10-13T10:00:00" }),
      ev({ id: DRAFT_EVENT_ID, summary: "Dentist", start: "2026-10-13T15:00:00" }),
    ]);
    expect(
      screen.getByRole("button", { name: `New event, not saved yet: ${clock(15)}, Dentist` }),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: `${clock(10)}, Three` })).toBeNull();
    expect(screen.getByText("+1 more")).toBeTruthy();
  });

  // Minimal depth draws dots: the ghost's is a dashed ring, not a fill, carries the mark that keeps a
  // click on it from closing quick create, and is the one dot with a name. On a day with more dots
  // than fit, it takes the last place rather than folding into the count.
  it("draws the ghost as a dashed ring among the dots at Minimal depth", () => {
    depth = "min";
    try {
      month([
        ...[8, 9, 10, 11, 12].map((h) =>
          ev({ id: `s${h}`, start: `2026-10-12T${String(h).padStart(2, "0")}:00:00` }),
        ),
        ev({
          id: DRAFT_EVENT_ID,
          calendar_id: "work",
          summary: "Dentist",
          start: "2026-10-12T15:00:00",
        }),
      ]);
      const ghost = document.querySelector("[data-draft-event]");
      expect(ghost).not.toBeNull();
      const ring = ghost!.querySelector<HTMLElement>("[aria-hidden]")!;
      expect(ring.className).toContain("border-dashed");
      expect(ring.style.borderColor).toBe("rgb(51, 102, 153)");
      expect(ring.style.backgroundColor).toBe("");
      expect(ghost!.textContent).toBe("New event, not saved yet: Dentist");
      // A synced event's dot is filled, as ever, and has no mark: four of five, and one counted.
      const dots = document.querySelectorAll<HTMLElement>("span[aria-hidden]");
      const filled = [...dots].filter((d) => d.style.backgroundColor === "rgb(153, 51, 102)");
      expect(filled).toHaveLength(4);
      expect(filled.every((d) => d.closest("[data-draft-event]") === null)).toBe(true);
      expect(screen.getByText("+1")).toBeTruthy();
    } finally {
      depth = "standard";
    }
  });

  // #884 C14b: an event the user declined (the calendar marks it, `markDeclined`) is struck
  // through, as Google draws it, and its name says so; any other is drawn as ever.
  it("strikes through an event the user declined, as a chip or a band, and says so", () => {
    const declined = (e: CalendarEvent) => ({ ...e, declined: true }) as CalendarEvent;
    month([
      declined(ev({ id: "c", summary: "Review", start: "2026-10-13T09:00:00" })),
      ev({ id: "p", summary: "Plain", start: "2026-10-14T09:00:00" }),
      declined(
        ev({ id: "b", summary: "Away", start: "2026-10-20", end: "2026-10-23", all_day: true }),
      ),
    ]);
    const chip = screen.getByRole("button", { name: `${clock(9)}, Review, declined` });
    expect(within(chip).getByText("Review").className).toContain("line-through");
    const plain = screen.getByRole("button", { name: `${clock(9)}, Plain` });
    expect(within(plain).getByText("Plain").className).not.toContain("line-through");
    const band = screen.getByRole("button", { name: "Away, declined" });
    expect(within(band).getByText("Away").className).toContain("line-through");
  });
});

// #884 C16c: Month on a calendar that drags. Each row's text is checked through the attributes and
// classes the drag and the eye read, since jsdom lays nothing out.
describe("MonthView: dragging", () => {
  const busy = [
    ev({ id: "a", summary: "One", start: "2026-10-13T08:00:00" }),
    ev({ id: "b", summary: "Two", start: "2026-10-13T09:00:00" }),
    ev({ id: "c", summary: "Three", start: "2026-10-13T10:00:00" }),
    ev({ id: "d", summary: "Four", start: "2026-10-13T11:00:00" }),
    ev({ id: "w", summary: "Away", start: "2026-10-20", end: "2026-10-23", all_day: true }),
  ];

  it("without interaction nothing changes: no marks, and the weeks snap", () => {
    month(busy);
    expect(
      document.querySelector("[data-surface], [data-event-id], [data-no-create], [data-day]"),
    ).toBeNull();
    expect(stream().className).toContain("snap-y");
    expect(stream().className).not.toContain("select-none");
  });

  it("marks the weeks, each day, and each chip and band with its event", () => {
    month(busy, still);
    const s = stream();
    expect(s.dataset.surface).toBe("month");
    // 120 months before October 2026 (1-10-2016 is a Saturday), from its Monday; a sixth of the
    // 600 px pane a week.
    expect(s.dataset.gridStart).toBe("2016-09-26");
    expect(s.dataset.weekH).toBe("100");
    // One row a week over ten years each side, and the drag knows how many; only the rows near
    // the viewport are drawn: the six in view and twelve more each side.
    expect(Number(s.dataset.weeks)).toBeGreaterThan(1000);
    expect(drawnRows().length).toBeLessThanOrEqual(6 + 2 * 12);
    expect(drawnRows().length).toBeGreaterThan(12);
    expect(s.className).toContain("select-none");
    expect(document.querySelector("[data-day='2026-10-13']")).not.toBeNull();
    const chip = screen.getByRole("button", { name: `${clock(8)}, One` });
    expect(chip.dataset.eventId).toBe("a");
    const band = screen.getByRole("button", { name: "Away" });
    expect(band.dataset.eventId).toBe("w");
  });

  it("day numbers, month names, more and the counts never create", () => {
    const marked = () =>
      [...document.querySelectorAll<HTMLElement>("[data-no-create]")].map((el) => el.textContent);
    const inCell = (day: string) =>
      [...document.querySelectorAll<HTMLElement>(`[data-day='${day}'] [data-no-create]`)].map(
        (el) => el.textContent,
      );
    month(busy, still);
    expect(inCell("2026-10-13")).toEqual(["13", "+1 more"]);
    const first = new Date(2026, 9, 1).toLocaleDateString(undefined, { month: "short" });
    expect(inCell("2026-10-01")).toEqual([first, "1"]);
    cleanup();
    for (const [d, count] of [
      ["power", "4"],
      ["min", "+1"],
    ] as const) {
      depth = d;
      try {
        month(
          d === "min"
            ? [8, 9, 10, 11, 12, 13].map((h) =>
                ev({ id: `s${h}`, start: `2026-10-13T${String(h).padStart(2, "0")}:00:00` }),
              )
            : busy,
          still,
        );
        expect(inCell("2026-10-13")).toContain(count);
        expect(marked().length).toBeGreaterThan(0);
      } finally {
        depth = "standard";
        cleanup();
      }
    }
  });

  it("a live drag turns snapping off, and it comes back after", () => {
    const { rerender } = month(busy, still);
    expect(stream().className).toContain("snap-y");
    rerender({ ...still, live: true });
    expect(stream().className).toContain("snap-none");
    expect(stream().className).not.toContain("snap-y");
    rerender(still);
    expect(stream().className).toContain("snap-y");
    expect(stream().className).not.toContain("snap-none");
  });

  it("draws the ghost over the days it would land on, in each week it touches", () => {
    const away = busy[4];
    // Moved to Saturday 17-10 to Tuesday 20-10: two days in each of two weeks.
    const event = ghostEvent(away, { kind: "all_day", first: "2026-10-17", last: "2026-10-20" });
    month(busy, {
      ...still,
      live: true,
      originId: away.id,
      ghost: { event, kind: "move", invalid: null, label: "17-10 – 20-10" },
    });
    const ghosts = [...document.querySelectorAll<HTMLElement>("[data-drag-ghost]")];
    expect(ghosts).toHaveLength(2);
    expect(ghosts.map((g) => [parseFloat(g.style.left), parseFloat(g.style.width)])).toEqual([
      [(5 / 7) * 100, (2 / 7) * 100],
      [0, (2 / 7) * 100],
    ]);
    expect(ghosts[0].getAttribute("aria-hidden")).toBe("true");
    expect(ghosts[0].textContent).toBe("Away · 17-10 – 20-10");
  });

  it("the dragged event dims where it was, and one being saved is dashed", () => {
    month(busy, { ...still, live: true, originId: "a", pendingIds: new Set(["w"]) });
    const dragged = screen.getByRole("button", { name: `${clock(8)}, One` });
    expect(dragged.className).toContain("opacity-40");
    expect(dragged.className).not.toContain("opacity-60");
    const saving = screen.getByRole("button", { name: "Away" });
    expect(saving.className).toContain("opacity-70");
    expect(saving.className).toContain("outline-dashed");
    expect(saving.className).not.toContain("opacity-60");
    // Everything else as ever: past, so greyed.
    expect(screen.getByRole("button", { name: `${clock(9)}, Two` }).className).toContain(
      "opacity-60",
    );
  });
});

// Ten years of weeks each side of an anchor, only the ones near the viewport drawn, so scrolling
// never reaches an edge: the wall at ~20 months is gone, and a jump past the ten years re-anchors.
// jsdom lays out nothing, but its scrollTop is a plain field, so a test sets it and fires `scroll`.
describe("MonthView: the stream has no wall", () => {
  const WEEK_MS = 7 * 86_400_000;
  // October 2026's first row: the Monday of the week holding the 1st, 28-9-2026, in a stream that
  // starts on 26-9-2016, 522 weeks on.
  const OCT_ROW = Math.round((Date.UTC(2026, 8, 28) - Date.UTC(2016, 8, 26)) / WEEK_MS);
  const key = (d: Date) => dayKey(d);
  const rowStart = (row: number) => new Date(2026, 8, 28 + (row - OCT_ROW) * 7);

  function mountAt(cursor: Date, onFocusDate: (d: Date) => void = () => {}) {
    const view = (c: Date) => (
      <MonthView
        cursor={c}
        events={[]}
        colorOf={() => "#336699"}
        onFocusDate={onFocusDate}
        now={new Date(2026, 9, 20)}
        interaction={still}
      />
    );
    const utils = render(view(cursor));
    return { rerender: (c: Date) => utils.rerender(view(c)) };
  }
  const settle = (ms: number) => act(() => void vi.advanceTimersByTime(ms));

  beforeEach(() => {
    vi.useFakeTimers({
      toFake: ["requestAnimationFrame", "cancelAnimationFrame", "setTimeout", "clearTimeout"],
    });
  });
  afterEach(() => vi.useRealTimers());

  it("opens on the cursor's week", () => {
    mountAt(new Date(2026, 9, 1));
    expect(OCT_ROW).toBe(522);
    expect(document.querySelector(`[data-day='${key(rowStart(OCT_ROW))}']`)).not.toBeNull();
    // Ten years back is drawn only once it is scrolled to.
    expect(document.querySelector("[data-day='2016-09-26']")).toBeNull();
  });

  it("scrolls past the old ±20-month edge: those weeks are drawn, the stream doesn't change, the month is reported", () => {
    const onFocusDate = vi.fn();
    mountAt(new Date(2026, 9, 1), onFocusDate);
    // The opening scroll is PM's own, and mutes the report until it has ended (700 ms at most).
    settle(700);
    const s = stream();
    const weeks = s.dataset.weeks;
    const start = s.dataset.gridStart;
    // 150 weeks on: nearly three years, well past where the old stream ended (87 weeks).
    const row = OCT_ROW + 150;
    s.scrollTop = row * 100;
    fireEvent.scroll(s);
    settle(20);
    expect(document.querySelector(`[data-day='${key(rowStart(row))}']`)).not.toBeNull();
    // The old weeks are let go, so the drawn rows stay a few.
    expect(document.querySelector(`[data-day='${key(rowStart(OCT_ROW))}']`)).toBeNull();
    expect(drawnRows().length).toBeLessThanOrEqual(6 + 2 * 12);
    // Scrolling doesn't move the stream: there is no re-anchor to wait for, and no wall.
    expect(s.dataset.weeks).toBe(weeks);
    expect(s.dataset.gridStart).toBe(start);
    // The week 40% down the pane is 152 weeks on; its Thursday's month is the focus.
    const thursday = new Date(2026, 8, 28 + (152 - 0) * 7 + 3);
    expect(onFocusDate).toHaveBeenCalledTimes(1);
    expect(onFocusDate).toHaveBeenCalledWith(
      new Date(thursday.getFullYear(), thursday.getMonth(), 1),
    );
  });

  it("draws the weeks at the very ends of the stream", () => {
    mountAt(new Date(2026, 9, 1));
    const s = stream();
    const rows = Number(s.dataset.weeks);
    s.scrollTop = (rows - 6) * 100;
    fireEvent.scroll(s);
    settle(20);
    // The last row is drawn, right to the bottom of the full-height stream.
    const last = drawnRows()[drawnRows().length - 1];
    expect(last.style.top).toBe(`${(rows - 1) * 100}px`);
    s.scrollTop = 0;
    fireEvent.scroll(s);
    settle(20);
    expect(drawnRows()[0].style.top).toBe("0px");
    expect(document.querySelector("[data-day='2016-09-26']")).not.toBeNull();
  });

  it("places each drawn week at its own row, in a stream of the whole height", () => {
    mountAt(new Date(2026, 9, 1));
    const rows = Number(stream().dataset.weeks);
    const inner = stream().firstElementChild as HTMLElement;
    expect(inner.style.height).toBe(`${rows * 100}px`);
    const tops = drawnRows().map((w) => parseInt(w.style.top, 10));
    expect(tops).toEqual([...tops].sort((a, b) => a - b));
    expect(tops.every((t) => t % 100 === 0)).toBe(true);
    expect(drawnRows().every((w) => w.style.height === "100px")).toBe(true);
    // Row 522 is October's first week, at 52,200 px.
    expect(tops).toContain(OCT_ROW * 100);
  });

  it("re-anchors for a jump past the ten years, and draws the cursor's week", () => {
    const { rerender } = mountAt(new Date(2026, 9, 1));
    const was = stream().dataset.gridStart;
    rerender(new Date(2050, 0, 1));
    const next = stream().dataset.gridStart;
    expect(next).not.toBe(was);
    // 120 months before January 2050, from its Monday (the 1st of January 2040 is a Sunday).
    expect(next).toBe("2039-12-26");
    expect(document.querySelector("[data-day='2050-01-03']")).not.toBeNull();
  });

  it("does not re-anchor for a jump inside the ten years, and draws that week", () => {
    const { rerender } = mountAt(new Date(2026, 9, 1));
    const was = stream().dataset.gridStart;
    rerender(new Date(2031, 2, 1));
    expect(stream().dataset.gridStart).toBe(was);
    expect(document.querySelector("[data-day='2031-03-05']")).not.toBeNull();
  });

  describe("PM's own scrolls", () => {
    let scrollTo: ReturnType<typeof vi.fn>;
    let kept: PropertyDescriptor | undefined;
    beforeEach(() => {
      kept = Object.getOwnPropertyDescriptor(Element.prototype, "scrollTo");
      scrollTo = vi.fn();
      Object.defineProperty(Element.prototype, "scrollTo", { configurable: true, value: scrollTo });
    });
    afterEach(() => {
      if (kept) Object.defineProperty(Element.prototype, "scrollTo", kept);
    });

    it("glides to a month that's near, and jumps to one that's far", () => {
      const { rerender } = mountAt(new Date(2026, 9, 1));
      // The first paint is a jump.
      expect(scrollTo).toHaveBeenLastCalledWith({ top: OCT_ROW * 100, behavior: "auto" });
      stream().scrollTop = OCT_ROW * 100;
      // November's first week is 4 rows on.
      rerender(new Date(2026, 10, 1));
      expect(scrollTo).toHaveBeenLastCalledWith({ top: (OCT_ROW + 4) * 100, behavior: "smooth" });
      stream().scrollTop = (OCT_ROW + 4) * 100;
      // June 2027 is 31 rows on: past a quarter, so no glide to chase.
      rerender(new Date(2027, 5, 1));
      expect(scrollTo.mock.lastCall![0].behavior).toBe("auto");
    });

    // A held key steps faster than a glide ends. The first step's safety timer must not free the
    // guard under the second: the month the glide is passing would be reported as the user's.
    it("holds the guard until 700 ms after the latest step, not the first", () => {
      const onFocusDate = vi.fn();
      const { rerender } = mountAt(new Date(2026, 9, 1), onFocusDate);
      const s = stream();
      s.scrollTop = OCT_ROW * 100;
      rerender(new Date(2026, 10, 1)); // t = 0
      settle(300);
      rerender(new Date(2026, 11, 1)); // t = 300
      // t = 770: the first step's timer (700) has passed, the second's (1,000) hasn't.
      settle(470);
      s.scrollTop = (OCT_ROW + 30) * 100; // a glide passing through February 2027
      fireEvent.scroll(s);
      settle(20);
      expect(onFocusDate).not.toHaveBeenCalled();
      // t = 1,040: released. The same position now reads as the user's.
      settle(250);
      fireEvent.scroll(s);
      settle(20);
      expect(onFocusDate).toHaveBeenCalledTimes(1);
    });

    it("frees the guard when the scroll ends", () => {
      const onFocusDate = vi.fn();
      const { rerender } = mountAt(new Date(2026, 9, 1), onFocusDate);
      const s = stream();
      s.scrollTop = OCT_ROW * 100;
      rerender(new Date(2026, 10, 1));
      s.scrollTop = (OCT_ROW + 30) * 100;
      fireEvent.scroll(s);
      settle(20);
      expect(onFocusDate).not.toHaveBeenCalled();
      fireEvent(s, new Event("scrollend"));
      fireEvent.scroll(s);
      settle(20);
      expect(onFocusDate).toHaveBeenCalledTimes(1);
    });
  });

  // Ten years out, the same few pixels of row height are months of scroll: the week at the top
  // stays at the top when the rows change height (the strip, a resize, the header wrapping).
  it("keeps the week at the top when the rows change height", () => {
    const callbacks: ResizeObserverCallback[] = [];
    const had = (globalThis as { ResizeObserver?: unknown }).ResizeObserver;
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      constructor(cb: ResizeObserverCallback) {
        callbacks.push(cb);
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    };
    try {
      mountAt(new Date(2026, 9, 1));
      settle(700); // the opening scroll is PM's own, and isn't the user's until it has ended
      const s = stream();
      s.scrollTop = 50 * 100;
      fireEvent.scroll(s);
      // The pane shrinks to 540 px (a strip appeared above it): 90 px weeks.
      act(() => {
        for (const cb of callbacks) {
          cb([{ contentRect: { height: 540 } } as ResizeObserverEntry], {} as ResizeObserver);
        }
      });
      expect(s.dataset.weekH).toBe("90");
      expect(s.scrollTop).toBe(50 * 90);
      // And back: the same week.
      act(() => {
        for (const cb of callbacks) {
          cb([{ contentRect: { height: 600 } } as ResizeObserverEntry], {} as ResizeObserver);
        }
      });
      expect(s.dataset.weekH).toBe("100");
      expect(s.scrollTop).toBe(50 * 100);
    } finally {
      (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = had;
    }
  });

  it("keeps a half-scrolled week as it was", () => {
    const callbacks: ResizeObserverCallback[] = [];
    const had = (globalThis as { ResizeObserver?: unknown }).ResizeObserver;
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      constructor(cb: ResizeObserverCallback) {
        callbacks.push(cb);
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    };
    try {
      mountAt(new Date(2026, 9, 1));
      settle(700);
      const s = stream();
      s.scrollTop = 50 * 100 + 50;
      fireEvent.scroll(s);
      act(() => {
        for (const cb of callbacks) {
          cb([{ contentRect: { height: 540 } } as ResizeObserverEntry], {} as ResizeObserver);
        }
      });
      expect(s.scrollTop).toBe(50.5 * 90);
    } finally {
      (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = had;
    }
  });

  // A glide's own scroll events pass through rows between the start and the target; the week to keep
  // is the target, so a strip appearing mid-glide doesn't stop it between months.
  it("keeps a glide's target week at the top when the rows change height mid-glide", () => {
    const callbacks: ResizeObserverCallback[] = [];
    const had = (globalThis as { ResizeObserver?: unknown }).ResizeObserver;
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      constructor(cb: ResizeObserverCallback) {
        callbacks.push(cb);
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    };
    const kept = Object.getOwnPropertyDescriptor(Element.prototype, "scrollTo");
    Object.defineProperty(Element.prototype, "scrollTo", { configurable: true, value: vi.fn() });
    try {
      const { rerender } = mountAt(new Date(2026, 9, 1));
      settle(700);
      const s = stream();
      s.scrollTop = OCT_ROW * 100;
      rerender(new Date(2026, 10, 1)); // a glide to November's first week, 4 rows on
      s.scrollTop = (OCT_ROW + 2) * 100; // part-way
      fireEvent.scroll(s);
      act(() => {
        for (const cb of callbacks) {
          cb([{ contentRect: { height: 540 } } as ResizeObserverEntry], {} as ResizeObserver);
        }
      });
      expect(s.scrollTop).toBe((OCT_ROW + 4) * 90);
    } finally {
      if (kept) Object.defineProperty(Element.prototype, "scrollTo", kept);
      (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = had;
    }
  });
});
