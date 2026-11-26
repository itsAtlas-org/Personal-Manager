// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Help mode on the Documents tab: every column header, the header buttons, the Columns picker, the
// source badge and the banners must carry an id that HELP defines. An id with no entry shows nothing
// and nothing else would notice, and the table's columns are a list that grows (DOC_COLUMN_KEYS), so
// this walks that list rather than naming the columns it knows about today.

import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Document } from "../lib/types";

const data = vi.hoisted(() => ({ docs: [] as unknown[] }));
vi.mock("../lib/ipc", () => {
  const known: Record<string, unknown> = {
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

import { CapabilityProvider } from "../lib/capabilities";
import { DOC_COLUMN_HELP_IDS, DOC_COLUMN_KEYS, writeColumns } from "../lib/documentColumns";
import { HELP } from "../lib/help";
import { ReaderProvider } from "../lib/reader";
import { ThemeProvider } from "../theme";
import { DocumentsView } from "./DocumentsView";

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

async function view() {
  render(
    <ThemeProvider>
      <CapabilityProvider>
        <ReaderProvider view="documents" onOpenProject={() => {}}>
          <DocumentsView onReviewClick={() => {}} />
        </ReaderProvider>
      </CapabilityProvider>
    </ThemeProvider>,
  );
  await act(async () => {});
}

/** The help id an element answers to, the way help mode finds it: the nearest `[data-help]`. */
const helpOf = (el: Element) => el.closest("[data-help]")?.getAttribute("data-help");

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
  data.docs = [
    doc(),
    // A connected file whose original is gone: draws the Source badge, the "can't be reached"
    // banner and the Reset… button, none of which exist for a plain vault file.
    doc({
      id: 2,
      title: "Lost notes",
      source_type: "index_only",
      source_state: "source_missing",
      source_id: "gdrive:me@example.com:abc",
    }),
  ];
});
afterEach(cleanup);

describe("help mode on the Documents tab", () => {
  it("every column header, with every column on, answers to an entry that exists", async () => {
    writeColumns([...DOC_COLUMN_KEYS]);
    await view();
    const heads = [...document.querySelectorAll("thead th")];
    // Title is always on, and is not one of the picker's keys.
    expect(heads).toHaveLength(DOC_COLUMN_KEYS.length + 1);
    const ids = heads.map((th) => th.getAttribute("data-help"));
    expect(ids).toEqual([
      "documents-col-title",
      ...DOC_COLUMN_KEYS.map((k) => DOC_COLUMN_HELP_IDS[k]),
    ]);
    for (const id of ids) expect(HELP[id!], String(id)).toBeDefined();
    // One id per column: two headers sharing an entry would explain one of them wrongly.
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("the Columns picker explains columns, not the All files / Folders / Projects switch", async () => {
    await view();
    const columns = screen.getByRole("button", { name: "Columns" });
    expect(helpOf(columns)).toBe("documents-columns");
    const layout = screen.getByRole("button", { name: "All files" });
    expect(helpOf(layout)).toBe("documents-view");
    // They must not nest: help mode outlines every ancestor carrying the attribute.
    expect(columns.closest("[data-help='documents-view']")).toBeNull();
    expect(layout.closest("[data-help='documents-columns']")).toBeNull();
  });

  it("the header buttons, the badge and the banners each have an entry", async () => {
    await view();
    const found: [string, Element | null][] = [
      ["documents-add", screen.getByRole("button", { name: "Add files" })],
      ["documents-add", screen.getByRole("button", { name: "Add folder" })],
      ["documents-reset", screen.getByRole("button", { name: "Reset…" })],
      ["documents-source-badge", screen.getByText("Source missing")],
      ["documents-dead-banner", screen.getByText(/can.t be reached at the source/)],
      ["documents-row-delete", screen.getAllByRole("button", { name: "Delete this document" })[0]],
    ];
    for (const [id, el] of found) {
      expect(helpOf(el!), id).toBe(id);
      expect(HELP[id], id).toBeDefined();
    }
  });
});
