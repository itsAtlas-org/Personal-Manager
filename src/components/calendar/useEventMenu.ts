// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The right-click menu of whatever the Calendar draws as an event: a synced event, and the three
// overlays (a task, a milestone, a pinboard entry). One handler on the Calendar's root finds the
// event by the `data-piece-of` id every surface already carries (Day and Week cards, Month chips
// and bands, the all-day row, the Agenda, the Runway's events strip, and the Terminal's versions of
// each), so no view is told about the menu. The ghost of an event being made has no menu.
//
// Every item repeats a button the event's details popover (or the overlay's own click) already
// offers, and calls the handler that button calls, so the confirm, the gating and the Undo stay in
// one place. An action the event doesn't offer is left out, as the popover leaves it out, and the
// reason is on "Open details". The menu never deletes: Delete… opens the same question the popover's
// button does. A keyboard user gets the menu from the Menu key or Shift+F10 on a focused event.

import { useRef, type MouseEvent as ReactMouseEvent } from "react";
import {
  copyViaHost,
  openContextMenu,
  tidy,
  wantsAppMenu,
  type MenuItem,
} from "../../kit/lib/contextMenu";
import {
  isMilestoneEvent,
  isPinboardEvent,
  isTaskEvent,
  taskIdOfEvent,
} from "../../kit/lib/calendar-layout";
import { changeTaskStatus } from "../../lib/ipc";
import { openLink } from "../../lib/linkFailure";
import { runMutation } from "../../lib/runMutation";
import type { Calendar, CalendarEvent, Milestone, Task, TaskStatus } from "../../lib/types";
import type { PopoverEditing } from "./parts/CalendarEventPopover";
import { sourceLabel } from "./parts/sourceLabel";
import { statusItems } from "./views/runwayMenu";

/** What the menu reads from the Calendar, so it asks for nothing the view doesn't already hold. */
export interface EventMenuDeps {
  /** Every event drawn, overlays included: the id on the element finds its event here. */
  events: readonly CalendarEvent[];
  calendars: readonly Calendar[];
  milestones: readonly Milestone[];
  tasks: readonly Task[];
  /** What the popover offers for `ev` (its rights, Duplicate's gate, and the handlers). */
  editing: (ev: CalendarEvent) => PopoverEditing;
  /** What a click on a synced event does: opens its details by `anchor`. */
  openDetails: (ev: CalendarEvent, anchor: DOMRect) => void;
  /** What a click on a task chip does. */
  editTask: (task: Task) => void;
  setTaskStatus: (task: Task, next: TaskStatus) => void;
  onOpenProject?: (project: string) => void;
  onOpenPinboard?: () => void;
  /** Quick create is open: a click on any event goes back to its box, so the menu offers only that. */
  quickOpen?: boolean;
}

/** The menu for `ev`, with the name it is announced under; null when there is none. `anchor` is
 *  the element it was asked on, read when an item needs it. */
export function eventMenu(
  ev: CalendarEvent,
  deps: EventMenuDeps,
  anchor: () => DOMRect,
): { label: string; items: MenuItem[] } | null {
  // The box of a new event is open and holds a draft. A click on any event goes back to it (an
  // edit, a delete or a jump to another tab would sit over the draft or take it away), so the menu
  // keeps to that one way back. The keyboard has no mousedown to close the box first.
  if (deps.quickOpen) {
    return {
      label: `Event ${ev.summary}`,
      items: [{ label: "Open details", onSelect: () => deps.openDetails(ev, anchor()) }],
    };
  }
  if (isMilestoneEvent(ev)) {
    const id = Number(ev.id.slice("milestone:".length));
    const m = deps.milestones.find((x) => x.id === id);
    if (!m || !deps.onOpenProject) return null;
    const open = deps.onOpenProject;
    return {
      label: `Milestone ${ev.summary}`,
      items: [{ label: `Open project ${m.project_name}`, onSelect: () => open(m.project_name) }],
    };
  }
  if (isPinboardEvent(ev)) {
    const open = deps.onOpenPinboard;
    if (!open) return null;
    return {
      label: `Pinboard entry ${ev.summary}`,
      items: [{ label: "Open in Pinboard", onSelect: open }],
    };
  }
  if (isTaskEvent(ev)) {
    const id = taskIdOfEvent(ev);
    const t = deps.tasks.find((x) => x.id === id);
    if (!t) return null;
    const project = t.project_name;
    const open = deps.onOpenProject;
    return {
      label: `Task ${t.title}`,
      items: [
        { label: "Edit task…", onSelect: () => deps.editTask(t) },
        ...statusItems(t.status, (next) => deps.setTaskStatus(t, next)),
        ...(project && open
          ? [
              { kind: "separator" as const },
              { label: `Open project ${project}`, onSelect: () => open(project) },
            ]
          : []),
      ],
    };
  }
  const ed = deps.editing(ev);
  const provider = deps.calendars.find((c) => c.id === ev.calendar_id)?.provider;
  const milestone = ev.uid
    ? deps.milestones.find((m) => m.event_uid && m.event_uid === ev.uid)
    : undefined;
  const project = milestone?.project_name;
  const openProject = deps.onOpenProject;
  const link = ev.html_link;
  const call = ev.conference_url;
  return {
    label: `Event ${ev.summary}`,
    // Joined by `tidy`: a group the event doesn't offer leaves no empty gap between separators.
    items: tidy([
      { label: "Open details", onSelect: () => deps.openDetails(ev, anchor()) },
      ...(ed.canEdit ? [{ label: "Edit…", onSelect: ed.onEdit }] : []),
      ...(ed.canDuplicate ? [{ label: "Duplicate", onSelect: ed.onDuplicate }] : []),
      { kind: "separator" },
      ...(link
        ? [
            { label: sourceLabel(provider), onSelect: () => openLink(link) },
            { label: "Copy event link", onSelect: () => copyViaHost(link, "Link copied") },
          ]
        : []),
      ...(call
        ? [
            { label: "Join the call", onSelect: () => openLink(call) },
            { label: "Copy call link", onSelect: () => copyViaHost(call, "Link copied") },
          ]
        : []),
      ...(project && openProject
        ? [{ label: `Open project ${project}`, onSelect: () => openProject(project) }]
        : []),
      { kind: "separator" },
      ...(ed.canDelete
        ? [
            {
              label: ed.removes ? "Remove…" : "Delete…",
              danger: true,
              onSelect: ed.onDelete,
            },
          ]
        : []),
    ]),
  };
}

/** The one `onContextMenu` for the Calendar's root. `deps` is read afresh on every right-click (a
 *  ref), so the handler can sit on the root without being rebuilt for each render's closures. The
 *  status change runs as the Runway's does: a failure is said in the tab's live region. */
export function useEventMenu(
  deps: Omit<EventMenuDeps, "setTaskStatus"> & { announce: (text: string) => void },
): (e: ReactMouseEvent<HTMLElement>) => void {
  const ref = useRef(deps);
  ref.current = deps;
  return (e) => {
    if (!wantsAppMenu(e) || !(e.target instanceof Element)) return;
    const piece = e.target.closest<HTMLElement>("[data-piece-of]");
    // The ghost of an event being made is not an event yet (it may be the piece itself or hold it).
    if (!piece || piece.closest("[data-draft-event]")) return;
    const now = ref.current;
    const ev = now.events.find((x) => x.id === piece.dataset.pieceOf);
    if (!ev) return;
    const menu = eventMenu(
      ev,
      {
        ...now,
        setTaskStatus: (task, next) =>
          void runMutation(
            () => changeTaskStatus(task.id, next),
            (message) => message && now.announce(message),
          ),
      },
      () => piece.getBoundingClientRect(),
    );
    if (menu) openContextMenu(e, menu.label, menu.items);
  };
}
