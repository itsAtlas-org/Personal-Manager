// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The right-click menus of the Runway's cards (both boards, and the parked shelf). Each item repeats
// something the card already offers as a visible control, so the menu is a shortcut and never the
// only way: a mouse user gets it from a right-click, a keyboard user from the Menu key or Shift+F10
// on any control inside the card. Every item calls the handler the visible control calls, so the
// confirm, the gating and the spoken result stay in one place; the menu itself never deletes, it
// opens the same "Delete this task?" question the editor asks.
//
// The Move… choices come from `moveChoices`, the list the Move… popover draws, so the two can't
// drift. They are flat items here, not a submenu: the menu has no submenu, and a card's choices fit
// one screen.

import {
  linkedEvent,
  type RunwayCard,
  type RunwayColumn,
  type TaskPlan,
} from "../../../lib/runway";
import { statusItems } from "../../../kit/tasks/taskMenu";
import type { MouseEvent as ReactMouseEvent } from "react";
import { openContextMenu, wantsAppMenu, type MenuItem } from "../../../kit/lib/contextMenu";
import type { CalendarEvent, Milestone, Task, TaskStatus } from "../../../lib/types";
import { moveChoices, runMoveChoice, type MoveChoice } from "./runwayLogic";

/** What a card's menu needs from its board: the same handlers the card's own buttons use. */
export interface RunwayMenuCtx {
  today: Date;
  columns: RunwayColumn[];
  events: CalendarEvent[];
  cycle: (task: Task, next: TaskStatus) => void;
  replan: (tasks: readonly Task[], planFor: (t: Task) => TaskPlan | null, where: string) => void;
  reorder: (order: number[], said: string) => void;
  edit: (task: Task) => void;
  /** Ask "Delete this task?"; the dialog does the delete. */
  askDelete: (task: Task) => void;
  onOpenProject?: (project: string) => void;
  onEventClick?: (ev: CalendarEvent, anchor: DOMRect) => void;
}

/** `choices` as flat menu items, a separator between groups: "Move up", "Move down", then "Move to
 *  <column>" for each column but the one the card is in, then the other lane and parking. Up and
 *  down are disabled at the ends of the lane (position state, which the menu may show). */
export function moveItems(
  choices: readonly MoveChoice[],
  onReplan: (planFor: (t: Task) => TaskPlan | null, where: string) => void,
  onReorder: (order: number[], said: string) => void,
): MenuItem[] {
  const out: MenuItem[] = [];
  let part: MoveChoice["part"] | null = null;
  for (const c of choices) {
    // The column it is already in isn't somewhere to move to.
    if (c.current) continue;
    if (part !== null && c.part !== part) out.push({ kind: "separator" });
    part = c.part;
    out.push({
      label: c.part === "column" ? `Move to ${c.label}` : c.label,
      disabled: c.act === "reorder" ? c.disabled : false,
      onSelect: () => runMoveChoice(c, onReplan, onReorder),
    });
  }
  return out;
}

/** A task's card (or a stack's open member, or a Terminal row). `laneCards` is the lane it is drawn
 *  in, for Move up and down; a member of an open stack has none, as its Move… has none. `anchor` is
 *  where the linked event's details open from: the card. */
export function taskCardItems(
  t: Task,
  column: RunwayColumn | null,
  laneCards: readonly RunwayCard[] | undefined,
  ctx: RunwayMenuCtx,
  anchor: () => DOMRect,
): MenuItem[] {
  const ev = linkedEvent(
    ctx.events,
    t.event_uid,
    ctx.today,
    t.deadline_from === "event" ? t.effective_deadline : null,
  );
  const choices = moveChoices({
    tasks: [t],
    columns: ctx.columns,
    here: t.parked ? null : (column?.id ?? null),
    laneCards,
    canReorder: true,
  });
  const project = t.project_name;
  const onEvent = ctx.onEventClick;
  return [
    { label: "Edit task…", onSelect: () => ctx.edit(t) },
    ...statusItems(t.status, (next) => ctx.cycle(t, next)),
    { kind: "separator" },
    ...moveItems(choices, (planFor, where) => ctx.replan([t], planFor, where), ctx.reorder),
    { kind: "separator" },
    ...(project && ctx.onOpenProject
      ? [{ label: `Open project ${project}`, onSelect: () => ctx.onOpenProject?.(project) }]
      : []),
    ...(ev && onEvent
      ? [{ label: `Open event ${ev.summary}`, onSelect: () => onEvent(ev, anchor()) }]
      : []),
    { kind: "separator" },
    { label: "Delete task…", danger: true, onSelect: () => ctx.askDelete(t) },
  ];
}

/** A stack's card: open or close it, then move the whole stack. Its tasks are edited, ticked and
 *  deleted one at a time, from their own cards once it is open. */
export function stackCardItems(
  tasks: readonly Task[],
  column: RunwayColumn | null,
  laneCards: readonly RunwayCard[],
  ctx: RunwayMenuCtx,
  open: boolean,
  setOpen: (open: boolean) => void,
): MenuItem[] {
  const choices = moveChoices({
    tasks,
    columns: ctx.columns,
    here: column?.id ?? null,
    laneCards,
    canReorder: true,
  });
  return [
    { label: open ? "Close stack" : "Open stack", onSelect: () => setOpen(!open) },
    { kind: "separator" },
    ...moveItems(choices, (planFor, where) => ctx.replan(tasks, planFor, where), ctx.reorder),
  ];
}

/** A milestone's card: its one visible control opens the project, so the menu has that alone. */
export function milestoneCardItems(m: Milestone, ctx: RunwayMenuCtx): MenuItem[] {
  if (!ctx.onOpenProject) return [];
  return [
    {
      label: `Open project ${m.project_name}`,
      onSelect: () => ctx.onOpenProject?.(m.project_name),
    },
  ];
}

/** The `onContextMenu` of a card: opens `build`'s items under the name `label`, unless something
 *  else took the click (a nested card) or Shift asks for the system menu. The anchor is the card
 *  itself, read when an item needs it, not the event, which React has let go of by then. */
export function onCardMenu(label: string, build: (anchor: () => DOMRect) => MenuItem[]) {
  return (e: ReactMouseEvent<HTMLElement>) => {
    if (!wantsAppMenu(e)) return;
    const card = e.currentTarget;
    openContextMenu(
      e,
      label,
      build(() => card.getBoundingClientRect()),
    );
  };
}
