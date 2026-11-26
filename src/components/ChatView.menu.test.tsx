// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// An answer's Copy and right-click menu. The bubble can't take focus, so Copy is a visible button
// beside Save as note, and the menu repeats it (with Save as note and the rating) as a shortcut. The
// button and the menu share one save and one rating, so they can't disagree.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const ipc = vi.hoisted(() => ({
  listTags: vi.fn(async () => []),
  answerFeedback: vi.fn(async () => ({ rating: null })),
  rateAnswer: vi.fn(async () => {}),
  ingestNote: vi.fn(async () => {}),
  recordCitationClick: vi.fn(async () => {}),
  getSettings: vi.fn(async () => ({})),
  setRetrievalConfidenceThreshold: vi.fn(async () => {}),
}));
vi.mock("../lib/ipc", () => ipc);
vi.mock("../lib/capabilities", () => ({
  useDevMode: () => ({ devMode: false, setDevMode: () => {} }),
  isDevBuild: false,
}));
vi.mock("../lib/reader", () => ({
  useReader: () => ({ openReader: () => {}, openReaderById: () => {}, current: null }),
}));
const depth = vi.hoisted(() => ({ value: "standard" as "minimal" | "standard" }));
vi.mock("../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: depth.value }),
}));
vi.mock("../theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useDepth: () => ({
    depth: depth.value,
    atLeast: (d: string) => depth.value === "standard" || d === "minimal",
    showPower: false,
  }),
}));

import { ChatView } from "./ChatView";
import { ContextMenuHost } from "./ui/ContextMenuHost";
import { closeContextMenu } from "../lib/contextMenu";

const user = {
  id: 1,
  conversation_id: 1,
  role: "user" as const,
  content: "what is the plan?",
  model: null,
  created_at: "2026-07-28T10:00:00Z",
};
const answer = {
  ...user,
  id: 2,
  role: "assistant" as const,
  content: "Ship on Friday.",
  model: "m",
  citations: [{ document_id: 5, title: "Plan", source_path: null, vault_path: "plan.md" }],
};
const bare = { ...answer, id: 3, citations: [] };

const writeText = vi.fn(async () => {});
// The save's own status line, under the answer (the page has other status regions).
const saveStatus = () =>
  within(document.querySelector("[data-help='chat-answer-actions']") as HTMLElement).getByRole(
    "status",
  );

function mount(messages: unknown[]) {
  return render(
    <>
      <ChatView messages={messages as never} streaming={null} />
      <ContextMenuHost />
    </>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  depth.value = "standard";
  Element.prototype.scrollIntoView = vi.fn();
  Object.assign(navigator, { clipboard: { writeText } });
});
afterEach(() => {
  act(() => closeContextMenu());
  cleanup();
});

describe("the visible Copy button", () => {
  it("copies the answer, beside Save as note", () => {
    mount([user, bare]);
    fireEvent.click(screen.getByRole("button", { name: "Copy answer" }));
    expect(writeText).toHaveBeenCalledWith("Ship on Friday.");
    expect(screen.getByRole("button", { name: "Save as note" })).toBeTruthy();
  });

  it("shows a visible Copied ✓ for a moment, not only the spoken line", () => {
    mount([bare]);
    fireEvent.click(screen.getByRole("button", { name: "Copy answer" }));
    expect(screen.getByRole("button", { name: "Copied ✓" })).toBeTruthy();
  });

  it("is on the answer only, not on the user's own message", () => {
    mount([user]);
    expect(screen.queryByRole("button", { name: "Copy answer" })).toBeNull();
  });
});

describe("an answer's right-click menu", () => {
  it("offers Copy answer, Save as note and the rating for an answer with sources", () => {
    mount([user, answer]);
    fireEvent.contextMenu(screen.getByRole("button", { name: "Copy answer" }), {
      clientX: 5,
      clientY: 5,
    });
    const menu = screen.getByRole("menu", { name: "Answer" });
    expect(menu.textContent).toContain("Copy answer");
    expect(screen.getByRole("menuitem", { name: "Save as note" })).toBeTruthy();
    expect(screen.getAllByRole("menuitemradio")).toHaveLength(2);
  });

  it("leaves the rating out where the answer shows none", () => {
    mount([bare]);
    fireEvent.contextMenu(screen.getByRole("button", { name: "Copy answer" }), {
      clientX: 5,
      clientY: 5,
    });
    expect(screen.getByRole("menuitem", { name: "Copy answer" })).toBeTruthy();
    expect(screen.queryAllByRole("menuitemradio")).toHaveLength(0);
  });

  it("Copy answer copies the text", () => {
    mount([bare]);
    fireEvent.contextMenu(screen.getByRole("button", { name: "Copy answer" }), {
      clientX: 5,
      clientY: 5,
    });
    fireEvent.click(screen.getByRole("menuitem", { name: "Copy answer" }));
    expect(writeText).toHaveBeenCalledWith("Ship on Friday.");
  });

  it("Save as note takes the button's own path, once, and the item goes while it is saved", async () => {
    mount([bare]);
    fireEvent.contextMenu(screen.getByRole("button", { name: "Copy answer" }), {
      clientX: 5,
      clientY: 5,
    });
    fireEvent.click(screen.getByRole("menuitem", { name: "Save as note" }));
    await waitFor(() => expect(ipc.ingestNote).toHaveBeenCalledTimes(1));
    expect(ipc.ingestNote).toHaveBeenCalledWith(
      "chat-3",
      "",
      expect.stringContaining("Ship on Friday."),
    );
    // The button shows the same state the menu started.
    await screen.findByText("Saved to notes ✓");
    fireEvent.contextMenu(screen.getByRole("button", { name: "Copy answer" }), {
      clientX: 5,
      clientY: 5,
    });
    expect(screen.queryByRole("menuitem", { name: "Save as note" })).toBeNull();
  });

  it("says aloud that a save from the menu worked, or that it didn't", async () => {
    mount([bare]);
    const status = saveStatus();
    expect(status.textContent).toBe("");
    fireEvent.contextMenu(screen.getByRole("button", { name: "Copy answer" }), {
      clientX: 5,
      clientY: 5,
    });
    fireEvent.click(screen.getByRole("menuitem", { name: "Save as note" }));
    await waitFor(() => expect(status.textContent).toBe("Saved to notes"));
  });

  it("says aloud when a save from the menu fails", async () => {
    ipc.ingestNote.mockRejectedValueOnce(new Error("disk full"));
    mount([bare]);
    fireEvent.contextMenu(screen.getByRole("button", { name: "Copy answer" }), {
      clientX: 5,
      clientY: 5,
    });
    fireEvent.click(screen.getByRole("menuitem", { name: "Save as note" }));
    await waitFor(() => expect(saveStatus().textContent).toBe("Couldn't save the answer"));
  });

  it("rates through the rating's own write, ticks it, and clears it when chosen again", async () => {
    mount([answer]);
    // The stored rating has been read (and is none) before anyone chooses.
    await act(async () => {});
    fireEvent.contextMenu(screen.getByRole("button", { name: "Copy answer" }), {
      clientX: 5,
      clientY: 5,
    });
    fireEvent.click(screen.getByRole("menuitemradio", { name: "This answer was helpful" }));
    await waitFor(() => expect(ipc.rateAnswer).toHaveBeenCalledWith(2, "up"));
    // The on-screen thumb now shows it pressed.
    expect(
      screen.getByRole("button", { name: "This answer was helpful" }).getAttribute("aria-pressed"),
    ).toBe("true");

    fireEvent.contextMenu(screen.getByRole("button", { name: "Copy answer" }), {
      clientX: 5,
      clientY: 5,
    });
    expect(
      screen
        .getByRole("menuitemradio", { name: "This answer was helpful" })
        .getAttribute("aria-checked"),
    ).toBe("true");
  });

  it("opens from the keyboard on the Copy button", () => {
    mount([bare]);
    const copy = screen.getByRole("button", { name: "Copy answer" });
    copy.focus();
    fireEvent.keyDown(copy, { key: "F10", shiftKey: true });
    expect(screen.getByRole("menu", { name: "Answer" })).toBeTruthy();
  });

  it("gives the user's own message no menu of this kind", () => {
    mount([user]);
    fireEvent.contextMenu(screen.getByText("what is the plan?"), { clientX: 5, clientY: 5 });
    expect(screen.queryByRole("menu", { name: "Answer" })).toBeNull();
  });
});
