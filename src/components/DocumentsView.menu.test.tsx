// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Documents tab's row menu, through the real right-click menu: Open uses the row's reader, Open
// project the reader provider's navigation, Copy title speaks, and Delete… opens the existing
// "Delete this file?" question and deletes nothing itself. The row stays focusable, which is the
// menu's keyboard route (the Menu key or Shift+F10 on it).

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Document } from "../lib/types";

const data = vi.hoisted(() => ({ docs: [] as unknown[] }));
const ipc = vi.hoisted(() => ({ deleteDocument: vi.fn(async () => {}) }));
vi.mock("../lib/ipc", () => {
  const known: Record<string, unknown> = {
    ...ipc,
    listDocuments: vi.fn(async () => data.docs),
    duplicateSnapshot: vi.fn(async () => null),
    getPref: vi.fn(async () => null),
    setPref: vi.fn(async () => {}),
    vaultStatus: vi.fn(async () => null),
  };
  return new Proxy(known, {
    has: () => true,
    get(target, prop: string) {
      if (prop === "then") return undefined;
      if (!(prop in target)) {
        target[prop] = /^on[A-Z]/.test(prop) ? vi.fn(async () => () => {}) : vi.fn(async () => []);
      }
      return target[prop];
    },
  });
});
vi.mock("./OrphanSweepBanner", () => ({ OrphanSweepBanner: () => null }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: async () => () => {} }),
}));

import { DocumentsView } from "./DocumentsView";
import { ReaderProvider } from "../lib/reader";
import { CapabilityProvider } from "../lib/capabilities";
import { ThemeProvider } from "../theme";
import { ContextMenuHost } from "./ui/ContextMenuHost";
import { closeContextMenu } from "../lib/contextMenu";

function doc(over: Partial<Document> = {}): Document {
  return {
    id: 1,
    title: "Pitch deck",
    source_path: null,
    ext: "md",
    byte_size: 10,
    chunk_count: 2,
    created_at: null,
    ingested_at: "2026-07-30T09:00:00Z",
    project: "Atlas",
    linked_projects: [],
    tags: [],
    importance: null,
    reviewed: true,
    last_activity: null,
    source_type: "vault",
    source_state: "ok",
    external_ref: null,
    source_id: null,
    ...over,
  } as unknown as Document;
}

const onOpenProject = vi.fn();
const writeText = vi.fn(async () => {});

async function view() {
  render(
    <ThemeProvider>
      <CapabilityProvider>
        <ReaderProvider view="documents" onOpenProject={onOpenProject}>
          <DocumentsView onReviewClick={() => {}} />
        </ReaderProvider>
      </CapabilityProvider>
      <ContextMenuHost />
    </ThemeProvider>,
  );
  await act(async () => {});
}

const rightClick = (el: Element) => fireEvent.contextMenu(el, { clientX: 5, clientY: 5 });
const rowOf = (title: string) => screen.getByText(title).closest("tr")!;

beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
  Element.prototype.scrollIntoView ??= () => {};
});
beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  data.docs = [doc(), doc({ id: 2, title: "Notes", project: "Beta" })];
  Object.assign(navigator, { clipboard: { writeText } });
});
afterEach(() => {
  act(() => closeContextMenu());
  cleanup();
});

describe("a Documents row's menu", () => {
  it("opens for the row under the pointer, named for it, and from the keyboard on the focused row", async () => {
    await view();
    rightClick(rowOf("Notes"));
    expect(screen.getByRole("menu", { name: "Document Notes" })).toBeTruthy();
    act(() => closeContextMenu());

    const row = rowOf("Pitch deck");
    row.focus();
    fireEvent.keyDown(row, { key: "F10", shiftKey: true });
    expect(screen.getByRole("menu", { name: "Document Pitch deck" })).toBeTruthy();
  });

  it("Open project goes to the row's project, and Copy title copies it", async () => {
    await view();
    rightClick(rowOf("Pitch deck"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Open project Atlas" }));
    expect(onOpenProject).toHaveBeenCalledWith("Atlas");

    rightClick(rowOf("Pitch deck"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Copy title" }));
    expect(writeText).toHaveBeenCalledWith("Pitch deck");
  });

  it("Delete… opens the existing question and deletes nothing by itself", async () => {
    await view();
    rightClick(rowOf("Pitch deck"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete…" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Delete this file?")).toBeTruthy();
    expect(ipc.deleteDocument).not.toHaveBeenCalled();
  });

  it("keeps the row's own Delete button", async () => {
    await view();
    expect(
      within(rowOf("Pitch deck")).getByRole("button", { name: "Delete this document" }),
    ).toBeTruthy();
  });
});
