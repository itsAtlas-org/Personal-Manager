// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The right-click menu of a project where the app lists one: a Focus card, a row of Focus's Archive
// folder, and an entry in the sidebar's Projects list. One builder, and each surface hands it only
// the handlers it has: an item whose handler is absent is left out, so the sidebar entry (which can
// open and archive) and the card (which can do all of it) share the wording and the order.
//
// Each item repeats a control that is already on screen, and calls its handler. On a card, Archive,
// Merge into… and Delete… sit inside the Triage editor, which is why Triage is in the menu too. Merge
// and Delete open the same type-to-confirm dialogs as the editor's buttons, so the menu never deletes
// or merges by itself; Archive is the gentle one (undone from the Archive folder) and has no confirm
// there either. Delete… is last.

import type { MenuItem } from "../kit/lib/contextMenu";

export interface ProjectMenuHandlers {
  open: () => void;
  /** Open or close the card's Triage editor. Absent where there is none. */
  triage?: () => void;
  /** Archive the project (shown while it isn't archived). */
  archive?: () => void;
  /** Put an archived project back (shown while it is archived). */
  unarchive?: () => void;
  /** Open "Merge into…". Absent when there is no other project to merge into. */
  merge?: () => void;
  /** Open "Delete project…". */
  askDelete?: () => void;
}

/** `triageOpen` words the Triage item as the card's own button does: "Close triage" while it is up. */
export function projectMenuItems(
  h: ProjectMenuHandlers,
  opts: { archived?: boolean; triageOpen?: boolean } = {},
): MenuItem[] {
  const { archive, unarchive, merge, askDelete, triage } = h;
  const keeping: MenuItem[] = [];
  if (!opts.archived && archive) keeping.push({ label: "Archive", onSelect: archive });
  if (opts.archived && unarchive) keeping.push({ label: "Unarchive", onSelect: unarchive });
  if (!opts.archived && merge) keeping.push({ label: "Merge into…", onSelect: merge });
  return [
    { label: "Open", onSelect: h.open },
    ...(triage
      ? [
          {
            label: opts.triageOpen ? "Close triage" : "Triage",
            onSelect: triage,
          } satisfies MenuItem,
        ]
      : []),
    { kind: "separator" },
    ...keeping,
    { kind: "separator" },
    ...(!opts.archived && askDelete
      ? [{ label: "Delete…", danger: true, onSelect: askDelete } satisfies MenuItem]
      : []),
  ];
}
