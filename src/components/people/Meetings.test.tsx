// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Who did you meet?" on screen (Contacts, slice 7b): the strip's two forms and both answers, the
// dialog's cards (pre-ticks, guests PM doesn't know, someone in Trash, the fold at eight, "Someone
// else…", Save, Skip, the series skip, Skip all and closing after the last card), and the card a
// past event opens, with or without guests. The clock is pinned, expected clocks are built with
// formatClock (CI is 12-hour en-US, the dev box en_GB), and every person is invented.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { formatClock } from "../../lib/format";
import { forgetAllDrafts } from "../../lib/meetingDrafts";
import type { CalendarEvent, OwedMeeting, OwedUnknownGuest, PersonRow } from "../../lib/types";

const ipc = vi.hoisted(() => ({
  answerMeeting: vi.fn(),
  owedMeetings: vi.fn(),
  eventGuests: vi.fn(),
  searchPeople: vi.fn(),
  getPerson: vi.fn(),
  // The theme provider reads and writes its preference through ipc.
  getPref: vi.fn(async () => null),
  setPref: vi.fn(async () => {}),
}));
vi.mock("../../lib/ipc", () => ipc);

import { ThemeProvider } from "../../theme";
import { EventMeetingDialog } from "./EventMeetingDialog";
import { MeetingDialog } from "./MeetingDialog";
import { MeetStrip } from "./MeetStrip";

const meeting = (over: Partial<OwedMeeting> = {}): OwedMeeting => ({
  uid: "evt-1@example.com",
  start: "2026-11-26T09:00:00Z",
  end: "2026-11-26T09:30:00Z",
  title: "Standup",
  all_day: false,
  recurs: false,
  known: [],
  unknown: [],
  ...over,
});

const ANN = { id: 3, display_name: "Ann Able", email: "ann@example.com" };
const BEN = { id: 4, display_name: "Ben Bryce", email: "ben@example.com" };
const NEWT: OwedUnknownGuest = { name: "Newt Nobody", email: "newt@example.com", trashed_id: null };
const TESS: OwedUnknownGuest = { name: "Tess Trashed", email: "tess@example.com", trashed_id: 8 };

const row = (id: number, name: string): PersonRow => ({
  id,
  uid: String(id).padStart(32, "0"),
  display_name: name,
  primary: `${name.split(" ")[0].toLowerCase()}@example.com`,
  company: null,
  labels: [],
  starred: false,
  last_met: null,
  google_state: null,
  trashed_at: null,
  birthday: null,
  leap_day: null,
});

function wrap(node: React.ReactNode) {
  return render(<ThemeProvider>{node}</ThemeProvider>);
}

beforeEach(() => {
  vi.clearAllMocks();
  forgetAllDrafts();
  localStorage.clear();
  vi.useFakeTimers({ toFake: ["Date"] });
  // Pinned by instant, and the strip is given a UTC zone, so no machine zone moves "today".
  vi.setSystemTime(new Date("2026-11-26T12:00:00Z"));
  ipc.answerMeeting.mockResolvedValue({ interaction: 1, created: 0, restored: 0 });
  ipc.owedMeetings.mockResolvedValue([]);
  ipc.eventGuests.mockResolvedValue({ known: [], unknown: [] });
  ipc.searchPeople.mockResolvedValue([]);
  ipc.getPerson.mockResolvedValue({ uid: "0".repeat(31) + "3" });
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("the strip", () => {
  it("asks about several meetings with Review and Not now, and takes no focus", () => {
    const onReview = vi.fn();
    wrap(
      <MeetStrip
        owed={[meeting(), meeting({ uid: "b" }), meeting({ uid: "c" })]}
        onReview={onReview}
      />,
    );
    expect(
      screen.getByText("You had 3 meetings with guests recently. Who did you meet?"),
    ).toBeTruthy();
    // Non-modal and never focused: a stray Enter can't answer it.
    expect(document.activeElement).toBe(document.body);
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    expect(onReview).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: "Not now" })).toBeTruthy();
  });

  it("names the meeting when there is one", () => {
    wrap(<MeetStrip owed={[meeting({ title: "Roadmap chat" })]} onReview={() => {}} />);
    expect(
      screen.getByText("You had a meeting with guests: Roadmap chat. Who did you meet?"),
    ).toBeTruthy();
  });

  it("draws nothing when nothing is owed", () => {
    const { container } = wrap(<MeetStrip owed={[]} onReview={() => {}} />);
    expect(container.textContent).toBe("");
  });

  it("hides on Not now until tomorrow, in the user's zone, and survives a remount", () => {
    const owed = [meeting()];
    const first = wrap(<MeetStrip owed={owed} zone="Etc/UTC" onReview={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    expect(screen.queryByRole("button", { name: "Review" })).toBeNull();
    first.unmount();
    // The same day, a fresh mount (a tab switch, a restart): still hidden.
    wrap(<MeetStrip owed={owed} zone="Etc/UTC" onReview={() => {}} />);
    expect(screen.queryByRole("button", { name: "Review" })).toBeNull();
    cleanup();
    // Tomorrow in that zone: back.
    vi.setSystemTime(new Date("2026-11-27T01:00:00Z"));
    wrap(<MeetStrip owed={owed} zone="Etc/UTC" onReview={() => {}} />);
    expect(screen.getByRole("button", { name: "Review" })).toBeTruthy();
  });

  it("comes back at once when a new meeting becomes owed", () => {
    const owed = [meeting()];
    const { rerender } = wrap(<MeetStrip owed={owed} zone="Etc/UTC" onReview={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    expect(screen.queryByRole("button", { name: "Review" })).toBeNull();
    rerender(
      <ThemeProvider>
        <MeetStrip
          owed={[...owed, meeting({ uid: "evt-2@example.com" })]}
          zone="Etc/UTC"
          onReview={() => {}}
        />
      </ThemeProvider>,
    );
    expect(
      screen.getByText("You had 2 meetings with guests recently. Who did you meet?"),
    ).toBeTruthy();
  });
});

describe("the meeting dialog's cards", () => {
  const dialog = (
    meetings: OwedMeeting[],
    props: Partial<React.ComponentProps<typeof MeetingDialog>> = {},
  ) => {
    const onClose = vi.fn();
    wrap(<MeetingDialog meetings={meetings} onClose={onClose} {...props} />);
    return onClose;
  };
  const card = (title: string) => screen.getByRole("region", { name: `Meeting ${title}` });

  it("shows the title and the date and time through formatClock, newest first", () => {
    dialog([
      meeting({ title: "Newer", start: "2026-11-26T09:00:00Z", end: "2026-11-26T09:30:00Z" }),
      meeting({ uid: "b", title: "Older", start: "2026-11-25T14:00:00Z", end: null }),
    ]);
    const titles = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
    expect(titles).toEqual(["Newer", "Older"]);
    expect(
      within(card("Newer")).getByText(
        new RegExp(
          `${formatClock(new Date("2026-11-26T09:00:00Z"))}.*${formatClock(new Date("2026-11-26T09:30:00Z"))}`,
        ),
      ),
    ).toBeTruthy();
  });

  it("pre-ticks guests in People and leaves guests PM doesn't know unticked", () => {
    dialog([meeting({ known: [ANN], unknown: [NEWT] })]);
    expect(
      (screen.getByRole("checkbox", { name: "Ann Able was there" }) as HTMLInputElement).checked,
    ).toBe(true);
    expect(screen.getByText("Not in People yet")).toBeTruthy();
    expect(
      (
        screen.getByRole("checkbox", {
          name: "Newt Nobody (newt@example.com) was there",
        }) as HTMLInputElement
      ).checked,
    ).toBe(false);
  });

  it("marks a guest who is only in Trash, and ticking sends their id so they come back", async () => {
    dialog([meeting({ unknown: [TESS] })]);
    expect(screen.getByText("In Trash")).toBeTruthy();
    fireEvent.click(
      screen.getByRole("checkbox", { name: "Tess Trashed (tess@example.com) was there" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(ipc.answerMeeting).toHaveBeenCalled());
    expect(ipc.answerMeeting.mock.calls[0][2]).toEqual([{ id: 8 }]);
  });

  it("folds more than eight unknown guests behind 'Show {n} more guests'", () => {
    const ten = Array.from({ length: 10 }, (_, i) => ({
      name: `Guest ${i}`,
      email: `guest${i}@example.com`,
      trashed_id: null,
    }));
    dialog([meeting({ unknown: ten })]);
    const boxes = () => within(card("Standup")).getAllByRole("checkbox");
    expect(boxes()).toHaveLength(8);
    fireEvent.click(screen.getByRole("button", { name: "Show 2 more guests" }));
    expect(boxes()).toHaveLength(10);
    expect(screen.queryByRole("button", { name: /more guests/ })).toBeNull();
  });

  it("saves who is ticked, new guests as new people, with the note", async () => {
    dialog([meeting({ known: [ANN], unknown: [NEWT] })]);
    fireEvent.click(
      screen.getByRole("checkbox", { name: "Newt Nobody (newt@example.com) was there" }),
    );
    fireEvent.change(screen.getByLabelText("Note (optional)"), {
      target: { value: " Good chat " },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(ipc.answerMeeting).toHaveBeenCalledOnce());
    expect(ipc.answerMeeting).toHaveBeenCalledWith(
      "evt-1@example.com",
      "2026-11-26T09:00:00Z",
      [{ id: 3 }, { new: { name: "Newt Nobody", email: "newt@example.com" } }],
      "logged",
      "Good chat",
      "Standup",
    );
  });

  it("cannot save with nobody ticked", () => {
    dialog([meeting({ known: [ANN] })]);
    fireEvent.click(screen.getByRole("checkbox", { name: "Ann Able was there" }));
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText("Tick at least one person to save.")).toBeTruthy();
  });

  it("adds someone else from a person picker over the search", async () => {
    ipc.searchPeople.mockResolvedValue([row(5, "Cal Placeholder"), row(3, "Ann Able")]);
    dialog([meeting({ known: [ANN] })]);
    fireEvent.click(screen.getByRole("button", { name: "Someone else…" }));
    fireEvent.change(screen.getByLabelText("Someone else who was at Standup"), {
      target: { value: "cal" },
    });
    // People already on the card are not offered twice.
    const pick = await screen.findByRole("button", { name: /Cal Placeholder/ });
    expect(screen.queryByRole("button", { name: /Ann Able/ })).toBeNull();
    fireEvent.click(pick);
    expect(within(card("Standup")).getByText("Cal Placeholder")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(ipc.answerMeeting).toHaveBeenCalled());
    expect(ipc.answerMeeting.mock.calls[0][2]).toEqual([{ id: 3 }, { id: 5 }]);
  });

  it("Skip writes a skip with nobody, and offers the series only for a repeating meeting", async () => {
    dialog([meeting({ known: [ANN] })]);
    expect(screen.queryByRole("button", { name: "Don't ask about this series" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Skip" }));
    await waitFor(() => expect(ipc.answerMeeting).toHaveBeenCalled());
    expect(ipc.answerMeeting).toHaveBeenCalledWith(
      "evt-1@example.com",
      "2026-11-26T09:00:00Z",
      [],
      "skipped",
      null,
      "Standup",
    );
  });

  it("Don't ask about this series writes the series answer", async () => {
    dialog([meeting({ recurs: true })]);
    fireEvent.click(screen.getByRole("button", { name: "Don't ask about this series" }));
    await waitFor(() => expect(ipc.answerMeeting).toHaveBeenCalled());
    expect(ipc.answerMeeting.mock.calls[0][3]).toBe("series_skipped");
  });

  it("closes only after the last card is answered", async () => {
    const onAnswered = vi.fn();
    const onClose = dialog(
      [meeting({ known: [ANN] }), meeting({ uid: "b", title: "Second", known: [ANN] })],
      { onAnswered },
    );
    fireEvent.click(within(card("Standup")).getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(screen.queryByRole("region", { name: "Meeting Standup" })).toBeNull(),
    );
    expect(onClose).not.toHaveBeenCalled();
    expect(onAnswered).toHaveBeenCalledOnce();
    fireEvent.click(within(card("Second")).getByRole("button", { name: "Skip" }));
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
  });

  it("Skip all skips every card and closes", async () => {
    const onClose = dialog([
      meeting({ title: "One" }),
      meeting({ uid: "b", title: "Two" }),
      meeting({ uid: "c", title: "Three", recurs: true }),
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Skip all" }));
    await waitFor(() => expect(onClose).toHaveBeenCalledOnce());
    expect(ipc.answerMeeting.mock.calls.map((c) => [c[0], c[3]])).toEqual([
      ["evt-1@example.com", "skipped"],
      ["b", "skipped"],
      ["c", "skipped"],
    ]);
  });

  it("holds Skip all and Close while a card's own answer is in flight", async () => {
    let finish: () => void = () => {};
    ipc.answerMeeting.mockImplementationOnce(() => new Promise<void>((r) => (finish = r)));
    dialog([meeting({ title: "One", known: [ANN] }), meeting({ uid: "b", title: "Two" })]);
    fireEvent.click(within(card("One")).getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect((screen.getByRole("button", { name: "Skip all" }) as HTMLButtonElement).disabled).toBe(
        true,
      ),
    );
    expect((screen.getByRole("button", { name: "Close" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
    finish();
    await waitFor(() =>
      expect((screen.getByRole("button", { name: "Close" }) as HTMLButtonElement).disabled).toBe(
        false,
      ),
    );
  });

  it("has no Skip all for a single card, and keeps a card whose answer failed", async () => {
    ipc.answerMeeting.mockRejectedValueOnce("Pick at least one person, or skip the meeting.");
    const onClose = dialog([meeting({ known: [ANN] })]);
    expect(screen.queryByRole("button", { name: "Skip all" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("Pick at least one person, or skip the meeting.")).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    expect(card("Standup")).toBeTruthy();
  });

  it("opens a known guest's record by their people uid when it can", async () => {
    const onOpenPerson = vi.fn();
    dialog([meeting({ known: [ANN] })], { onOpenPerson });
    fireEvent.click(screen.getByRole("button", { name: "Open Ann Able" }));
    await waitFor(() => expect(onOpenPerson).toHaveBeenCalledWith("0".repeat(31) + "3"));
  });
});

describe("what a card keeps while its dialog is closed", () => {
  // Clicking a guest's name opens their record and closes the dialog: the owner unmounts it and
  // mounts it again, with the same meetings, when the dialog is reopened.
  const open = (meetings: OwedMeeting[], onOpenPerson?: () => void) =>
    wrap(<MeetingDialog meetings={meetings} onClose={() => {}} onOpenPerson={onOpenPerson} />);
  const tick = (name: string) =>
    (screen.getByRole("checkbox", { name }) as HTMLInputElement).checked;
  const note = () =>
    (screen.getByRole("textbox", { name: "Note (optional)" }) as HTMLTextAreaElement).value;
  const m = () => meeting({ known: [ANN, BEN], unknown: [NEWT] });

  it("keeps what was ticked, written and picked when another guest's name is clicked", async () => {
    ipc.searchPeople.mockResolvedValue([row(9, "Cleo Cole")]);
    let view = open([m()], () => view.unmount());
    // Ann un-ticked, the unknown guest ticked, a note written, someone else picked.
    fireEvent.click(screen.getByRole("checkbox", { name: "Ann Able was there" }));
    fireEvent.click(
      screen.getByRole("checkbox", { name: "Newt Nobody (newt@example.com) was there" }),
    );
    fireEvent.change(screen.getByRole("textbox", { name: "Note (optional)" }), {
      target: { value: "Talked about the trip" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Someone else…" }));
    fireEvent.change(screen.getByLabelText("Someone else who was at Standup"), {
      target: { value: "cleo" },
    });
    fireEvent.click(await screen.findByRole("button", { name: /Cleo Cole/ }));
    // Clicking Ben's name opens his record: the dialog goes.
    fireEvent.click(screen.getByRole("button", { name: "Open Ben Bryce" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    // Opened again, it is as it was left.
    view = open([m()]);
    expect(tick("Ann Able was there")).toBe(false);
    expect(tick("Ben Bryce was there")).toBe(true);
    expect(tick("Newt Nobody (newt@example.com) was there")).toBe(true);
    expect(note()).toBe("Talked about the trip");
    expect(screen.getByRole("list", { name: "Other people" }).textContent).toContain("Cleo Cole");
    // And it saves what is shown.
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(ipc.answerMeeting).toHaveBeenCalledOnce());
    expect(ipc.answerMeeting.mock.calls[0][2]).toEqual([
      { id: 4 },
      { new: { name: "Newt Nobody", email: "newt@example.com" } },
      { id: 9 },
    ]);
    expect(ipc.answerMeeting.mock.calls[0][4]).toBe("Talked about the trip");
  });

  it("starts clean once the card was answered", async () => {
    const view = open([m()]);
    fireEvent.click(screen.getByRole("checkbox", { name: "Ann Able was there" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Note (optional)" }), {
      target: { value: "Gone after saving" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(ipc.answerMeeting).toHaveBeenCalledOnce());
    view.unmount();
    // The same meeting again (a card an event's "Log who you met" builds, say).
    open([m()]);
    expect(tick("Ann Able was there")).toBe(true);
    expect(note()).toBe("");
  });

  it("starts clean after Skip, and after Skip all", async () => {
    let view = open([m()]);
    fireEvent.click(screen.getByRole("checkbox", { name: "Ann Able was there" }));
    fireEvent.click(screen.getByRole("button", { name: "Skip" }));
    await waitFor(() => expect(ipc.answerMeeting).toHaveBeenCalledOnce());
    view.unmount();
    view = open([m()]);
    expect(tick("Ann Able was there")).toBe(true);
    view.unmount();

    ipc.answerMeeting.mockClear();
    view = open([m(), meeting({ uid: "b", title: "Two" })]);
    fireEvent.click(screen.getByRole("checkbox", { name: "Ann Able was there" }));
    fireEvent.click(screen.getByRole("button", { name: "Skip all" }));
    await waitFor(() => expect(ipc.answerMeeting).toHaveBeenCalledTimes(2));
    view.unmount();
    open([m()]);
    expect(tick("Ann Able was there")).toBe(true);
  });

  it("keeps a card's input apart from another meeting's", () => {
    const view = open([m()]);
    fireEvent.click(screen.getByRole("checkbox", { name: "Ann Able was there" }));
    view.unmount();
    open([meeting({ uid: "other", known: [ANN] })]);
    expect(tick("Ann Able was there")).toBe(true);
  });

  it("ticks a guest PM has come to know since the card was left", () => {
    const view = open([meeting({ known: [ANN], unknown: [NEWT] })]);
    fireEvent.click(screen.getByRole("checkbox", { name: "Ann Able was there" }));
    view.unmount();
    // Newt was added to People meanwhile (known, so ticked as a fresh card has them); Ann stays off.
    open([
      meeting({ known: [ANN, { id: 7, display_name: "Newt Nobody", email: "newt@example.com" }] }),
    ]);
    expect(tick("Ann Able was there")).toBe(false);
    expect(tick("Newt Nobody was there")).toBe(true);
  });

  it("ignores Escape while an answer is in flight, as the Close button does", async () => {
    let finish: () => void = () => {};
    ipc.answerMeeting.mockImplementationOnce(() => new Promise<void>((r) => (finish = r)));
    const onClose = vi.fn();
    wrap(
      <MeetingDialog
        meetings={[meeting({ known: [ANN] }), meeting({ uid: "b", title: "Two" })]}
        onClose={onClose}
      />,
    );
    fireEvent.click(
      within(screen.getByRole("region", { name: "Meeting Standup" })).getByRole("button", {
        name: "Save",
      }),
    );
    await waitFor(() =>
      expect((screen.getByRole("button", { name: "Close" }) as HTMLButtonElement).disabled).toBe(
        true,
      ),
    );
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
    finish();
    await waitFor(() =>
      expect((screen.getByRole("button", { name: "Close" }) as HTMLButtonElement).disabled).toBe(
        false,
      ),
    );
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledOnce();
  });
});

describe("Log who you met on a past event", () => {
  const event = (over: Partial<CalendarEvent> = {}): CalendarEvent =>
    ({
      id: "e1",
      calendar_id: "cal",
      summary: "Coffee",
      description: null,
      location: null,
      start: "2026-11-25T09:00:00Z",
      end: "2026-11-25T10:00:00Z",
      all_day: false,
      html_link: null,
      uid: "evt-9@example.com",
      attendees: [
        {
          name: "Ann A.",
          email: "ann@example.com",
          response: "accepted",
          optional: false,
          organizer: false,
          self: false,
        },
        {
          name: "Me",
          email: "me@example.com",
          response: "accepted",
          optional: false,
          organizer: false,
          self: true,
        },
      ],
      ...over,
    }) as CalendarEvent;

  it("uses the owed list's card when the occurrence is on it, without asking for guests", async () => {
    ipc.owedMeetings.mockResolvedValue([
      meeting({
        uid: "evt-9@example.com",
        start: "2026-11-25T09:00:00Z",
        title: "Coffee",
        known: [ANN],
      }),
    ]);
    wrap(<EventMeetingDialog event={event()} onClose={() => {}} />);
    expect(await screen.findByRole("region", { name: "Meeting Coffee" })).toBeTruthy();
    expect(ipc.eventGuests).not.toHaveBeenCalled();
    expect(screen.getByRole("heading", { name: "Log who you met" })).toBeTruthy();
  });

  it("builds the card from the event's own guests when PM wouldn't have asked", async () => {
    ipc.eventGuests.mockResolvedValue({ known: [ANN], unknown: [NEWT] });
    const ev = event();
    wrap(<EventMeetingDialog event={ev} onClose={() => {}} />);
    await screen.findByRole("region", { name: "Meeting Coffee" });
    expect(ipc.eventGuests).toHaveBeenCalledWith(ev.attendees);
    expect(
      (screen.getByRole("checkbox", { name: "Ann Able was there" }) as HTMLInputElement).checked,
    ).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(ipc.answerMeeting).toHaveBeenCalled());
    expect(ipc.answerMeeting).toHaveBeenCalledWith(
      "evt-9@example.com",
      "2026-11-25T09:00:00Z",
      [{ id: 3 }],
      "logged",
      null,
      "Coffee",
    );
  });

  it("opens for an event with no guests, where Someone else… is the only way to name anyone", async () => {
    wrap(<EventMeetingDialog event={event({ attendees: [] })} onClose={() => {}} />);
    await screen.findByRole("region", { name: "Meeting Coffee" });
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByRole("button", { name: "Someone else…" })).toBeTruthy();
    expect(screen.queryByText("Not in People yet")).toBeNull();
  });

  it("can only log an event with no iCal id: no Skip, and no answer row is promised", async () => {
    ipc.eventGuests.mockResolvedValue({ known: [ANN], unknown: [] });
    wrap(<EventMeetingDialog event={event({ uid: null })} onClose={() => {}} />);
    await screen.findByRole("region", { name: "Meeting Coffee" });
    expect(screen.queryByRole("button", { name: "Skip" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(ipc.answerMeeting).toHaveBeenCalled());
    expect(ipc.answerMeeting.mock.calls[0].slice(0, 2)).toEqual(["", "2026-11-25T09:00:00Z"]);
  });

  it("says so when the guests can't be read, and tries again", async () => {
    ipc.eventGuests.mockRejectedValueOnce("nope").mockResolvedValue({ known: [], unknown: [] });
    wrap(<EventMeetingDialog event={event()} onClose={() => {}} />);
    expect(await screen.findByText("nope")).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    });
    expect(await screen.findByRole("region", { name: "Meeting Coffee" })).toBeTruthy();
  });
});
