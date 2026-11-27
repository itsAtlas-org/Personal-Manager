// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A role on Anthropic. What is its own: Fable's warning stands under the model list after Fable is
// chosen (the picker draws a row's note only while it is open), is linked to the picker and spoken
// when the choice changes; Mythos never appears; effort offers Low to Max with the model's own
// default named; and Low on Haiku says it also switches thinking off.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const m = vi.hoisted(() => ({
  setRoleCloudProvider: vi.fn(),
  setDirectRole: vi.fn(),
  listModels: vi.fn(),
}));

vi.mock("../../lib/ipc", () => ({
  setRoleCloudProvider: (...a: unknown[]) => m.setRoleCloudProvider(...a),
  setDirectRole: (...a: unknown[]) => m.setDirectRole(...a),
  listModels: () => m.listModels(),
}));
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import {
  anthropicView,
  directState,
  FABLE,
  FLASH,
  FLASH_LITE,
  geminiView,
  HAIKU,
  PRO,
  OPUS,
  SONNET,
  stateWith,
} from "../../lib/directProvidersFixture";
import type { DirectProvidersState, RoleName } from "../../lib/types";
import { RoleBlock } from "./RoleBlock";

const NOTE = "Anthropic keeps what you send to Fable for 30 days.";
const next: { current: DirectProvidersState | null } = { current: null };

function Harness({ initial, role = "chat" }: { initial: DirectProvidersState; role?: RoleName }) {
  const [state, setState] = useState(initial);
  const dp = {
    state,
    error: null,
    reload: async () => {
      const n = next.current ?? state;
      setState(n);
      return n;
    },
  };
  return (
    <RoleBlock
      role={role}
      label={role === "chat" ? "Chat model" : "Background model"}
      description="OpenRouter's own words."
      helpId="settings-chat-models"
      dp={dp}
      routing="cloud"
      openRouter={{
        models: ["x/y"],
        onChange: () => {},
        autoSwitch: false,
        onAutoSwitchChange: () => {},
      }}
      onError={() => {}}
    />
  );
}

const onAnthropic = (chatModel: string, effort: string | null = null) =>
  stateWith(
    [
      anthropicView({
        roles: {
          chat: { uses_this_provider: true, model: chatModel, effort },
          background: { uses_this_provider: true, model: HAIKU.id, effort: "low" },
        },
      }),
    ],
    { chat: "anthropic", background: "anthropic" },
  );

beforeEach(() => {
  next.current = null;
  m.setRoleCloudProvider.mockResolvedValue(undefined);
  m.setDirectRole.mockResolvedValue(undefined);
  m.listModels.mockResolvedValue([]);
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("Fable's warning", () => {
  it("stands under the model list while Fable is the effective model", () => {
    render(<Harness initial={onAnthropic(FABLE.id)} />);
    expect(screen.getByText(NOTE)).toBeTruthy();
  });

  it("is absent for every other Anthropic model", () => {
    for (const id of [SONNET.id, OPUS.id, HAIKU.id]) {
      const { unmount } = render(<Harness initial={onAnthropic(id)} />);
      expect(screen.queryByText(NOTE), id).toBeNull();
      unmount();
    }
  });

  it("is linked to the picker, so focusing it reads the warning", () => {
    render(<Harness initial={onAnthropic(FABLE.id)} />);
    const line = screen.getByText(NOTE);
    const trigger = screen.getByRole("button", { name: "Choose a model for chat" });
    expect(line.id).not.toBe("");
    expect(trigger.getAttribute("aria-describedby")).toBe(line.id);
  });

  it("leaves the picker undescribed when there is nothing to say", () => {
    render(<Harness initial={onAnthropic(SONNET.id)} />);
    expect(
      screen
        .getByRole("button", { name: "Choose a model for chat" })
        .getAttribute("aria-describedby"),
    ).toBeNull();
  });

  it("is on the same line for the Background role", () => {
    const state = stateWith(
      [
        anthropicView({
          roles: {
            chat: { uses_this_provider: true, model: SONNET.id, effort: null },
            background: { uses_this_provider: true, model: FABLE.id, effort: "low" },
          },
        }),
      ],
      { chat: "anthropic", background: "anthropic" },
    );
    render(<Harness initial={state} role="background" />);
    expect(screen.getByText(NOTE)).toBeTruthy();
  });

  it("is spoken, once, when the choice changes to Fable, and the picker row carries it too", async () => {
    render(<Harness initial={onAnthropic(SONNET.id)} />);
    fireEvent.click(screen.getByRole("button", { name: "Choose a model for chat" }));
    // The row's own note, drawn while the list is open.
    expect(await screen.findByText(NOTE)).toBeTruthy();
    next.current = onAnthropic(FABLE.id);
    fireEvent.click(screen.getByText("Claude Fable 5.2"));
    await waitFor(() =>
      expect(m.setDirectRole).toHaveBeenCalledWith("anthropic", "chat", [FABLE.id], null),
    );
    await waitFor(() => {
      const regions = screen.getAllByRole("status");
      // Exactly one region speaks it, and not twice over.
      const speaking = regions.filter((r) => (r.textContent ?? "").includes(NOTE));
      expect(speaking.map((r) => r.textContent)).toEqual([`Claude Fable 5.2. ${NOTE}`]);
    });
    // The standing line is now there as well.
    expect(screen.getByText(NOTE).tagName).toBe("P");
  });

  it("never lists Mythos, even when a row for it reaches the view", () => {
    const base = onAnthropic(SONNET.id);
    const withMythos = {
      ...base,
      providers: base.providers.map((p) => ({
        ...p,
        models: [
          ...p.models,
          { ...SONNET, id: "anthropic/claude-mythos-5-2", name: "Claude Mythos 5.2" },
        ],
      })),
    };
    const { container } = render(<Harness initial={withMythos} />);
    fireEvent.click(screen.getByRole("button", { name: "Choose a model for chat" }));
    expect(container.textContent?.toLowerCase()).not.toContain("mythos");
    for (const name of [
      "Claude Fable 5.2",
      "Claude Opus 5.5",
      "Claude Sonnet 5.5",
      "Claude Haiku 5.5",
    ]) {
      expect(screen.getAllByText(name).length).toBeGreaterThan(0);
    }
  });
});

describe("A Gemini role's catalogue notes are not warnings", () => {
  const NOTE_G = "Gemini 4. Paid keys only. Thinks at medium by default.";
  const PREVIEW = "A preview model, so Google may retire it.";
  const onGemini = () =>
    stateWith(
      [
        geminiView({
          models: [{ ...FLASH, note: NOTE_G }, { ...FLASH_LITE, note: PREVIEW }, PRO],
          roles: {
            chat: { uses_this_provider: true, model: FLASH.id, effort: null },
            background: { uses_this_provider: true, model: FLASH_LITE.id, effort: "low" },
          },
        }),
      ],
      { chat: "gemini", background: "gemini" },
    );

  it("draws no standing line, links nothing to the picker and announces nothing", async () => {
    render(<Harness initial={onGemini()} />);
    expect(screen.queryByText(NOTE_G)).toBeNull();
    expect(
      screen
        .getByRole("button", { name: "Choose a model for chat" })
        .getAttribute("aria-describedby"),
    ).toBeNull();
    // The row's own note still shows inside the open picker, as it always did.
    fireEvent.click(screen.getByRole("button", { name: "Choose a model for chat" }));
    expect(await screen.findByText(new RegExp(NOTE_G))).toBeTruthy();
    // Changing the model to one with a note says nothing in the live region either.
    next.current = stateWith(
      [
        geminiView({
          models: [{ ...FLASH, note: NOTE_G }, { ...FLASH_LITE, note: PREVIEW }, PRO],
          roles: {
            chat: { uses_this_provider: true, model: FLASH_LITE.id, effort: null },
            background: { uses_this_provider: true, model: FLASH_LITE.id, effort: "low" },
          },
        }),
      ],
      { chat: "gemini", background: "gemini" },
    );
    fireEvent.click(screen.getByText(FLASH_LITE.name));
    await waitFor(() => expect(m.setDirectRole).toHaveBeenCalled());
    for (const r of screen.queryAllByRole("status")) {
      expect(r.textContent ?? "").not.toContain(PREVIEW);
      expect(r.textContent ?? "").not.toContain(NOTE_G);
    }
  });
});

describe("Anthropic's effort", () => {
  it("offers Low to Max for Opus, with the model default named", () => {
    render(<Harness initial={onAnthropic(OPUS.id)} />);
    const select = screen.getByLabelText("Chat effort") as HTMLSelectElement;
    expect(
      within(select)
        .getAllByRole("option")
        .map((o) => o.textContent),
    ).toEqual(["Model default (Medium)", "Low", "Medium", "High", "Extra high", "Max"]);
  });

  it("names High as Sonnet's and Fable's own default", () => {
    for (const id of [SONNET.id, FABLE.id]) {
      const { unmount } = render(<Harness initial={onAnthropic(id)} />);
      const select = screen.getByLabelText("Chat effort") as HTMLSelectElement;
      expect(within(select).getAllByRole("option")[0].textContent).toBe("Model default (High)");
      unmount();
    }
  });

  it("writes Extra high as it is, and the default as null", async () => {
    render(<Harness initial={onAnthropic(OPUS.id)} />);
    fireEvent.change(screen.getByLabelText("Chat effort"), { target: { value: "xhigh" } });
    await waitFor(() =>
      expect(m.setDirectRole).toHaveBeenCalledWith("anthropic", "chat", [OPUS.id], "xhigh"),
    );
  });

  it("says Low also switches Haiku's thinking off, for Background on Haiku only", () => {
    render(<Harness initial={onAnthropic(SONNET.id)} role="background" />);
    expect(
      screen.getByText(/Low keeps it quick and cheap\. Low also switches Haiku's thinking off\./),
    ).toBeTruthy();
    cleanup();
    // Chat on Haiku: no such line, chat never switches thinking off.
    render(<Harness initial={onAnthropic(HAIKU.id)} />);
    expect(screen.queryByText(/switches Haiku's thinking off/)).toBeNull();
    cleanup();
    // Background on Sonnet: its thinking is not touched.
    const state = stateWith(
      [
        anthropicView({
          roles: {
            chat: { uses_this_provider: true, model: SONNET.id, effort: null },
            background: { uses_this_provider: true, model: SONNET.id, effort: "low" },
          },
        }),
      ],
      { chat: "anthropic", background: "anthropic" },
    );
    render(<Harness initial={state} role="background" />);
    expect(screen.queryByText(/thinking off/)).toBeNull();
  });
});

describe("the notes under an Anthropic role", () => {
  it("says the route, and that mods can't use the key", () => {
    render(<Harness initial={onAnthropic(SONNET.id)} role="background" />);
    expect(
      screen.getByText(
        "Background work goes straight to Anthropic with your key and sends parts of your documents. It isn't zero data retention.",
      ),
    ).toBeTruthy();
    expect(
      screen.getByText(
        "Mods can't use your own keys. While Background is on Anthropic, a mod that asks for AI gets no answer.",
      ),
    ).toBeTruthy();
  });

  it("offers the provider beside OpenAI, labelled as direct with no zero data retention", () => {
    const state = stateWith([anthropicView()], { chat: "openrouter" });
    render(<Harness initial={state} />);
    const select = screen.getByLabelText("Chat provider") as HTMLSelectElement;
    expect(
      within(select).getByRole("option", { name: "Anthropic, direct (no zero data retention)" }),
    ).toBeTruthy();
    expect(directState({ view: anthropicView() }).providers[0].provider).toBe("anthropic");
  });
});
