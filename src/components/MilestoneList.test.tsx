// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The milestone row's LIFECYCLE, which is where it silently corrupted dates. A row is keyed by id, so
// the same instance survives every refetch — and its `persist` compares the local draft against the
// live prop, so a local value that never adopted a refetched one is exactly the case that skips the
// no-op guard and writes the outdated day back over the newer one. The dates a milestone owns are
// also the dates the flags layer and Due-soon read, so a silent overwrite here is not cosmetic.
//
// The other half is the optimistic date commit: a REFUSED write triggers no refetch, so `m.due_date`
// never changes, the adopt effect never re-fires, and without an explicit rollback the field is left
// showing — and primed to re-commit — a date the backend rejected. Both halves are tested, because
// neither closes the finding on its own.
//
// `useTheme` is stubbed the way ConnectorItemRow.test.tsx does it, so the row's Button/Input/Select
// render without the full ThemeProvider (which pulls in IPC).

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Milestone } from "../lib/types";
import { MilestoneList } from "./MilestoneList";
import { ContextMenuHost } from "./ui/ContextMenuHost";
import { closeContextMenu } from "../lib/contextMenu";

vi.mock("../lib/ipc", () => ({
  addMilestone: vi.fn(async () => 1),
  deleteMilestone: vi.fn(async () => {}),
  reorderMilestones: vi.fn(async () => {}),
  setMilestoneEvent: vi.fn(async () => {}),
  setMilestoneStatus: vi.fn(async () => {}),
  updateMilestone: vi.fn(async () => {}),
}));

vi.mock("../kit/theme", async (importOriginal) => ({
  ...(await importOriginal()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    modePref: "system",
    modeSource: "system",
    accent: "mono",
    depth: "standard",
    autoLocation: "",
    teachVisible: true,
    setSystem: () => {},
    setModePref: () => {},
    setAccent: () => {},
    setDepth: () => {},
    setAutoLocation: () => {},
    setTeachVisible: () => {},
  }),
}));

import {
  deleteMilestone,
  reorderMilestones,
  setMilestoneEvent,
  setMilestoneStatus,
  updateMilestone,
} from "../lib/ipc";

const mockUpdate = vi.mocked(updateMilestone);
const mockSetEvent = vi.mocked(setMilestoneEvent);

function milestone(over: Partial<Milestone> = {}): Milestone {
  return {
    id: 1,
    project_name: "Atlas",
    label: "pitch",
    due_date: "2026-08-01",
    event_uid: null,
    calendar_linked: false,
    event_missing: false,
    state: "unmet",
    status: "not_started",
    source_type: null,
    external_id: null,
    sort_order: 0,
    madeBy: null,
    ...over,
  };
}

function renderList(m: Milestone) {
  return render(<MilestoneList project="Atlas" milestones={[m]} onChanged={() => {}} />);
}

/** The editable deadline field (absent while the milestone is calendar-linked). */
function dateInput(): HTMLInputElement {
  return screen.getByLabelText("Milestone deadline") as HTMLInputElement;
}

function labelInput(): HTMLInputElement {
  return screen.getByPlaceholderText("label") as HTMLInputElement;
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(cleanup);

describe("a refetch moving the milestone underneath the row", () => {
  it("shows the new date, not the one it mounted with", () => {
    const { rerender } = renderList(milestone());
    expect(dateInput().value).toBe("01-08-2026");

    // Same id — React keeps the instance, so only the adopt effect can update the field.
    rerender(
      <MilestoneList
        project="Atlas"
        milestones={[milestone({ due_date: "2026-09-01" })]}
        onChanged={() => {}}
      />,
    );
    expect(dateInput().value).toBe("01-09-2026");
  });

  // THE silent overwrite. Blurring the label without touching it must be a no-op; with a stale local
  // date it isn't — `nextDate` differs from the live `curDate`, the guard doesn't fire, and the row
  // writes its mount-time day back over the one the sync just delivered.
  it("does not write its stale date back when the label is merely blurred", async () => {
    const { rerender } = renderList(milestone());
    rerender(
      <MilestoneList
        project="Atlas"
        milestones={[milestone({ due_date: "2026-09-01" })]}
        onChanged={() => {}}
      />,
    );

    fireEvent.focus(labelInput());
    fireEvent.blur(labelInput());
    await waitFor(() => expect(mockUpdate).not.toHaveBeenCalled());
  });

  it("adopts a changed label but keeps text the user is still typing", () => {
    const { rerender } = renderList(milestone());
    rerender(
      <MilestoneList
        project="Atlas"
        milestones={[milestone({ label: "renamed remotely" })]}
        onChanged={() => {}}
      />,
    );
    expect(labelInput().value).toBe("renamed remotely");

    // A refetch that brings back the SAME label must not stamp on an in-flight edit: the dep never
    // changes, so the effect never re-runs.
    fireEvent.change(labelInput(), { target: { value: "mid-typing" } });
    rerender(
      <MilestoneList
        project="Atlas"
        milestones={[milestone({ label: "renamed remotely" })]}
        onChanged={() => {}}
      />,
    );
    expect(labelInput().value).toBe("mid-typing");
  });

  // The end-to-end shape of the real defect: while linked the field isn't rendered at all, so a
  // stale local date is invisible right up until Unlink makes it editable again.
  it("shows the MOVED date after a calendar-linked milestone is unlinked", async () => {
    const linked = milestone({ calendar_linked: true, event_uid: "uid-1" });
    const { rerender } = renderList(linked);
    expect(screen.queryByLabelText("Milestone deadline")).toBeNull();

    // A calendar sync moves the event; a sibling mutation's refetch delivers the new date.
    rerender(
      <MilestoneList
        project="Atlas"
        milestones={[milestone({ ...linked, due_date: "2026-09-01" })]}
        onChanged={() => {}}
      />,
    );
    fireEvent.click(screen.getByTitle(/Unlink from calendar/));
    // Unlinking can't be undone, so the button asks first.
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Unlink" }));
    await waitFor(() => expect(mockSetEvent).toHaveBeenCalledWith(1, null, "2026-09-01"));

    rerender(
      <MilestoneList
        project="Atlas"
        milestones={[milestone({ due_date: "2026-09-01" })]}
        onChanged={() => {}}
      />,
    );
    expect(dateInput().value).toBe("01-09-2026");
  });
});

describe("a date the backend refuses", () => {
  it("rolls the field back and says so", async () => {
    mockUpdate.mockRejectedValueOnce(new Error("that date is in the past"));
    renderList(milestone());

    fireEvent.change(dateInput(), { target: { value: "15-09-2026" } });
    fireEvent.blur(dateInput());

    // The error line is the only thing on screen that explains the revert.
    await screen.findByRole("alert");
    expect(screen.getByRole("alert").textContent).toContain("that date is in the past");
    // No refetch follows a refusal, so `m.due_date` never moves and only the explicit rollback can
    // put the field back.
    await waitFor(() => expect(dateInput().value).toBe("01-08-2026"));
  });
});

describe("an empty list", () => {
  it("says there are none yet when nothing is hidden", () => {
    render(<MilestoneList project="Atlas" milestones={[]} onChanged={() => {}} />);
    expect(screen.getByText(/No milestones yet/)).toBeTruthy();
  });

  it("says how many completed ones are hidden, and how to show them", () => {
    render(
      <MilestoneList project="Atlas" milestones={[]} onChanged={() => {}} hiddenCompleted={3} />,
    );
    expect(screen.queryByText(/No milestones yet/)).toBeNull();
    expect(
      screen.getByText("3 completed milestones are hidden. Tick Completed to show them."),
    ).toBeTruthy();
    // A status, so a screen reader hears it when unticking Completed empties the list.
    expect(screen.getByRole("status").textContent).toMatch(/3 completed milestones are hidden/);
  });

  it("uses the singular for one hidden milestone", () => {
    render(
      <MilestoneList project="Atlas" milestones={[]} onChanged={() => {}} hiddenCompleted={1} />,
    );
    expect(
      screen.getByText("1 completed milestone is hidden. Tick Completed to show it."),
    ).toBeTruthy();
  });
});

describe("who made a milestone (PR C, K5)", () => {
  it("shows a chip with the maker's name and ALWAYS its tier word, and nothing for the user's own", () => {
    const { rerender, container } = renderList(milestone());
    expect(container.textContent).not.toContain("party");
    rerender(
      <MilestoneList
        project="Atlas"
        milestones={[milestone({ madeBy: { name: "Pinboard", tier: "first", signed: true } })]}
        onChanged={() => {}}
      />,
    );
    expect(container.textContent).toContain("Pinboard· first party");
  });

  it("an unsigned mod named Atlas reads as unsigned, in the editable row and the read-only summary", () => {
    const made = { name: "Atlas", tier: "first", signed: false } as const;
    const { container, rerender } = renderList(milestone({ madeBy: made }));
    expect(container.textContent).toContain("Atlas· unsigned");
    expect(container.textContent).not.toContain("first party");
    rerender(
      <MilestoneList
        project="Atlas"
        milestones={[milestone({ madeBy: made })]}
        onChanged={() => {}}
        readOnly
      />,
    );
    expect(container.textContent).toContain("made by Atlas (unsigned)");
  });
});

describe("removing and unlinking ask first", () => {
  const confirm = (name: string) =>
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name }));

  afterEach(() => act(() => closeContextMenu()));

  it("the row's × asks, and removes only on Remove", async () => {
    renderList(milestone());
    fireEvent.click(screen.getByRole("button", { name: "Remove milestone pitch" }));
    expect(deleteMilestone).not.toHaveBeenCalled();
    expect(within(screen.getByRole("dialog")).getByText("Remove this milestone?")).toBeTruthy();
    confirm("Remove");
    await waitFor(() => expect(deleteMilestone).toHaveBeenCalledWith(1));
  });

  it("cancelling the question removes nothing", () => {
    renderList(milestone());
    fireEvent.click(screen.getByRole("button", { name: "Remove milestone pitch" }));
    confirm("Cancel");
    expect(deleteMilestone).not.toHaveBeenCalled();
  });

  it("a right-click inside the open question opens no row menu, and the dialog sits outside a met row", () => {
    render(
      <>
        <MilestoneList
          project="Atlas"
          milestones={[milestone({ state: "met", status: "done" })]}
          onChanged={() => {}}
        />
        <ContextMenuHost />
      </>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Remove milestone pitch" }));
    const dialog = screen.getByRole("dialog");
    fireEvent.contextMenu(within(dialog).getByText("Remove this milestone?"), {
      clientX: 5,
      clientY: 5,
    });
    expect(screen.queryByRole("menu")).toBeNull();
    // Not inside the row, so a met row's opacity and stacking context can't fade or trap it.
    expect(dialog.closest(".opacity-70")).toBeNull();
  });

  it("Unlink asks first and unlinks only on confirm", async () => {
    mockSetEvent.mockClear();
    renderList(milestone({ calendar_linked: true, event_uid: "u" }));
    fireEvent.click(screen.getByTitle(/Unlink from calendar/));
    expect(mockSetEvent).not.toHaveBeenCalled();
    confirm("Unlink");
    await waitFor(() => expect(mockSetEvent).toHaveBeenCalledWith(1, null, "2026-08-01"));
    // The Unlink button is gone once unlinked, so focus is put on the row's progress control.
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByLabelText("Milestone progress")),
    );
  });
});

describe("the milestone row's right-click menu", () => {
  const confirm = (name: string) =>
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name }));
  const open = () => {
    // Its padding, not a field: the label, date and progress controls keep the system menu.
    fireEvent.contextMenu(screen.getByRole("button", { name: "Remove milestone pitch" }), {
      clientX: 5,
      clientY: 5,
    });
  };
  const list = (milestones: Milestone[], onChanged = () => {}) =>
    render(
      <>
        <MilestoneList project="Atlas" milestones={milestones} onChanged={onChanged} />
        <ContextMenuHost />
      </>,
    );
  afterEach(() => act(() => closeContextMenu()));

  it("sets the progress through the row's own write", async () => {
    list([milestone()]);
    open();
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Done" }));
    await waitFor(() => expect(setMilestoneStatus).toHaveBeenCalledWith(1, "done"));
  });

  it("Remove… opens the confirm and removes nothing by itself", async () => {
    list([milestone()]);
    open();
    fireEvent.click(screen.getByRole("menuitem", { name: "Remove…" }));
    expect(deleteMilestone).not.toHaveBeenCalled();
    confirm("Remove");
    await waitFor(() => expect(deleteMilestone).toHaveBeenCalledWith(1));
  });

  it("offers Unlink from calendar… only for a linked milestone", () => {
    const { unmount } = list([milestone()]);
    open();
    expect(screen.queryByRole("menuitem", { name: "Unlink from calendar…" })).toBeNull();
    act(() => closeContextMenu());
    unmount();
    list([milestone({ calendar_linked: true, event_uid: "u" })]);
    open();
    fireEvent.click(screen.getByRole("menuitem", { name: "Unlink from calendar…" }));
    expect(within(screen.getByRole("dialog")).getByText("Unlink from the calendar?")).toBeTruthy();
  });

  it("Move down reorders in manual order", async () => {
    list([milestone({ id: 1 }), milestone({ id: 2, label: "second", sort_order: 1 })]);
    open();
    fireEvent.click(screen.getByRole("menuitem", { name: "Move down" }));
    await waitFor(() => expect(reorderMilestones).toHaveBeenCalled());
  });
});
