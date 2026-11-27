// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The "not working" card: what it shows for each state, that every button is a real named button, that
// a click carries the offer it names, that nothing is sent by showing it, and that focus is not
// taken.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NotWorkingCard } from "./NotWorkingCard";
import type { NotWorkingInfo, NotWorkingOffer } from "../lib/types";

// Same stub the other component tests use: the <Button>s reach for `useTheme`, and the real
// ThemeProvider pulls in IPC.
vi.mock("../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    modePref: "system",
    modeSource: "system",
    accent: "mono",
    depth: "standard",
    autoLocation: "",
    teachVisible: true,
    setSystem: () => {},
    setModePref: () => {},
    setAccent: () => {},
    setDepth: () => {},
    setAutoLocation: () => {},
    setTeachVisible: () => {},
  }),
}));

const gemini: NotWorkingOffer = {
  key: "gemini:gemini/gemini-3.5-flash",
  provider: "gemini",
  model: "gemini/gemini-3.5-flash",
  label: "Gemini 3.5 Flash",
  company: "Google",
  data_note: "Google gets this reply. Google Gemini logs requests for a limited time.",
  kind: "other_company",
};
const local: NotWorkingOffer = {
  key: "local:llama3",
  provider: "local",
  model: "llama3",
  label: "llama3",
  company: "this computer",
  data_note: "Stays on this computer.",
  kind: "local",
};

function info(over: Partial<NotWorkingInfo> = {}): NotWorkingInfo {
  return {
    turn_id: 9,
    class: "no_credit",
    headline: "Claude Sonnet 5.5 isn't working right now",
    reason: "Your Anthropic key has no credit left or is at a spend limit.",
    detail: null,
    reassurance: "Nothing was sent anywhere else.",
    offers: [gemini, local],
    can_edit_list: true,
    limit: null,
    tried: [{ provider: "anthropic", model: "anthropic/claude-sonnet-5-5" }],
    ...over,
  };
}

function setup(i: NotWorkingInfo, busy = false) {
  const h = {
    onUse: vi.fn(),
    onUseFromNowOn: vi.fn(),
    onEditList: vi.fn(),
    onRaiseLimit: vi.fn(),
  };
  const view = render(<NotWorkingCard info={i} busy={busy} {...h} />);
  return { ...h, ...view };
}

afterEach(cleanup);

describe("NotWorkingCard", () => {
  it("is a group named by its headline, with the reason, the reassurance and each offer", () => {
    setup(info());
    const group = screen.getByRole("group", { name: "Claude Sonnet 5.5 isn't working right now" });
    expect(group).toBeTruthy();
    expect(screen.getByText(/has no credit left/)).toBeTruthy();
    expect(screen.getByText("Nothing was sent anywhere else.")).toBeTruthy();
    expect(screen.getByText("Use something else for this reply:")).toBeTruthy();
    // Names carry the model and who gets the reply.
    expect(
      screen.getByRole("button", { name: "Gemini 3.5 Flash (Google, your key)" }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "llama3 (this computer)" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Use Gemini 3.5 Flash from now on" })).toBeTruthy();
  });

  it("describes each offer button by its data note, so a screen reader hears it with the button", () => {
    setup(info());
    const b = screen.getByRole("button", { name: "Gemini 3.5 Flash (Google, your key)" });
    const note = document.getElementById(b.getAttribute("aria-describedby") ?? "");
    expect(note?.textContent).toBe(
      "Google gets this reply. Google Gemini logs requests for a limited time.",
    );
  });

  it("says nothing it can't stand behind: no reassurance line when the failure has none", () => {
    setup(info({ reassurance: null, class: "refused" }));
    expect(screen.queryByText(/anywhere else/)).toBeNull();
    expect(screen.getByText("Rephrase it, or ask another model:")).toBeTruthy();
  });

  it("a click carries the offer it names, and 'from now on' is its own button", () => {
    const h = setup(info());
    fireEvent.click(screen.getByRole("button", { name: "llama3 (this computer)" }));
    expect(h.onUse).toHaveBeenCalledWith(local);
    fireEvent.click(screen.getByRole("button", { name: "Use Gemini 3.5 Flash from now on" }));
    expect(h.onUseFromNowOn).toHaveBeenCalledWith(gemini);
    expect(h.onUse).toHaveBeenCalledTimes(1);
  });

  it("shows nothing sent by itself: rendering calls no handler", () => {
    const h = setup(info());
    expect(h.onUse).not.toHaveBeenCalled();
    expect(h.onUseFromNowOn).not.toHaveBeenCalled();
  });

  it("waits while a re-send is under way", () => {
    setup(info(), true);
    for (const name of [
      "Gemini 3.5 Flash (Google, your key)",
      "llama3 (this computer)",
      "Use Gemini 3.5 Flash from now on",
    ]) {
      expect((screen.getByRole("button", { name }) as HTMLButtonElement).disabled).toBe(true);
    }
  });

  it("opens the list, and for a limit says the figures and offers to raise it", () => {
    const h = setup(
      info({
        class: "limit_reached",
        reason: "Anthropic is paused by your daily limit.",
        limit: {
          provider: "anthropic",
          limit_usd: 5,
          spent_today_usd: 5.12,
          resumes_at: "2026-11-28T00:00:00+00:00",
        },
      }),
    );
    expect(screen.getByText(/at least \$5\.12 of your \$5\.00 limit/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Raise your limit" }));
    expect(h.onRaiseLimit).toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Edit my list" }));
    expect(h.onEditList).toHaveBeenCalled();
  });

  it("has no 'Raise your limit' unless a limit stopped it, and no buttons list with no offers", () => {
    setup(info({ offers: [] }));
    expect(screen.queryByRole("button", { name: "Raise your limit" })).toBeNull();
    expect(screen.queryByRole("list")).toBeNull();
    expect(screen.queryByText("Use something else for this reply:")).toBeNull();
    expect(screen.getByRole("button", { name: "Edit my list" })).toBeTruthy();
  });

  // jsdom flushes the mount effect inside render, so "empty on the first paint, filled after" can't be
  // observed here: this pins the region (polite, a status) and its final sentence, and the component
  // sets the sentence from an effect precisely so a real browser sees it as a change.
  it("announces itself politely once it is on screen, and leaves focus where it was", () => {
    const composer = document.createElement("textarea");
    document.body.append(composer);
    composer.focus();
    setup(info());
    const status = screen.getByRole("status");
    expect(status.textContent).toBe("Claude Sonnet 5.5 isn't working. 2 options.");
    expect(status.getAttribute("aria-live")).toBe("polite");
    expect(document.activeElement).toBe(composer);
    composer.remove();
  });

  it("describes 'from now on' by the note AND by who Chat will use, the heaviest button on the card", () => {
    setup(info());
    const b = screen.getByRole("button", { name: "Use Gemini 3.5 Flash from now on" });
    const text = (b.getAttribute("aria-describedby") ?? "")
      .split(" ")
      .map((id) => document.getElementById(id)?.textContent)
      .join(" | ");
    expect(text).toContain("Chat will use Google from now on.");
    expect(text).toContain("Google gets this reply.");
    const l = screen.getByRole("button", { name: "Use llama3 from now on" });
    const lt = (l.getAttribute("aria-describedby") ?? "")
      .split(" ")
      .map((id) => document.getElementById(id)?.textContent)
      .join(" | ");
    expect(lt).toContain("Chat will use this computer from now on.");
  });

  it("takes focus on its heading only when it appears after a click, and not otherwise", () => {
    const composer = document.createElement("textarea");
    document.body.append(composer);
    composer.focus();
    const view = render(
      <NotWorkingCard
        info={info()}
        busy={false}
        focusOnMount
        onUse={() => {}}
        onUseFromNowOn={() => {}}
        onEditList={() => {}}
        onRaiseLimit={() => {}}
      />,
    );
    expect(document.activeElement).toBe(
      screen.getByRole("heading", { name: "Claude Sonnet 5.5 isn't working right now" }),
    );
    view.unmount();
    composer.focus();
    setup(info());
    expect(document.activeElement).toBe(composer);
    composer.remove();
  });

  it("keeps the provider's own sentence as a muted line under the reason", () => {
    setup(info({ detail: "Add credit at aistudio.google.com/billing, then try again." }));
    expect(
      screen.getByText("Add credit at aistudio.google.com/billing, then try again."),
    ).toBeTruthy();
    cleanup();
    setup(info({ detail: null }));
    expect(screen.queryByText(/aistudio/)).toBeNull();
  });

  it("a Local AI server on another machine is named as that, never as this computer", () => {
    const remote: NotWorkingOffer = {
      ...local,
      company: "your Local AI server",
      data_note: "Goes to your Local AI server.",
    };
    setup(info({ offers: [remote] }));
    expect(screen.getByRole("button", { name: "llama3 (your Local AI server)" })).toBeTruthy();
    const group = screen.getByRole("group");
    expect(group.textContent).not.toContain("this computer");
    expect(
      document.getElementById(
        screen
          .getByRole("button", { name: "Use llama3 from now on" })
          .getAttribute("aria-describedby")
          ?.split(" ")[0] ?? "",
      )?.textContent,
    ).toBe("Chat will use your Local AI server from now on.");
  });

  it("scrolls inside itself instead of pushing the composer off a small screen", () => {
    setup(info());
    const group = screen.getByRole("group");
    expect(group.className).toContain("max-h-[50vh]");
    expect(group.className).toContain("overflow-y-auto");
  });

  it("a refusal's card announces what happened, not that the model is down", () => {
    setup(
      info({
        class: "refused",
        headline: "Claude Sonnet 5.5 didn't answer this one",
        reassurance: null,
      }),
    );
    expect(screen.getByRole("status").textContent).toBe(
      "Claude Sonnet 5.5 didn't answer this one. 2 options.",
    );
  });
});
