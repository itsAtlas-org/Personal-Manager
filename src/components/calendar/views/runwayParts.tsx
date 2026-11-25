// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The pieces both Runways draw alike (PA-2): the buttons the Calendar header shows, a column's
// read-only events strip, the Move… menu (the keyboard's way to do what a drag does, and the
// Terminal's only way) and the dialog that adds, edits or deletes a named window. The hooks and words they share are in
// `runwayLogic.ts`.

import { useId, useState } from "react";
import { createPortal } from "react-dom";
import type { RunwayWindow } from "../../../lib/calendarPrefs";
import { dayKey, isEventPast, PAST_EVENT_CLASS } from "../../../lib/calendar-layout";
import {
  type RunwayCard,
  type RunwayColumn,
  type RunwayEventStrip,
  type TaskPlan,
} from "../../../lib/runway";
import type { CalendarEvent, Task } from "../../../lib/types";
import { DateField } from "../../DateField";
import { DeleteTaskDialog } from "../../tasks/DeleteTaskDialog";
import { Button, Callout, ConfirmDialog, Dialog, Input, Popover, cn } from "../../ui";
import { moveChoices, newWindowId, runMoveChoice, stripWhen, type MoveChoice } from "./runwayLogic";

/** A column's events, read-only, at its head (R1): each opens its details (a pinboard entry opens
 *  the Pinboard). Nothing is drawn for a column with none. Later's says how many more the List
 *  has. */
export function EventsStrip({
  strip,
  column,
  now,
  colorOf,
  onEventClick,
  onShowList,
  terminal = false,
}: {
  strip: RunwayEventStrip | undefined;
  column: RunwayColumn;
  now: Date;
  colorOf?: (calendarId: string) => string;
  onEventClick?: (ev: CalendarEvent, anchor: DOMRect) => void;
  onShowList?: () => void;
  terminal?: boolean;
}) {
  if (!strip || (strip.events.length === 0 && strip.more === 0)) return null;
  const moreWords = `+${strip.more} more in List`;
  return (
    <div
      data-runway-events=""
      aria-label={`Events, ${column.label}`}
      role="group"
      className={cn(
        "flex flex-col gap-0.5",
        terminal ? "pl-2" : "border-b border-rule px-2 pb-1.5 pt-1",
      )}
    >
      <p
        className={cn(
          "text-ink4",
          terminal ? "" : "font-mono text-[0.625rem] uppercase tracking-wide",
        )}
      >
        {terminal ? "events" : "Events"}
      </p>
      <ul className={cn("flex flex-col", terminal ? "pl-3" : "gap-0.5")}>
        {strip.events.map((ev) => {
          const when = stripWhen(ev, column);
          const color = colorOf?.(ev.calendar_id);
          return (
            <li key={ev.id} className="min-w-0">
              <button
                type="button"
                title={ev.summary}
                data-piece-of={ev.id}
                onClick={(e) => onEventClick?.(ev, e.currentTarget.getBoundingClientRect())}
                className={cn(
                  "flex w-full min-w-0 items-baseline gap-1.5 text-left text-xs text-ink2 hover:text-ink",
                  isEventPast(ev, now) && PAST_EVENT_CLASS,
                )}
              >
                {terminal ? (
                  <span aria-hidden className="text-ink4">
                    ·
                  </span>
                ) : (
                  <span
                    aria-hidden
                    className="inline-block h-2 w-2 shrink-0 self-center rounded-full bg-ink4"
                    style={color ? { background: color } : undefined}
                  />
                )}
                <span className="shrink-0 font-mono text-[0.6875rem] text-ink3">{when}</span>
                <span className="min-w-0 truncate">{ev.summary}</span>
              </button>
            </li>
          );
        })}
      </ul>
      {strip.more > 0 &&
        (onShowList ? (
          <button
            type="button"
            onClick={onShowList}
            className="self-start font-mono text-[0.6875rem] text-ink3 hover:text-ink"
          >
            {moreWords}
          </button>
        ) : (
          <p className="font-mono text-[0.6875rem] text-ink4">{moreWords}</p>
        ))}
    </div>
  );
}

/** The Runway's own two buttons: the events switch and + Window. They change and extend the board,
 *  so the board keeps them (and the window dialog they open), though they are drawn in the header.
 *  Both are `bar` size, the height of the header's switches, so the row reads as one. */
export function RunwayTools({
  showEvents,
  onToggleEvents,
  onAddWindow,
}: {
  /** Whether the columns show their events strips, and the switch for it. */
  showEvents: boolean;
  onToggleEvents: () => void;
  onAddWindow: () => void;
}) {
  return (
    <div data-help="calendar-runway" className="flex items-center gap-1">
      <Button
        variant="tertiary"
        size="bar"
        onClick={onToggleEvents}
        title={
          showEvents
            ? "Hide the events and pinboard entries at the top of each column"
            : "Show each day's events and pinboard entries at the top of its column"
        }
      >
        {showEvents ? "Hide events" : "Show events"}
      </Button>
      <Button variant="tertiary" size="bar" onClick={onAddWindow} title="Name a stretch of days">
        + Window
      </Button>
    </div>
  );
}

/** The Runway's buttons, drawn into the slot the Calendar header holds beside "From today".
 *  Portaled, not lifted: the board stays the one owner of the dialogs the buttons open and of the
 *  events switch's effect, and every control exists once, in the header. CalendarView is the only
 *  mount of either board and sets the slot in the commit the header mounts, before paint, so there
 *  is no inline fallback to keep. Nothing is drawn until the slot is there. */
export function RunwayChrome({
  chrome,
  showEvents,
  onToggleEvents,
  onAddWindow,
}: {
  chrome: HTMLElement | null;
  showEvents: boolean;
  onToggleEvents: () => void;
  onAddWindow: () => void;
}) {
  if (!chrome) return null;
  return createPortal(
    <RunwayTools
      showEvents={showEvents}
      onToggleEvents={onToggleEvents}
      onAddWindow={onAddWindow}
    />,
    chrome,
  );
}

/** The Move… popover: up or down within its lane, then the columns a task can go to, then the other
 *  lane, then parking. For a stack, it moves all of its tasks. Up and down are the keyboard's (and
 *  the Terminal's) way to reorder, as a drag within a column is the pointer's. What it lists is
 *  `moveChoices`, which the card's right-click menu draws too. */
export function MoveMenu({
  name,
  tasks,
  columns,
  here,
  onReplan,
  laneCards,
  onReorder,
  terminal = false,
}: {
  /** What's being moved, for the trigger's name ("Move Draft the deck"). */
  name: string;
  tasks: Task[];
  columns: RunwayColumn[];
  /** The column the card is in (null when parked). */
  here: string | null;
  onReplan: (planFor: (t: Task) => TaskPlan | null, where: string) => void;
  /** The cards of the lane (or the parked shelf) it's in, as drawn: offers Move up / Move down. */
  laneCards?: readonly RunwayCard[];
  /** Put the lane in a new order, and say so. */
  onReorder?: (order: number[], said: string) => void;
  terminal?: boolean;
}) {
  const choices = moveChoices({
    tasks,
    columns,
    here,
    laneCards,
    canReorder: onReorder !== undefined,
  });
  if (choices.length === 0) return null;
  const item =
    "block w-full rounded-[var(--radius-sm)] px-2 py-1 text-left text-xs text-ink2 hover:bg-surface hover:text-ink disabled:cursor-default disabled:text-ink4 disabled:hover:bg-transparent";
  return (
    <Popover
      escapeClipping
      align="right"
      ariaLabel={`Move ${name}`}
      trigger={({ open, toggle }) => (
        <Button
          variant="tertiary"
          size="xs"
          aria-expanded={open}
          aria-haspopup="true"
          aria-label={`Move ${name}`}
          title="Move to another day, lane or the parked shelf"
          onClick={toggle}
          className={terminal ? "font-mono" : undefined}
        >
          Move…
        </Button>
      )}
    >
      {({ close }) => {
        const row = (c: MoveChoice) => (
          <button
            key={c.key}
            type="button"
            className={item}
            disabled={c.disabled}
            aria-current={c.current ? "true" : undefined}
            onClick={() => {
              close();
              runMoveChoice(c, onReplan, onReorder);
            }}
          >
            {c.label}
            {c.sub && <span className="ml-1 text-ink4">{c.sub}</span>}
          </button>
        );
        const steps = choices.filter((c) => c.part === "step");
        return (
          <div className={cn("flex flex-col", terminal && "font-mono")}>
            {steps.length > 0 && (
              <>
                {steps.map(row)}
                <div className="my-1 border-t border-rule" />
              </>
            )}
            <p className="px-2 pb-1 pt-0.5 text-[0.6875rem] text-ink4">Move to</p>
            {choices.filter((c) => c.part === "column").map(row)}
            <div className="my-1 border-t border-rule" />
            {choices.filter((c) => c.part === "other").map(row)}
          </div>
        );
      }}
    </Popover>
  );
}

/** "Delete this task?" for a card's right-click menu: the editor's own question, asked from the
 *  board. `task` is the one being asked about (null: closed); `remove` does the delete, once the
 *  answer is yes, and says whether it went (a failure shows in the board's own error line). */
export function DeleteTaskAsk({
  task,
  remove,
  onClose,
}: {
  task: Task | null;
  remove: (task: Task) => Promise<boolean>;
  onClose: () => void;
}) {
  const [busy, setBusy] = useState(false);
  return (
    <DeleteTaskDialog
      open={task !== null}
      title={task?.title}
      busy={busy}
      onConfirm={async () => {
        if (!task) return;
        setBusy(true);
        await remove(task);
        setBusy(false);
        onClose();
      }}
      onClose={busy ? () => {} : onClose}
    />
  );
}

/** Add, rename, re-date or delete a named window. `window` absent adds one. */
export function WindowDialog({
  open,
  window: w,
  today,
  windows,
  onWindowsChange,
  onClose,
}: {
  open: boolean;
  window?: RunwayWindow | null;
  today: Date;
  windows: RunwayWindow[];
  onWindowsChange: (windows: RunwayWindow[]) => void;
  onClose: () => void;
}) {
  if (!open) return null;
  return (
    <WindowDialogBody
      key={w?.id ?? "new"}
      window={w ?? null}
      today={today}
      windows={windows}
      onWindowsChange={onWindowsChange}
      onClose={onClose}
    />
  );
}

function WindowDialogBody({
  window: w,
  today,
  windows,
  onWindowsChange,
  onClose,
}: {
  window: RunwayWindow | null;
  today: Date;
  windows: RunwayWindow[];
  onWindowsChange: (windows: RunwayWindow[]) => void;
  onClose: () => void;
}) {
  const ids = { label: useId(), start: useId(), end: useId() };
  const [label, setLabel] = useState(w?.label ?? "");
  const [start, setStart] = useState(w?.start ?? dayKey(today));
  const [end, setEnd] = useState(w?.end ?? "");
  const [tried, setTried] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const problem = !label.trim()
    ? "Give the window a name."
    : !start || !end
      ? "Pick its first and last day."
      : end < start
        ? "Its last day can't be before its first."
        : null;

  function save() {
    setTried(true);
    if (problem) return;
    const next: RunwayWindow = { id: w?.id ?? newWindowId(), label: label.trim(), start, end };
    onWindowsChange(w ? windows.map((x) => (x.id === w.id ? next : x)) : [...windows, next]);
    onClose();
  }

  const fieldLabel = "mb-1 block text-xs text-ink3";
  return (
    <>
      <Dialog
        open
        onClose={onClose}
        title={w ? "Edit window" : "New window"}
        subtitle="A stretch of days with a name of its own (a trip, a sprint), shown as its own column."
        widthClassName="max-w-sm"
        footer={
          <>
            {w && (
              <Button variant="danger" onClick={() => setConfirmDelete(true)} className="mr-auto">
                Delete window
              </Button>
            )}
            <Button variant="tertiary" onClick={onClose}>
              Cancel
            </Button>
            <Button variant="primary" onClick={save}>
              {w ? "Save" : "Add window"}
            </Button>
          </>
        }
      >
        <div className="mt-4 flex flex-col gap-3">
          <div>
            <label htmlFor={ids.label} className={fieldLabel}>
              Name
            </label>
            <Input
              id={ids.label}
              value={label}
              maxLength={80}
              data-autofocus
              placeholder="e.g. Lisbon trip"
              onChange={(e) => setLabel(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  save();
                }
              }}
            />
          </div>
          <div className="flex flex-wrap items-end gap-2">
            <div className="min-w-0 flex-1 basis-28">
              <label htmlFor={ids.start} className={fieldLabel}>
                First day
              </label>
              <DateField
                id={ids.start}
                value={start}
                onCommit={(iso) => {
                  setStart(iso);
                  if (iso && end && end < iso) setEnd("");
                }}
                ariaLabel="First day"
                className="h-8 px-2 text-xs"
              />
            </div>
            <div className="min-w-0 flex-1 basis-28">
              <label htmlFor={ids.end} className={fieldLabel}>
                Last day
              </label>
              <DateField
                id={ids.end}
                value={end}
                onCommit={setEnd}
                min={start || undefined}
                ariaLabel="Last day"
                className="h-8 px-2 text-xs"
              />
            </div>
          </div>
          {tried && problem && <Callout as="p">{problem}</Callout>}
        </div>
      </Dialog>
      <ConfirmDialog
        open={confirmDelete}
        title="Delete this window?"
        confirmLabel="Delete"
        danger
        onConfirm={() => {
          setConfirmDelete(false);
          if (w) onWindowsChange(windows.filter((x) => x.id !== w.id));
          onClose();
        }}
        onClose={() => setConfirmDelete(false)}
      >
        “{w?.label}” goes from the Runway. Its tasks keep their days and sit in the columns those
        days fall in.
      </ConfirmDialog>
    </>
  );
}
