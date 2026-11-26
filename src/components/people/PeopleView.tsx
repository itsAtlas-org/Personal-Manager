// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The People tab (Contacts, PR A): a header with the search box, filters and the list on the left,
// the person on the right. This slice builds the shell, the list, selecting several, Trash and its
// Undo; the record in the right pane, adding and editing, import and export, merging and the
// timeline come in later slices, each joining the pieces below as one more entry.
//
// The tab router unmounts a view on every tab switch, so this holds no truth of its own: the list is
// re-read on mount, on a filter, search or sort change, and on `people://changed` (`usePeopleData`).
// What is only ever view state is who is ticked, who is open and what is typed. `reveal` is the
// Calendar's one-shot pattern: Ctrl K and (later) the meeting dialog hand over a person's `uid`, the
// view takes it once, shows everyone and selects them; a uid that names nobody (deleted, merged away,
// in Trash) leaves nothing selected, never an error.

import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import { openContextMenu, wantsAppMenu } from "../../lib/contextMenu";
import {
  createLabel,
  deletePeopleNow,
  labelPeople,
  restorePeople,
  starPeople,
  trashPeople,
  unlabelPeople,
} from "../../lib/ipc";
import {
  errorWords,
  foldName,
  idsBetween,
  labelCoverage,
  readPeopleSort,
  SORT_OPTIONS,
  upcomingBirthdays,
  writePeopleSort,
} from "../../lib/people";
import { personMenuItems, personMenuLabel } from "../../lib/personMenu";
import type { PeopleFilter, PeopleSort, PersonRow } from "../../lib/types";
import { Button, Callout, ConfirmDialog, Input, Select, Skeleton } from "../ui";
import { LabelNameDialog, LabelPicker } from "./LabelPicker";
import { PeopleList } from "./PeopleList";
import { BirthdayStrip, FilterRail, PeoplePane } from "./PeopleSide";
import { SelectionBar } from "./SelectionBar";
import { UndoStrip, useUndoNotices, useUndoShortcut } from "./UndoStrip";
import { usePeopleData } from "./usePeopleData";

/** How long typing must pause before the search runs. */
const SEARCH_DELAY_MS = 200;
/** How long a plain notice ("2 people came back from Trash.") stays. */
const NOTE_MS = 6_000;

interface Props {
  /** A person to show, once: the `uid` and a nonce so the same person can be asked for again. */
  reveal?: { uid: string; nonce: number } | null;
  /** Called once `reveal` is taken, so the owner drops it and a later visit doesn't jump. */
  onRevealTaken?: () => void;
}

const EMPTY_WORDS: Record<string, string> = {
  Starred: "Nobody is starred yet.",
  Unlabelled: "Everyone has a label.",
  Trash: "Trash is empty.",
  Label: "Nobody has this label yet.",
};

export function PeopleView({ reveal, onRevealTaken }: Props) {
  const [filter, setFilter] = useState<PeopleFilter>("All");
  const [typed, setTyped] = useState("");
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<PeopleSort>(readPeopleSort);
  const data = usePeopleData(filter, query, sort);
  const { rows, reload } = data;

  const [checked, setChecked] = useState<ReadonlySet<number>>(new Set());
  const anchor = useRef<number | null>(null);
  const [activeId, setActiveId] = useState<number | null>(null);
  const undo = useUndoNotices();
  useUndoShortcut(undo.undoNewest);
  const [note, setNote] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<{ ids: number[]; name: string } | null>(null);
  const [deleteBusy, setDeleteBusy] = useState(false);
  const [labelling, setLabelling] = useState<number[] | null>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const searching = query.trim() !== "";
  const inTrash = filter === "Trash" && !searching;

  // --- search: as you type, debounced; clearing goes straight back to the filter's list -------------
  useEffect(() => {
    if (typed.trim() === "") {
      setQuery("");
      return;
    }
    const t = window.setTimeout(() => setQuery(typed), SEARCH_DELAY_MS);
    return () => window.clearTimeout(t);
  }, [typed]);

  function chooseFilter(next: PeopleFilter) {
    setFilter(next);
    setTyped("");
    setQuery("");
    setChecked(new Set());
    setProblem(null);
    anchor.current = null;
  }

  function chooseSort(next: PeopleSort) {
    setSort(next);
    writePeopleSort(next);
  }

  // --- a notice that goes by itself ------------------------------------------------------------------
  useEffect(() => {
    if (!note) return;
    const t = window.setTimeout(() => setNote(null), NOTE_MS);
    return () => window.clearTimeout(t);
  }, [note]);

  // --- reveal: take it once, show everyone, select the person if they are there -----------------------
  const [wanted, setWanted] = useState<string | null>(null);
  const revealNonce = reveal?.nonce;
  const takenRef = useRef(onRevealTaken);
  takenRef.current = onRevealTaken;
  useEffect(() => {
    if (!reveal) return;
    setFilter("All");
    setTyped("");
    setQuery("");
    setChecked(new Set());
    setWanted(reveal.uid);
    takenRef.current?.();
    // Only a new reveal moves the view: later renders mustn't jump back to it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [revealNonce]);
  useEffect(() => {
    if (wanted === null || !data.settled || filter !== "All" || searching) return;
    const row = rows.find((r) => r.uid === wanted);
    setWanted(null);
    if (!row) {
      setActiveId(null);
      return;
    }
    setActiveId(row.id);
    anchor.current = row.id;
    listRef.current
      ?.querySelector(`[data-person-id="${row.id}"]`)
      ?.scrollIntoView?.({ block: "nearest" });
  }, [wanted, data.settled, filter, searching, rows]);

  // --- what is ticked or open must still be in the list ------------------------------------------------
  useEffect(() => {
    if (!data.settled) return;
    const ids = new Set(rows.map((r) => r.id));
    setChecked((prev) => {
      const kept = [...prev].filter((id) => ids.has(id));
      return kept.length === prev.size ? prev : new Set(kept);
    });
    setActiveId((id) => (id !== null && !ids.has(id) ? null : id));
  }, [rows, data.settled]);

  // --- focus after a row goes: a neighbour, only if focus fell to the page ---------------------------------
  const pendingFocus = useRef<{ id: number; gone: number[] } | null>(null);
  useEffect(() => {
    const p = pendingFocus.current;
    if (!p || p.gone.some((id) => rows.some((r) => r.id === id))) return;
    pendingFocus.current = null;
    const el = document.activeElement;
    if (el && el !== document.body) return;
    listRef.current?.querySelector<HTMLElement>(`[data-person-main="${p.id}"]`)?.focus();
  }, [rows]);

  function planFocus(ids: readonly number[]) {
    const gone = new Set(ids);
    const at = Math.max(...rows.map((r, i) => (gone.has(r.id) ? i : -1)));
    const next =
      rows.slice(at + 1).find((r) => !gone.has(r.id)) ??
      [...rows.slice(0, at)].reverse().find((r) => !gone.has(r.id));
    pendingFocus.current = next ? { id: next.id, gone: [...ids] } : null;
  }

  // --- doing things --------------------------------------------------------------------------------------
  /** One write: any failure is said in words and the list re-read, so it never shows a guess. */
  const run = useCallback(
    async (write: () => Promise<unknown>): Promise<boolean> => {
      setProblem(null);
      try {
        await write();
        return true;
      } catch (err) {
        setProblem(errorWords(err));
        return false;
      } finally {
        reload();
      }
    },
    [reload],
  );

  const star = (targets: readonly PersonRow[], starred: boolean) =>
    void run(() =>
      starPeople(
        targets.map((r) => r.id),
        starred,
      ),
    );

  const toggleLabel = (ids: number[], labelId: number, on: boolean) =>
    void run(() => (on ? labelPeople(ids, labelId) : unlabelPeople(ids, labelId)));

  async function toTrash(targets: readonly PersonRow[]) {
    const ids = targets.map((r) => r.id);
    planFocus(ids);
    const ok = await run(() => trashPeople(ids));
    if (!ok) {
      pendingFocus.current = null;
      return;
    }
    setChecked(new Set());
    if (activeId !== null && ids.includes(activeId)) setActiveId(null);
    undo.push(
      ids.length === 1
        ? `Moved ${targets[0].display_name} to Trash.`
        : `Moved ${ids.length} people to Trash.`,
      async () => {
        try {
          await restorePeople(ids);
        } catch (err) {
          setProblem(errorWords(err));
        } finally {
          reload();
        }
      },
    );
  }

  async function fromTrash(targets: readonly PersonRow[]) {
    const ids = targets.map((r) => r.id);
    planFocus(ids);
    let n = 0;
    const ok = await run(async () => {
      n = await restorePeople(ids);
    });
    if (!ok) {
      pendingFocus.current = null;
      return;
    }
    setChecked(new Set());
    setNote(n === 1 ? "1 person came back from Trash." : `${n} people came back from Trash.`);
  }

  async function confirmDeleteNow() {
    if (!deleting) return;
    setDeleteBusy(true);
    planFocus(deleting.ids);
    const ok = await run(() => deletePeopleNow(deleting.ids));
    setDeleteBusy(false);
    if (!ok) {
      pendingFocus.current = null;
    } else {
      setChecked(new Set());
    }
    setDeleting(null);
  }

  async function nameLabel(ids: number[], name: string) {
    const key = foldName(name);
    const existing = data.labels.find((l) => foldName(l.name) === key);
    try {
      const id = existing?.id ?? (await createLabel(name));
      await labelPeople(ids, id);
    } finally {
      reload();
    }
  }

  // --- selecting ---------------------------------------------------------------------------------------------
  function extendTo(id: number, from: number | null) {
    const span = idsBetween(rows, from, id);
    setChecked((prev) => new Set([...prev, ...span]));
  }

  function toggle(row: PersonRow, shift: boolean) {
    if (shift && anchor.current !== null) {
      extendTo(row.id, anchor.current);
      return;
    }
    anchor.current = row.id;
    setChecked((prev) => {
      const next = new Set(prev);
      if (!next.delete(row.id)) next.add(row.id);
      return next;
    });
  }

  function open(row: PersonRow, e: MouseEvent) {
    if (e.shiftKey) {
      extendTo(row.id, anchor.current ?? activeId);
      return;
    }
    if (e.ctrlKey || e.metaKey) {
      toggle(row, false);
      return;
    }
    anchor.current = row.id;
    setActiveId(row.id);
  }

  const selected = useMemo(() => rows.filter((r) => checked.has(r.id)), [rows, checked]);

  /** Who a row's action reaches: the whole selection when the row is part of one, else the row. */
  const targetsOf = (row: PersonRow): PersonRow[] =>
    checked.has(row.id) && selected.length > 1 ? selected : [row];

  function rowMenu(e: MouseEvent, row: PersonRow) {
    if (!wantsAppMenu(e)) return;
    const targets = targetsOf(row);
    const ids = targets.map((r) => r.id);
    const { all } = labelCoverage(targets);
    openContextMenu(
      e,
      personMenuLabel(row.display_name, targets.length),
      personMenuItems(
        {
          count: targets.length,
          trashed: targets.every((r) => r.trashed_at !== null),
          allStarred: targets.every((r) => r.starred),
          labels: data.labels,
          carriedByAll: all,
        },
        {
          open: () => {
            anchor.current = row.id;
            setActiveId(row.id);
          },
          star: (starred) => star(targets, starred),
          toggleLabel: (labelId, on) => toggleLabel(ids, labelId, on),
          newLabel: () => setLabelling(ids),
          trash: () => void toTrash(targets),
          restore: () => void fromTrash(targets),
          askDeleteNow: () =>
            setDeleting({
              ids,
              name: targets.length === 1 ? targets[0].display_name : `${ids.length} people`,
            }),
        },
      ),
      { withFallback: true },
    );
  }

  // --- what the list says ----------------------------------------------------------------------------------
  const birthdays = useMemo(
    () => (filter === "All" && !searching ? upcomingBirthdays(rows, new Date()) : []),
    [filter, searching, rows],
  );
  const active = rows.find((r) => r.id === activeId) ?? null;
  const filterName =
    typeof filter === "object"
      ? (data.labels.find((l) => l.id === filter.Label)?.name ?? "Label")
      : filter;
  const emptyWords = searching
    ? `Nobody matches “${query.trim()}”.`
    : filter === "All"
      ? "Nobody here yet. Add someone, or import a vCard file from your phone or iCloud."
      : EMPTY_WORDS[typeof filter === "object" ? "Label" : filter];

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col">
      <header className="flex flex-wrap items-center justify-between gap-3 border-b border-border px-6 py-3">
        <div>
          <h1 className="font-head text-sm font-semibold text-ink">People</h1>
          <p className="text-xs text-ink3">
            {data.loading
              ? "Reading your people…"
              : `${rows.length} ${rows.length === 1 ? "person" : "people"}${searching ? " found" : ""}`}
          </p>
        </div>
        <div className="w-full max-w-sm" data-help="people-list">
          <Input
            type="search"
            aria-label="Search people"
            placeholder="Search names, emails, companies, phone numbers"
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
          />
        </div>
      </header>

      <div className="flex min-h-0 flex-1">
        <aside className="w-44 shrink-0 overflow-y-auto border-r border-border">
          <FilterRail filter={filter} labels={data.labels} onChange={chooseFilter} />
        </aside>

        <section
          aria-label={searching ? "Search results" : filterName}
          className="flex min-h-0 min-w-0 flex-1 basis-0 flex-col border-r border-border"
        >
          <div className="flex items-center justify-between gap-2 border-b border-border px-4 py-2">
            <h2 className="min-w-0 truncate text-sm font-semibold text-ink">
              {searching ? `Results for “${query.trim()}”` : filterName}
            </h2>
            <label className="flex shrink-0 items-center gap-2 text-xs text-ink3">
              Sort by
              <Select
                compact
                data-help="people-list"
                value={sort}
                disabled={searching}
                onChange={(e) => chooseSort(e.target.value as PeopleSort)}
              >
                {SORT_OPTIONS.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </Select>
            </label>
          </div>

          {inTrash && (
            <p
              data-help="people-trash"
              className="border-b border-border px-4 py-2 text-xs text-ink3"
            >
              People in Trash are deleted for good after 30 days.
            </p>
          )}
          {note && (
            <Callout tone="info" variant="strip" size="sm" body="ink" live>
              {note}
            </Callout>
          )}
          {(problem || (data.failed && !data.loading)) && (
            <Callout tone="danger" variant="strip" size="sm">
              <span>{problem ?? "PM couldn't read your people just now."}</span>{" "}
              {problem ? (
                // A failed write isn't repeated for you: the list is already re-read, so the
                // message only needs to go away.
                <Button variant="tertiary" size="xs" onClick={() => setProblem(null)}>
                  Dismiss
                </Button>
              ) : (
                <Button variant="tertiary" size="xs" onClick={reload}>
                  Try again
                </Button>
              )}
            </Callout>
          )}

          {selected.length > 0 && (
            <SelectionBar count={selected.length} onClear={() => setChecked(new Set())}>
              {inTrash ? (
                <>
                  <Button variant="secondary" size="xs" onClick={() => void fromTrash(selected)}>
                    Restore
                  </Button>
                  <Button
                    variant="danger"
                    size="xs"
                    onClick={() =>
                      setDeleting({
                        ids: selected.map((r) => r.id),
                        name:
                          selected.length === 1
                            ? selected[0].display_name
                            : `${selected.length} people`,
                      })
                    }
                  >
                    Delete now…
                  </Button>
                </>
              ) : (
                <>
                  <Button
                    variant="secondary"
                    size="xs"
                    onClick={() => star(selected, !selected.every((r) => r.starred))}
                  >
                    {selected.every((r) => r.starred) ? "Unstar" : "Star"}
                  </Button>
                  <LabelPicker
                    labels={data.labels}
                    targets={selected}
                    onToggle={(labelId, on) =>
                      toggleLabel(
                        selected.map((r) => r.id),
                        labelId,
                        on,
                      )
                    }
                    onNew={() => setLabelling(selected.map((r) => r.id))}
                  />
                  <Button variant="danger" size="xs" onClick={() => void toTrash(selected)}>
                    Move to Trash
                  </Button>
                </>
              )}
            </SelectionBar>
          )}

          <div ref={listRef} className="min-h-0 flex-1 overflow-y-auto">
            {data.loading ? (
              <div
                className="flex flex-col gap-3 p-4"
                role="status"
                aria-label="Reading your people"
              >
                {[0, 1, 2, 3, 4].map((i) => (
                  <Skeleton key={i} className="h-9" />
                ))}
              </div>
            ) : rows.length === 0 && !data.failed ? (
              <p className="px-4 py-8 text-center text-sm text-ink3">{emptyWords}</p>
            ) : (
              <>
                <BirthdayStrip
                  birthdays={birthdays}
                  onOpen={(row) => {
                    anchor.current = row.id;
                    setActiveId(row.id);
                  }}
                />
                <PeopleList
                  rows={rows}
                  checked={checked}
                  activeId={activeId}
                  inTrash={inTrash}
                  onOpen={open}
                  onToggle={toggle}
                  onStar={(row) => star([row], !row.starred)}
                  onRestore={(row) => void fromTrash([row])}
                  onMenu={rowMenu}
                />
              </>
            )}
          </div>
        </section>

        <div className="min-h-0 min-w-0 flex-[1.2] basis-0 overflow-y-auto">
          <PeoplePane row={active} />
        </div>
      </div>

      {/* Dialogs and the strip sit here, beside the list and never inside it: a right-click in a
          dialog must not reach the list's menu handler. */}
      {labelling && (
        <LabelNameDialog
          count={labelling.length}
          onSubmit={(name) => nameLabel(labelling, name)}
          onClose={() => setLabelling(null)}
        />
      )}
      <ConfirmDialog
        open={deleting !== null}
        title={deleting ? `Delete ${deleting.name} for good?` : "Delete for good?"}
        confirmLabel="Delete for good"
        danger
        busy={deleteBusy}
        initialFocus="cancel"
        helpId="people-trash"
        onConfirm={() => void confirmDeleteNow()}
        onClose={() => setDeleting(null)}
      >
        <p className="mt-2 text-sm text-ink2">
          Their details and labels are deleted too. This can't be undone.
        </p>
      </ConfirmDialog>
      <UndoStrip notices={undo.notices} onUndo={undo.undo} onDismiss={undo.dismiss} />
    </div>
  );
}
