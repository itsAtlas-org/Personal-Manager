// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The header title opens in edit mode from outside, as the sidebar row menu's Rename asks: focused and
// selected like a click, and the request is handed back once so a later remount can't reopen it.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../lib/ipc", () => ({ renameConversation: vi.fn(async (_id: number, t: string) => t) }));

import { ConversationTitle } from "./ConversationTitle";

afterEach(cleanup);

describe("ConversationTitle", () => {
  it("starts editing, focused, when asked to, and says it has", () => {
    const started = vi.fn();
    render(
      <ConversationTitle
        conversationId={1}
        title="Trip plan"
        onRenamed={() => {}}
        startEditing
        onEditStarted={started}
      />,
    );
    const input = screen.getByRole("textbox", { name: "Conversation title" }) as HTMLInputElement;
    expect(input.value).toBe("Trip plan");
    expect(document.activeElement).toBe(input);
    expect(started).toHaveBeenCalledOnce();
  });

  it("stays a button until asked", () => {
    render(<ConversationTitle conversationId={1} title="Trip plan" onRenamed={() => {}} />);
    expect(screen.getByRole("button", { name: "Trip plan" })).toBeTruthy();
    expect(screen.queryByRole("textbox")).toBeNull();
  });
});
