// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The "not working" card's state in the chat stream: the event sets it instead of an error, the
// rejection that follows does not replace it, a re-send drops the failed message and sends the same
// text once with the override, and the next send or a conversation switch clears it.

import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ChatEvent, EntryRef, Message, NotWorkingInfo } from "./types";

const h = vi.hoisted(() => ({
  onEvent: null as ((e: ChatEvent) => void) | null,
  resolve: null as (() => void) | null,
  reject: null as ((e: unknown) => void) | null,
}));

vi.mock("./capabilities", () => ({ useDevMode: () => ({ devMode: false }) }));
vi.mock("./ipc", () => ({
  sendMessage: vi.fn((_c: number, _t: string, onEvent: (e: ChatEvent) => void) => {
    h.onEvent = onEvent;
    return new Promise<void>((res, rej) => {
      h.resolve = res;
      h.reject = rej;
    });
  }),
}));

import { sendMessage } from "./ipc";
import { useChatStream } from "./useChatStream";

const card: NotWorkingInfo = {
  turn_id: 4,
  class: "busy",
  headline: "Claude isn't working right now",
  reason: "Anthropic is busy.",
  detail: null,
  reassurance: "Nothing was sent anywhere else.",
  offers: [],
  can_edit_list: true,
  limit: null,
  tried: [{ provider: "anthropic", model: "anthropic/claude-sonnet-5-5" }],
};

describe("useChatStream and the not-working card", () => {
  beforeEach(() => {
    h.onEvent = null;
    vi.mocked(sendMessage).mockClear();
  });

  it("holds the card instead of an error, and the rejection that follows does not replace it", async () => {
    const { result } = renderHook(() => useChatStream(() => 1));
    let sent!: Promise<boolean>;
    act(() => {
      sent = result.current.send(1, "hi");
    });
    act(() => h.onEvent!({ type: "not_working", ...card }));
    await act(async () => {
      h.reject!("Anthropic is having trouble right now. Try again shortly.");
      expect(await sent).toBe(false);
    });
    expect(result.current.notWorking).toEqual({ convId: 1, text: "hi", info: card });
    expect(result.current.error).toBeNull();
  });

  it("still shows a plain error when the send is refused before any card (a refused override)", async () => {
    const { result } = renderHook(() => useChatStream(() => 1));
    let sent!: Promise<boolean>;
    act(() => {
      sent = result.current.send(1, "hi");
    });
    await act(async () => {
      h.reject!("Gemini is paused: ...");
      await sent;
    });
    expect(result.current.error).toBe("Gemini is paused: ...");
    expect(result.current.notWorking).toBeNull();
  });

  it("a re-send takes the failed message off, sends the same text once, and passes the override", async () => {
    const { result } = renderHook(() => useChatStream(() => 1));
    let sent!: Promise<boolean>;
    act(() => {
      sent = result.current.send(1, "hi");
    });
    act(() => h.onEvent!({ type: "not_working", ...card }));
    await act(async () => {
      h.reject!("x");
      await sent;
    });
    // What the page holds after a failure: the persisted, unanswered user turn.
    const failed: Message = {
      id: 77,
      conversation_id: 1,
      role: "user",
      content: "hi",
      model: null,
      created_at: "",
    };
    act(() => result.current.setMessages([failed]));
    const state = result.current.notWorking!;
    const override: EntryRef = { provider: "gemini", model: "gemini/gemini-3.5-flash" };
    act(() => {
      void result.current.resend(state, override);
    });
    expect(result.current.notWorking).toBeNull();
    const user = result.current.messages.filter((m) => m.role === "user");
    expect(user).toHaveLength(1);
    expect(user[0].id).toBeLessThan(0);
    const calls = vi.mocked(sendMessage).mock.calls;
    const call = calls[calls.length - 1];
    expect(call[0]).toBe(1);
    expect(call[1]).toBe("hi");
    expect(call[5]).toEqual(override);
    // What the card said was already tried is handed back, so the next card can leave it out.
    expect(call[6]).toEqual(card.tried);
  });

  it("a click the backend refuses brings the card back, without the entry that was refused", async () => {
    const gemini = {
      key: "gemini:g",
      provider: "gemini" as const,
      model: "g",
      label: "G",
      company: "Google",
      data_note: "",
      kind: "other_company" as const,
    };
    const local = { ...gemini, key: "local:l", provider: "local" as const, model: "l" };
    const { result } = renderHook(() => useChatStream(() => 1));
    let sent!: Promise<boolean>;
    act(() => {
      sent = result.current.send(1, "hi");
    });
    act(() => h.onEvent!({ type: "not_working", ...card, offers: [gemini, local] }));
    await act(async () => {
      h.reject!("x");
      await sent;
    });
    const state = result.current.notWorking!;
    let resent!: Promise<boolean>;
    act(() => {
      resent = result.current.resend(state, { provider: "gemini", model: "g" });
    });
    expect(result.current.notWorking).toBeNull();
    // Refused before anything was sent: a plain rejection, no card event.
    await act(async () => {
      h.reject!("Gemini is paused where you are.");
      await resent;
    });
    expect(result.current.error).toBe("Gemini is paused where you are.");
    const back = result.current.notWorking!;
    expect(back.info.offers.map((o) => o.key)).toEqual(["local:l"]);
    expect(back.info.tried).toEqual([...card.tried, { provider: "gemini", model: "g" }]);
  });

  it("a re-send that fails with a card of its own shows that card, not the old one", async () => {
    const { result } = renderHook(() => useChatStream(() => 1));
    let sent!: Promise<boolean>;
    act(() => {
      sent = result.current.send(1, "hi");
    });
    act(() => h.onEvent!({ type: "not_working", ...card }));
    await act(async () => {
      h.reject!("x");
      await sent;
    });
    const state = result.current.notWorking!;
    let resent!: Promise<boolean>;
    act(() => {
      resent = result.current.resend(state, { provider: "gemini", model: "g" });
    });
    act(() =>
      h.onEvent!({
        type: "not_working",
        ...card,
        headline: "G isn't working right now",
        tried: [...card.tried, { provider: "gemini", model: "g" }],
      }),
    );
    await act(async () => {
      h.reject!("x");
      await resent;
    });
    expect(result.current.notWorking!.info.headline).toBe("G isn't working right now");
    expect(result.current.notWorking!.info.tried).toHaveLength(2);
  });

  it("an ordinary send passes no override, and the next send clears a card", async () => {
    const { result } = renderHook(() => useChatStream(() => 1));
    act(() => {
      void result.current.send(1, "first");
    });
    act(() => h.onEvent!({ type: "not_working", ...card }));
    expect(result.current.notWorking).not.toBeNull();
    act(() => {
      void result.current.send(1, "second");
    });
    expect(result.current.notWorking).toBeNull();
    const calls = vi.mocked(sendMessage).mock.calls;
    expect(calls[calls.length - 1][5]).toBeNull();
  });

  it("a card for a conversation the person has left is dropped, like any other event", () => {
    let current = 1;
    const { result } = renderHook(() => useChatStream(() => current));
    act(() => {
      void result.current.send(1, "hi");
    });
    current = 2;
    act(() => h.onEvent!({ type: "not_working", ...card }));
    expect(result.current.notWorking).toBeNull();
  });

  it("clearing the transient state clears the card", () => {
    const { result } = renderHook(() => useChatStream(() => 1));
    act(() => {
      void result.current.send(1, "hi");
    });
    act(() => h.onEvent!({ type: "not_working", ...card }));
    act(() => result.current.clearTransient());
    expect(result.current.notWorking).toBeNull();
  });
});
