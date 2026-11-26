// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The right-click menu of a document row, one builder for the Documents tab and a project's file list
// (the same `Document`, the same dialog to delete it). Every item but the two Copy ones repeats
// something the row or the reader already offers and calls that handler, so the confirm and the
// gating stay in one place: Delete… opens the existing "Delete this file?" question and never deletes.
// The row keeps its own Delete and Edit buttons, which only show on hover or focus, so the menu is a
// shortcut and not the only way to them. An item the row doesn't offer is left out, never greyed.
//
// The Copy items are new: the row has no copy button. They speak through `copyViaHost` ("Copied"),
// and the rows' keyboard route is the Menu key or Shift+F10 on the focused row.

import { copyViaHost, type MenuItem } from "../kit/lib/contextMenu";
import { openSource } from "./ipc";
import type { Document } from "./types";

export interface DocumentMenuHandlers {
  /** Open the reader on the document. */
  open: () => void;
  /** Open the project page of the document's home project. Left out, no "Open project". */
  openProject?: (project: string) => void;
  /** Open the row's inline editor for its project and importance (the Documents tab's Edit button,
   *  which only shows with the learning tools on). Absent where the row has no such editor. */
  edit?: () => void;
  /** Import a Google Sheet fully. Absent while an import runs, as the row's button is disabled. */
  importFully?: () => void;
  /** Ask "Delete this file?"; the dialog does the delete. */
  askDelete: () => void;
}

/** Whether the row can go to the document's source: an indexed file or page that is still there. The
 *  reader's own button is looser (it also shows for a missing source, where it fails); the menu
 *  leaves the item out instead of offering a dead one. */
export function hasOpenableSource(doc: Document): boolean {
  return (
    doc.source_type === "index_only" && !!doc.external_ref && doc.source_state !== "source_missing"
  );
}

/** Whether "Import fully" applies: a Google Sheet that is only indexed (its link points at
 *  /spreadsheets/) and still at its source. The Documents row's button uses the same test. */
export function canImportFully(doc: Document): boolean {
  return hasOpenableSource(doc) && !!doc.external_ref?.includes("/spreadsheets/");
}

/** Open the document's source the way the reader's button does (a failure is the system's to
 *  show, and the reader swallows it too). */
export function openDocumentSource(doc: Document): void {
  void openSource(doc.id).catch(() => {});
}

/** The menu's accessible name: "Chat <title>" for a saved chat, else "Document <title>". */
export function documentMenuLabel(doc: Document): string {
  return `${doc.source_type === "chat" ? "Chat" : "Document"} ${doc.title}`;
}

export function documentMenuItems(doc: Document, h: DocumentMenuHandlers): MenuItem[] {
  const isLocal = doc.source_id?.startsWith("local:") ?? false;
  const project = doc.project?.trim();
  const openProject = h.openProject;
  const link = doc.external_ref && /^https?:\/\//i.test(doc.external_ref) ? doc.external_ref : null;
  const edit = h.edit;
  return [
    { label: doc.source_type === "chat" ? "Open chat" : "Open", onSelect: h.open },
    ...(hasOpenableSource(doc)
      ? [
          {
            label: isLocal ? "Reveal in file manager" : "Open source",
            onSelect: () => openDocumentSource(doc),
          } satisfies MenuItem,
        ]
      : []),
    ...(project && project !== "Unsorted" && openProject
      ? [
          {
            label: `Open project ${project}`,
            onSelect: () => openProject(project),
          } satisfies MenuItem,
        ]
      : []),
    { kind: "separator" },
    ...(edit ? [{ label: "Change project or importance", onSelect: edit } satisfies MenuItem] : []),
    ...(canImportFully(doc) && h.importFully
      ? [{ label: "Import fully", onSelect: h.importFully } satisfies MenuItem]
      : []),
    { kind: "separator" },
    { label: "Copy title", onSelect: () => copyViaHost(doc.title, "Title copied") },
    ...(link
      ? [
          {
            label: "Copy link",
            onSelect: () => copyViaHost(link, "Link copied"),
          } satisfies MenuItem,
        ]
      : []),
    { kind: "separator" },
    { label: "Delete…", danger: true, onSelect: h.askDelete },
  ];
}
