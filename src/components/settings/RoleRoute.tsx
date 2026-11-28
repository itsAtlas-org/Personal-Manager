// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useId, useLayoutEffect, useRef, useState } from "react";

import { providerView, roleName } from "../../lib/directProviders";
import {
  routeAdd,
  routeMove,
  routeRemove,
  routeReset,
  routeSetEffort,
  routeTryAgain,
} from "../../lib/ipc";
import {
  ADD_LABEL,
  addedAnnouncement,
  atDefaultCloudRows,
  firstEditLine,
  groupedWithAbove,
  HOW_TITLE,
  howParagraphs,
  LIST_RULE,
  type LocalFacts,
  modsBlocked,
  modsGetNoAnswer,
  movedAnnouncement,
  ONE_ROW_HINT,
  removedAnnouncement,
  rowName,
  sectionName,
  somethingElseUsable,
} from "../../lib/roleRoute";
import type {
  CloudProviderId,
  DirectProvidersState,
  RoleName,
  RouteEdited,
  RouteView,
} from "../../lib/types";
import { Button, SectionInfo, VisuallyHidden } from "../ui";
import { ResetLink } from "./ResetControls";
import { AddModelPicker } from "./AddModelPicker";
import { RouteRow } from "./RouteRow";

/** Where focus goes once an edit has drawn the new list. */
type Focus =
  { kind: "row"; key: string; act: "up" | "down" | "remove" } | { kind: "add" } | { kind: "none" };

/**
 * One role's half of AI & Models: the ordered list of models PM tries top to bottom, with the
 * buttons that move, remove and add rows.
 *
 * The list is Rust's. Every button asks for one edit and draws the list that comes back; the page
 * never works out a move, a status or a walk. A refused edit puts its words in `onError` and leaves
 * the list as it was. After a move focus returns to the same button on the item's new row (or, when
 * the item has reached an end and that button is now disabled, to the opposite one), and a polite
 * live region says what happened, because a reordered list is otherwise silent to a screen reader.
 */
export function RoleRoute({
  role,
  route,
  dp,
  openRouterKey,
  local,
  helpId,
  onRoute,
  onChanged,
  onError,
  defaultRoute,
  failed = false,
}: {
  role: RoleName;
  /** null until the backend has answered: the heading shows and the list is not guessed. */
  route: RouteView | null;
  dp: DirectProvidersState | null;
  /** An OpenRouter key is saved for this role. */
  openRouterKey: boolean;
  local: LocalFacts;
  helpId: string;
  /** An edit answered with the list as it now is. */
  onRoute: (role: RoleName, route: RouteView) => void;
  /** A row's health was cleared: read the lists again. */
  onChanged: () => void;
  onError: (message: string | null) => void;
  /** What a fresh install's list is, so the Reset link shows only for a list that differs. */
  defaultRoute?: RouteView;
  /** The lists could not be read (and none ever was): say so, with a way to try again, instead of a
   *  heading over nothing. `onChanged` is the retry (it reads the lists again). */
  failed?: boolean;
}) {
  const headingId = useId();
  const [announce, setAnnounce] = useState("");
  const [firstEdit, setFirstEdit] = useState<string | null>(null);
  const [picking, setPicking] = useState(false);
  const focus = useRef<Focus>({ kind: "none" });
  const working = useRef(false);
  const listRef = useRef<HTMLOListElement>(null);
  const addRef = useRef<HTMLButtonElement>(null);
  const who = roleName(role);

  // Focus is put back AFTER the list has been drawn, in the same frame: the keyed rows are moved,
  // and a browser drops focus from a node it moves.
  useLayoutEffect(() => {
    const f = focus.current;
    focus.current = { kind: "none" };
    if (f.kind === "add") {
      addRef.current?.focus();
    } else if (f.kind === "row") {
      const li = [...(listRef.current?.querySelectorAll<HTMLElement>("[data-row-key]") ?? [])].find(
        (el) => el.dataset.rowKey === f.key,
      );
      const pick = (act: string) => li?.querySelector<HTMLButtonElement>(`[data-act="${act}"]`);
      const own = pick(f.act);
      const opposite = pick(f.act === "up" ? "down" : "up");
      const target = own && !own.disabled ? own : opposite && !opposite.disabled ? opposite : own;
      target?.focus();
    }
  }, [route]);

  /** One edit at a time: a second click while one is on its way is dropped, not queued, so two edits
   *  can't both be worked out from the same old list. */
  async function edit(
    run: () => Promise<RouteEdited>,
    then: (edited: RouteEdited) => string,
    where: Focus,
  ) {
    if (working.current) return;
    working.current = true;
    onError(null);
    try {
      const edited = await run();
      focus.current = where;
      onRoute(role, edited.route);
      setAnnounce(then(edited));
      // Said once, on the edit that stored the list for the first time, under the list and aloud.
      setFirstEdit(edited.materialised ? firstEditLine(edited.unused_saved) : null);
    } catch (e) {
      focus.current = { kind: "none" };
      onError(String(e));
      onChanged();
    } finally {
      working.current = false;
    }
  }

  const entries = route?.entries ?? [];
  const count = entries.length;

  function move(index: number, dir: "up" | "down") {
    const to = dir === "up" ? index - 1 : index + 1;
    if (to < 0 || to >= count) return;
    const row = entries[index];
    void edit(
      () => routeMove(role, index, to, row.key),
      () => movedAnnouncement(rowName(row), to + 1, count),
      { kind: "row", key: row.key, act: dir },
    );
  }

  function remove(index: number) {
    const row = entries[index];
    const neighbour = entries[index + 1] ?? entries[index - 1];
    void edit(
      () => routeRemove(role, index, row.key),
      (e) => removedAnnouncement(rowName(row), e.route.entries.length, role),
      // Focus stays in the list, where the person was: on the Remove button of the row that took the
      // removed one's place (the row above when it was the last). "Add a model" only when no other
      // row is left to land on.
      neighbour ? { kind: "row", key: neighbour.key, act: "remove" } : { kind: "add" },
    );
  }

  function effort(index: number, value: string | null) {
    const row = entries[index];
    void edit(
      () => routeSetEffort(role, index, row.key, value),
      () => `Effort for ${rowName(row)} is now ${value ?? "the default"}`,
      { kind: "none" },
    );
  }

  async function tryAgain(provider: string) {
    onError(null);
    try {
      await routeTryAgain(provider as CloudProviderId);
      setAnnounce("PM will try it again");
    } catch (e) {
      onError(String(e));
    }
    onChanged();
  }

  async function add(provider: CloudProviderId | "local", model: string) {
    // Not `edit`: a refusal is shown inside the dialog, which stays open for another choice.
    if (working.current) return;
    working.current = true;
    try {
      const edited = await routeAdd(role, provider, model);
      setPicking(false);
      focus.current = { kind: "add" };
      onRoute(role, edited.route);
      const added = edited.route.entries[edited.route.entries.length - 1];
      setAnnounce(addedAnnouncement(added ? rowName(added) : "the model"));
      setFirstEdit(edited.materialised ? firstEditLine(edited.unused_saved) : null);
      onError(null);
    } finally {
      working.current = false;
    }
  }

  async function reset() {
    await edit(
      () => routeReset(role),
      () => `${who} is back to PM's default model`,
      { kind: "add" },
    );
  }

  const differs =
    route !== null && defaultRoute !== undefined && !atDefaultCloudRows(route, defaultRoute);
  const lone =
    count === 1 && route !== null && somethingElseUsable(route, dp, openRouterKey, local);
  const mods = role === "background" && route ? modsBlocked(route) : undefined;

  return (
    // The id is how "Edit my list" on the not-working card lands on THIS role's list.
    <section
      id={`sec-ai-route-${role}`}
      aria-labelledby={headingId}
      data-help={helpId}
      className="space-y-2"
    >
      <div className="flex items-center justify-between gap-2">
        {/* tabIndex -1: "Edit my list" lands focus here (SettingsView's deep link); not a Tab stop. */}
        <h2 id={headingId} tabIndex={-1} className="text-sm font-medium text-ink2">
          {sectionName(role)}
        </h2>
        {differs && <ResetLink onReset={() => void reset()} />}
      </div>
      <p className="text-xs text-ink3">{LIST_RULE}</p>

      {!route && failed && (
        <div role="alert" className="flex flex-wrap items-center gap-2 text-xs text-ink3">
          <span>PM couldn&apos;t read this list.</span>
          <Button size="xs" variant="secondary" onClick={onChanged}>
            Try again
          </Button>
        </div>
      )}

      {route && (
        <ol
          ref={listRef}
          // `list-style: none` (the reset) makes WebKitGTK drop the list semantics.
          role="list"
          aria-labelledby={headingId}
          data-help="settings-role-route"
          className="space-y-1.5"
        >
          {entries.map((row, i) => (
            <RouteRow
              key={row.key}
              role={role}
              row={row}
              index={i}
              count={count}
              grouped={groupedWithAbove(entries, i)}
              view={
                dp && row.provider !== "openrouter" && row.provider !== "local"
                  ? providerView(dp, row.provider)
                  : undefined
              }
              actions={{
                onMove: (dir) => move(i, dir),
                onRemove: () => remove(i),
                onEffort: (value) => effort(i, value),
                onTryAgain: () => void tryAgain(row.provider),
              }}
            />
          ))}
        </ol>
      )}

      {firstEdit && <p className="text-xs text-ink3">{firstEdit}</p>}
      {lone && <p className="text-xs text-ink3">{ONE_ROW_HINT}</p>}
      {mods && <p className="text-xs text-ink4">{modsGetNoAnswer(mods.provider)}</p>}

      {route && (
        <Button
          ref={addRef}
          type="button"
          variant="secondary"
          size="sm"
          aria-haspopup="dialog"
          aria-label={`${ADD_LABEL} to ${who}`}
          onClick={() => {
            onError(null);
            setPicking(true);
          }}
        >
          {ADD_LABEL}
        </Button>
      )}

      <SectionInfo title={HOW_TITLE}>
        {howParagraphs().map((p) => (
          <p key={p}>{p}</p>
        ))}
      </SectionInfo>

      {route && (
        <AddModelPicker
          role={role}
          open={picking}
          entries={entries}
          dp={dp}
          openRouterKey={openRouterKey}
          local={local}
          onClose={() => setPicking(false)}
          onAdd={add}
        />
      )}

      {/* Mounted from the first render: a region that arrives together with its text is often not read. */}
      <VisuallyHidden role="status" aria-live="polite">
        {[announce, firstEdit].filter(Boolean).join(". ")}
      </VisuallyHidden>
    </section>
  );
}
