// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the card's three buttons do, wired as the app wires them: an offer re-sends to exactly that
// model; "from now on" runs the CHAT setters in order and then re-sends as an ordinary send; one
// click at a time (a second one while the settings are being written must not send the reply twice);
// a failed setter shows the error and sends nothing.

import { act, renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { FromNowOnSetters } from "./notWorking";
import type { NotWorkingInfo, NotWorkingOffer } from "./types";
import type { NotWorkingState } from "./useChatStream";
import { useNotWorkingActions } from "./useNotWorkingActions";
import { useSendQueue } from "./useSendQueue";

const gemini: NotWorkingOffer = {
  key: "gemini:gemini/gemini-3.5-flash",
  provider: "gemini",
  model: "gemini/gemini-3.5-flash",
  label: "Gemini 3.5 Flash",
  company: "Google",
  data_note: "Google gets this reply.",
  kind: "other_company",
};

const info: NotWorkingInfo = {
  turn_id: 4,
  class: "busy",
  headline: "Claude isn't working right now",
  reason: "Anthropic is busy.",
  detail: null,
  reassurance: "Nothing was sent anywhere else.",
  offers: [gemini],
  can_edit_list: true,
  limit: null,
  tried: [{ provider: "anthropic", model: "anthropic/claude-sonnet-5-5" }],
};
const card: NotWorkingState = { convId: 3, text: "hello", info };

function setup(over: { setDirectRole?: FromNowOnSetters["setDirectRole"] } = {}) {
  const order: string[] = [];
  let release!: () => void;
  const gate = new Promise<void>((r) => (release = r));
  const setters: FromNowOnSetters = {
    setDirectRole:
      over.setDirectRole ??
      vi.fn(async (p, role) => {
        order.push(`directRole ${p} ${role}`);
        await gate;
      }),
    setRoleCloudProvider: vi.fn(async (role, p) => {
      order.push(`roleProvider ${role} ${p}`);
    }),
    setChatModels: vi.fn(async () => {}),
    setLocalLlmRouting: vi.fn(async () => {}),
    chatEffortFor: vi.fn(async () => null),
  };
  const resend = vi.fn(async () => true);
  const setError = vi.fn();
  const settled = vi.fn(async () => {});
  const focusComposer = vi.fn();
  const refreshSettings = vi.fn();
  const dropFailedSend = vi.fn();
  const hook = renderHook(() =>
    useNotWorkingActions({
      card,
      chatModels: [],
      setters,
      resend,
      setError,
      settled,
      refreshSettings,
      focusComposer,
      dropFailedSend,
    }),
  );
  return {
    hook,
    order,
    release,
    setters,
    resend,
    setError,
    settled,
    focusComposer,
    refreshSettings,
    dropFailedSend,
  };
}

describe("useNotWorkingActions", () => {
  it("an offer click re-sends this reply to exactly that model, once, and moves focus to the composer", async () => {
    const t = setup();
    await act(async () => {
      await t.hook.result.current.use(gemini, false);
    });
    expect(t.resend).toHaveBeenCalledTimes(1);
    expect(t.resend).toHaveBeenCalledWith(card, { provider: "gemini", model: gemini.model });
    expect(t.setters.setDirectRole).not.toHaveBeenCalled();
    expect(t.focusComposer).toHaveBeenCalledTimes(1);
    expect(t.settled).toHaveBeenCalledWith(3);
  });

  it("'from now on' sets the Chat role in order, then re-sends as an ordinary send (no override)", async () => {
    const t = setup();
    t.release();
    await act(async () => {
      await t.hook.result.current.use(gemini, true);
    });
    expect(t.order).toEqual(["directRole gemini chat", "roleProvider chat gemini"]);
    expect(t.refreshSettings).toHaveBeenCalled();
    expect(t.resend).toHaveBeenCalledTimes(1);
    expect(t.resend).toHaveBeenCalledWith(card, null);
  });

  it("a second click while the settings are being written sends nothing more", async () => {
    const t = setup();
    let first!: Promise<void>;
    await act(async () => {
      first = t.hook.result.current.use(gemini, true);
      // A double click, and a click on the other button of the same offer.
      void t.hook.result.current.use(gemini, true);
      void t.hook.result.current.use(gemini, false);
    });
    expect(t.hook.result.current.busy).toBe(true);
    expect(t.resend).not.toHaveBeenCalled();
    t.release();
    await act(async () => {
      await first;
    });
    expect(t.resend).toHaveBeenCalledTimes(1);
    expect(t.setters.setDirectRole).toHaveBeenCalledTimes(1);
    expect(t.hook.result.current.busy).toBe(false);
    // Once it has settled, the card (if it is still there) can be clicked again.
    await act(async () => {
      await t.hook.result.current.use(gemini, false);
    });
    expect(t.resend).toHaveBeenCalledTimes(2);
  });

  it("a setter that fails shows its error, sends nothing, and leaves focus alone", async () => {
    const t = setup({
      setDirectRole: vi.fn(async () => {
        throw new Error("Gemini is paused where you are.");
      }),
    });
    await act(async () => {
      await t.hook.result.current.use(gemini, true);
    });
    expect(t.setError).toHaveBeenCalledWith("Error: Gemini is paused where you are.");
    expect(t.resend).not.toHaveBeenCalled();
    expect(t.focusComposer).not.toHaveBeenCalled();
    expect(t.hook.result.current.busy).toBe(false);
  });

  it("does nothing when there is no card", async () => {
    const resend = vi.fn(async () => true);
    const hook = renderHook(() =>
      useNotWorkingActions({
        card: null,
        chatModels: [],
        setters: {} as FromNowOnSetters,
        resend,
        setError: vi.fn(),
        settled: vi.fn(async () => {}),
        refreshSettings: vi.fn(),
        focusComposer: vi.fn(),
        dropFailedSend: vi.fn(),
      }),
    );
    await act(async () => {
      await hook.result.current.use(gemini, false);
    });
    expect(resend).not.toHaveBeenCalled();
  });

  it("a refused 'from now on' keeps the failed message queued; a click that re-sends takes it off", async () => {
    const refused = setup({
      setDirectRole: vi.fn(async () => {
        throw new Error("no");
      }),
    });
    await act(async () => {
      await refused.hook.result.current.use(gemini, true);
    });
    expect(refused.dropFailedSend).not.toHaveBeenCalled();
    const t = setup();
    await act(async () => {
      await t.hook.result.current.use(gemini, false);
    });
    expect(t.dropFailedSend).toHaveBeenCalledTimes(1);
  });

  // The queue's own "Try again" and the next typed message both re-send the failed head. The card's
  // click already re-sends it, so the head has to be gone or the same question is paid for twice.
  describe.each([
    ["an offer click", false],
    ["'from now on'", true],
  ])("with the real send queue, after %s", (_name, fromNowOn) => {
    function wired() {
      const sent: string[] = [];
      const resend = vi.fn(async () => true);
      const hook = renderHook(() => {
        // The composer's send: it fails (a carded failure reports not-ok), as App's handleSend does.
        const queue = useSendQueue(async (text) => {
          sent.push(text);
          return false;
        });
        const actions = useNotWorkingActions({
          card,
          chatModels: [],
          setters: {
            setDirectRole: vi.fn(async () => {}),
            setRoleCloudProvider: vi.fn(async () => {}),
            setChatModels: vi.fn(async () => {}),
            setLocalLlmRouting: vi.fn(async () => {}),
            chatEffortFor: vi.fn(async () => null),
          },
          resend,
          setError: vi.fn(),
          settled: vi.fn(async () => {}),
          refreshSettings: vi.fn(),
          focusComposer: vi.fn(),
          dropFailedSend: () => {
            if (queue.failedId != null) queue.remove(queue.failedId);
          },
        });
        return { queue, actions };
      });
      return { hook, sent, resend };
    }

    it("leaves no stale 'Try again' chip, and a later message does not re-send the failed one", async () => {
      const w = wired();
      await act(async () => {
        w.hook.result.current.queue.enqueue("hello");
      });
      expect(w.hook.result.current.queue.stalled).toBe(true);
      expect(w.hook.result.current.queue.failedId).not.toBeNull();
      await act(async () => {
        await w.hook.result.current.actions.use(gemini, fromNowOn);
      });
      expect(w.hook.result.current.queue.failedId).toBeNull();
      expect(w.hook.result.current.queue.queued).toEqual([]);
      expect(w.hook.result.current.queue.stalled).toBe(false);
      // Typing something new now sends only that.
      await act(async () => {
        w.hook.result.current.queue.enqueue("next one");
      });
      expect(w.sent).toEqual(["hello", "next one"]);
    });
  });
});
