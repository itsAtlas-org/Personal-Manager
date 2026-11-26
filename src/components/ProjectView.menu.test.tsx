// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The project page's right-click menus: a file row (the same builder as the Documents tab, with the
// same "Delete this file?" question and no delete of its own), and the Archive button's path, which
// now regenerates the briefing as Focus's always did.

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { ReactElement } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Document } from "../lib/types";

const data = vi.hoisted(() => ({ docs: [] as unknown[] }));
const ipc = vi.hoisted(() => ({
  setProjectArchived: vi.fn(async () => {}),
  deleteDocument: vi.fn(async () => {}),
}));
vi.mock("../lib/ipc", () => {
  const known: Record<string, unknown> = {
    ...ipc,
    listDocuments: vi.fn(async () => data.docs),
    listTasks: vi.fn(async () => []),
    listTaskStacks: vi.fn(async () => []),
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

const refresh = vi.hoisted(() => vi.fn(async () => {}));
vi.mock("../lib/briefing", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useBriefing: () => ({ briefing: null, busy: false, refresh }),
}));

import { ProjectView } from "./ProjectView";
import { ReaderProvider } from "../lib/reader";
import { CapabilityProvider } from "../lib/capabilities";
import { ThemeProvider } from "../theme";
import { ContextMenuHost } from "./ui/ContextMenuHost";
import { closeContextMenu } from "../lib/contextMenu";
import type { ProjectChat } from "../lib/useProjectChat";

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

// The scoped chat session App owns, as an empty thread: only what ProjectView reads.
const chat = {
  convId: null,
  messages: [],
  prompts: [],
  confidences: [],
  providers: [],
  thoughts: [],
  thoughtFold: {},
  streaming: null,
  streamingThought: null,
  error: null,
  fallback: null,
  sending: false,
  dismissedIdleFor: null,
  queue: {
    queued: [],
    stalled: false,
    failedId: null,
    full: false,
    remove: () => {},
    edit: () => {},
    hold: () => {},
    resume: () => {},
    enqueue: () => {},
  },
  setError: () => {},
  setDismissedIdleFor: () => {},
  dismissFallback: () => {},
  newChat: () => {},
  noteLiveFold: () => {},
  noteThoughtFold: () => {},
} as unknown as ProjectChat;

function mount(ui: ReactElement) {
  return render(
    <ThemeProvider>
      <CapabilityProvider>
        <ReaderProvider view="project:Atlas">{ui}</ReaderProvider>
      </CapabilityProvider>
      <ContextMenuHost />
    </ThemeProvider>,
  );
}

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
  data.docs = [];
});
afterEach(() => {
  act(() => closeContextMenu());
  cleanup();
});

async function view() {
  mount(
    <ProjectView
      project="Atlas"
      chat={chat}
      localAi={null}
      onUpgrade={() => {}}
      onBack={() => {}}
    />,
  );
  await act(async () => {});
}

const rightClick = (el: Element) => fireEvent.contextMenu(el, { clientX: 5, clientY: 5 });

describe("a project file row's menu", () => {
  it("offers Open, Copy title and Delete… last, and opens from the keyboard on the row", async () => {
    data.docs = [doc()];
    await view();
    const row = screen.getByText("Pitch deck").closest("li")!;
    row.focus();
    fireEvent.keyDown(row, { key: "ContextMenu" });
    const menu = screen.getByRole("menu", { name: "Document Pitch deck" });
    expect([...menu.querySelectorAll('[role="menuitem"]')].map((i) => i.textContent)).toEqual([
      "Open",
      "Copy title",
      "Delete…",
    ]);
  });

  it("Delete… asks the existing question and deletes only when it is answered", async () => {
    data.docs = [doc()];
    await view();
    rightClick(screen.getByText("Pitch deck").closest("li")!);
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete…" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Delete this file?")).toBeTruthy();
    expect(ipc.deleteDocument).not.toHaveBeenCalled();
  });

  it("names a saved chat as a chat", async () => {
    data.docs = [doc({ source_type: "chat", title: "Q3 chat" })];
    await view();
    rightClick(screen.getByText("Q3 chat").closest("li")!);
    expect(screen.getByRole("menu", { name: "Chat Q3 chat" })).toBeTruthy();
    expect(screen.getByRole("menuitem", { name: "Open chat" })).toBeTruthy();
  });
});

describe("the project page's Archive", () => {
  it("regenerates the briefing, which it never did", async () => {
    await view();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Archive" }));
    });
    expect(ipc.setProjectArchived).toHaveBeenCalledWith("Atlas", true);
    expect(refresh).toHaveBeenCalledOnce();
  });
});
