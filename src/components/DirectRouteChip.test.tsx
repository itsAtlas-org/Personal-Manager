// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The composer's "OpenAI direct" chip: the one place chat says, where the person is typing, that
// this reply goes straight to a provider and isn't zero data retention. It must say nothing for a
// person who never turned a key on, nothing while a local model is the one answering, and must
// never flash "no key" before PM has actually read whether the key is there.

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const m = vi.hoisted(() => ({ directProvidersState: vi.fn(), routeStatus: vi.fn() }));

vi.mock("../lib/ipc", () => ({
  directProvidersState: () => m.directProvidersState(),
  routeStatus: () => m.routeStatus(),
  onRouteChanged: () => Promise.resolve(() => {}),
}));

import { directState, openaiView, settingsFor } from "../lib/directProvidersFixture";
import { claudeRow, geminiRow, orRow, routeView } from "../lib/routeFixture";
import type { LocalLlmStatus } from "../lib/types";
import { DirectRouteChip } from "./DirectRouteChip";

const direct = settingsFor({ chat_provider: "openai", chat_cloud_model: "openai/gpt-6.2-sol" });
const localAnswering = { chat_local_model: "qwen3:8b" } as unknown as LocalLlmStatus;
const localIdle = { chat_local_model: null } as unknown as LocalLlmStatus;

beforeEach(() => {
  m.directProvidersState.mockResolvedValue(directState({ chat: "openai" }));
  // A role with one model: nobody behind the head, so the chip is what it always was.
  m.routeStatus.mockResolvedValue({
    chat: routeView([orRow("a/one")], true),
    background: routeView([orRow("a/one")], true),
  });
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("the composer chip", () => {
  it("renders nothing on OpenRouter, and does not even read the provider state", () => {
    const { container } = render(<DirectRouteChip settings={settingsFor()} localAi={null} />);
    expect(container.textContent).toBe("");
    expect(m.directProvidersState).not.toHaveBeenCalled();
  });

  it("renders nothing before settings have loaded", () => {
    const { container } = render(<DirectRouteChip settings={null} localAi={null} />);
    expect(container.textContent).toBe("");
  });

  it("says OpenAI direct, with what that means in its title", async () => {
    render(<DirectRouteChip settings={direct} localAi={localIdle} />);
    const chip = await screen.findByText("OpenAI direct");
    expect(chip.getAttribute("title")).toBe(
      "Chat goes straight to OpenAI with your own key. It isn't zero data retention. Change it in Settings > AI & Models.",
    );
  });

  it("wears the due tint and says no credit when the account has none", async () => {
    m.directProvidersState.mockResolvedValue(
      directState({
        view: openaiView({
          key_check: { at: "x", outcome: "no_credit", listed: [], listed_total: 0 },
        }),
        chat: "openai",
      }),
    );
    render(<DirectRouteChip settings={direct} localAi={null} />);
    const chip = await screen.findByText("OpenAI: no credit");
    expect(chip.getAttribute("title")).toBe(
      "Your OpenAI account is out of credit or at a spend limit, so chat won't answer until you add credit or raise the limit. Then press Check again in Settings > AI & Models.",
    );
    expect(chip.getAttribute("style")).toContain("--st-due");
  });

  it("says shared when the person ticked that their project shares data", async () => {
    m.directProvidersState.mockResolvedValue(
      directState({ view: openaiView({ shares_data: true }), chat: "openai" }),
    );
    render(<DirectRouteChip settings={direct} localAi={null} />);
    expect(await screen.findByText("OpenAI direct, shared")).toBeTruthy();
  });

  it("is hidden while a local model is the one answering chat", () => {
    const { container } = render(<DirectRouteChip settings={direct} localAi={localAnswering} />);
    expect(container.textContent).toBe("");
  });

  it("says there is no key, in the due tint, once PM has read that the key is not there", async () => {
    m.directProvidersState.mockResolvedValue(
      directState({ view: openaiView({ key: "absent", key_check: null }), chat: "openai" }),
    );
    render(<DirectRouteChip settings={direct} localAi={null} />);
    const chip = await screen.findByText("OpenAI: no key");
    expect(chip.getAttribute("title")).toContain("Chat can't answer until you fix this.");
    expect(chip.getAttribute("style")).toContain("--st-due");
  });

  it("says the key can't be read, not that it is missing, when the keychain is unreadable", async () => {
    m.directProvidersState.mockResolvedValue(
      directState({ view: openaiView({ key: "unreadable", key_check: null }), chat: "openai" }),
    );
    render(<DirectRouteChip settings={direct} localAi={null} />);
    const chip = await screen.findByText("OpenAI: key unreadable");
    expect(chip.getAttribute("title")).toContain("can't read your key");
    expect(chip.getAttribute("title")).not.toContain("no key to use");
    expect(chip.getAttribute("style")).toContain("--st-due");
  });

  it("says the first row isn't working and which is next, instead of a stale healthy chip", async () => {
    m.routeStatus.mockResolvedValue({
      chat: routeView([
        claudeRow({ status: { kind: "not_working", reason: "busy" } }),
        geminiRow(),
      ]),
      background: routeView([orRow("a/one")], true),
    });
    render(<DirectRouteChip settings={direct} localAi={localIdle} />);
    const chip = await screen.findByText("Claude isn't working, Gemini is next");
    expect(chip.getAttribute("style")).toContain("--st-due");
    // The healthy label is gone: the chip says one thing.
    expect(screen.queryByText("OpenAI direct")).toBeNull();
  });

  it("says it on OpenRouter too, and says nothing when no later row can answer", async () => {
    m.routeStatus.mockResolvedValue({
      chat: routeView([
        orRow("a/one", { status: { kind: "not_working" } }),
        claudeRow({ status: { kind: "no_key" } }),
      ]),
      background: routeView([orRow("a/one")], true),
    });
    const { container } = render(<DirectRouteChip settings={settingsFor()} localAi={null} />);
    await waitFor(() => expect(m.routeStatus).toHaveBeenCalled());
    expect(container.textContent).toBe("");
    cleanup();
    m.routeStatus.mockResolvedValue({
      chat: routeView([orRow("a/one", { status: { kind: "not_working" } }), geminiRow()]),
      background: routeView([orRow("a/one")], true),
    });
    render(<DirectRouteChip settings={settingsFor()} localAi={null} />);
    expect(await screen.findByText("OpenRouter isn't working, Gemini is next")).toBeTruthy();
  });

  it("says nothing at all until the read lands, not a flash of no key", async () => {
    let finish: (s: unknown) => void = () => {};
    m.directProvidersState.mockReturnValue(new Promise((r) => (finish = r)));
    const { container } = render(<DirectRouteChip settings={direct} localAi={null} />);
    expect(container.textContent).toBe("");
    finish(directState({ chat: "openai" }));
    await waitFor(() => expect(container.textContent).toBe("OpenAI direct"));
  });
});
