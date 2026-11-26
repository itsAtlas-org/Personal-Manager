// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The right-click menu of a row in the People list. One pure builder; the tab hands it the handlers
// it already has, so every item repeats a control on screen (the star, the selection bar's Label and
// Move to Trash, the Trash view's Restore and Delete now) and calls the same code: a confirm lives
// in one place, and the menu never deletes by itself. "Delete now…" ends in an ellipsis because a
// dialog follows; "Move to Trash" has none because an Undo strip follows instead.
//
// The labels are radios (`checked`), one per label, ticked when every person the menu acts on carries
// it: PM's menu has no submenu, and this is the same shape as a task's status items. When the row is
// part of a checked selection the menu acts on all of it (`count`), and Open (one record) is left out.
// A row in Trash offers only Restore and Delete now: nothing else makes sense there. Edit… repeats
// the record's Edit button, for a row that isn't the one open.
//
// Merge sits beside Star: "Merge with…" on one person (it asks who with), "Merge these two…" on a
// selection of exactly two (the dialog opens at once); both end in an ellipsis because a dialog
// follows, and a merge has an Undo strip, so neither asks again. A selection of three or more has
// no merge. Log a meeting joins in the slice that builds it, one entry.

import type { MenuItem } from "../kit/lib/contextMenu";

export interface PersonMenuHandlers {
  open: () => void;
  /** Open the editor for this person (the record's own Edit button, from the row). */
  edit: () => void;
  /** Star (`true`) or unstar (`false`) everyone the menu acts on. */
  star: (starred: boolean) => void;
  /** Put a label on everyone the menu acts on (`on`), or take it off. */
  toggleLabel: (labelId: number, on: boolean) => void;
  /** Ask for a new label's name; the dialog labels them too. */
  newLabel: () => void;
  /** Move them to Trash, with the Undo strip. */
  trash: () => void;
  /** Bring them back from Trash. */
  restore: () => void;
  /** Ask "Delete for good?"; the dialog does the delete. */
  askDeleteNow: () => void;
  /** Start a merge: with one person, a picker for the other; with exactly two, the dialog. */
  merge: () => void;
  /** Open the export dialog for the people the menu acts on (the selection bar's Export…). */
  exportThem: () => void;
}

export interface PersonMenuState {
  /** The selection bar, which carries Export…, is on screen for these people: the menu repeats it
   *  only then, and never offers it for a lone unticked row. */
  canExport?: boolean;
  /** How many people the menu acts on: 1 for a lone row, the selection's size for a checked one. */
  count: number;
  /** The row is in Trash (and so is everyone with it). */
  trashed: boolean;
  /** Everyone the menu acts on is starred already. */
  allStarred: boolean;
  /** Every label there is. */
  labels: readonly { id: number; name: string }[];
  /** The labels every one of them carries. */
  carriedByAll: ReadonlySet<number>;
}

/** The menu's accessible name: "Person Ada Lovelace", or "People: 3 selected". */
export function personMenuLabel(name: string, count: number): string {
  return count > 1 ? `People: ${count} selected` : `Person ${name}`;
}

export function personMenuItems(s: PersonMenuState, h: PersonMenuHandlers): MenuItem[] {
  const open: MenuItem[] = s.count === 1 ? [{ label: "Open", onSelect: h.open }] : [];
  if (s.trashed) {
    return [
      ...open,
      { label: "Restore", onSelect: h.restore },
      { kind: "separator" },
      { label: "Delete now…", danger: true, onSelect: h.askDeleteNow },
    ];
  }
  // Edit… ends in an ellipsis because a dialog follows. It sits with Open: both are about the one
  // record, so a selection of several has neither.
  const edit: MenuItem[] = s.count === 1 ? [{ label: "Edit…", onSelect: h.edit }] : [];
  const merge: MenuItem[] =
    s.count === 1
      ? [{ label: "Merge with…", onSelect: h.merge }]
      : s.count === 2
        ? [{ label: "Merge these two…", onSelect: h.merge }]
        : [];
  return [
    ...open,
    ...edit,
    {
      label: s.allStarred ? "Unstar" : "Star",
      onSelect: () => h.star(!s.allStarred),
    },
    ...merge,
    ...(s.canExport ? [{ label: "Export…", onSelect: h.exportThem } as MenuItem] : []),
    { kind: "separator" },
    ...s.labels.map((l): MenuItem => ({
      label: `Label: ${l.name}`,
      checked: s.carriedByAll.has(l.id),
      onSelect: () => h.toggleLabel(l.id, !s.carriedByAll.has(l.id)),
    })),
    { label: "New label…", onSelect: h.newLabel },
    { kind: "separator" },
    { label: "Move to Trash", danger: true, onSelect: h.trash },
  ];
}
