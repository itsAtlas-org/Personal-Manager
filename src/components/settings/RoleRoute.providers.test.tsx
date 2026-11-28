// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the old per-role provider block pinned, in the shape the list gives it: a Gemini row words its
// own terms, a paused provider's row says so in its status (the way out is the list's own Remove),
// effort lists are each model's own levels, Fable's warning stands under its row, and a Gemini note
// that is a catalogue fact is never a standing warning. Replaces RoleBlock.anthropic/gemini tests.

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../lib/ipc", () => ({
  routeAdd: vi.fn(),
  routeMove: vi.fn(),
  routeRemove: vi.fn(),
  routeSetEffort: vi.fn(),
  routeReset: vi.fn(),
  routeTryAgain: vi.fn(),
  listModels: vi.fn().mockResolvedValue([]),
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
  OPUS,
  PRO,
  SONNET,
  stateWith,
} from "../../lib/directProvidersFixture";
import { claudeRow, geminiRow, orRow, routeView } from "../../lib/routeFixture";
import type { DirectProvidersState, RoleName, RouteRow } from "../../lib/types";
import { RoleRoute } from "./RoleRoute";

afterEach(cleanup);

function show(
  entries: RouteRow[],
  dp: DirectProvidersState | null,
  role: RoleName = "chat",
  openRouterKey = true,
) {
  return render(
    <RoleRoute
      role={role}
      route={routeView(entries)}
      dp={dp}
      openRouterKey={openRouterKey}
      local={{ on: false, model: null }}
      helpId="settings-chat-models"
      onRoute={() => {}}
      onChanged={() => {}}
      onError={() => {}}
    />,
  );
}

const claude = (model: typeof SONNET, over: Partial<RouteRow> = {}) =>
  claudeRow({
    model: model.id,
    name: model.name,
    warning: model.note ?? undefined,
    effort_options: model.effort_levels,
    ...over,
  });

const optionsOf = (label: string) =>
  [...(screen.getByLabelText(label) as HTMLSelectElement).options].map((o) => o.textContent);

describe("Fable's warning", () => {
  it("stands under the row while Fable is listed, and is absent for every other Anthropic model", () => {
    show([claude(FABLE), claude(SONNET), orRow("a/one")], directState({ view: anthropicView() }));
    expect(screen.getAllByText(FABLE.note as string)).toHaveLength(1);
    const [fable, sonnet] = screen.getAllByRole("listitem");
    expect(within(fable).getByText(FABLE.note as string)).toBeTruthy();
    expect(within(sonnet).queryByText(FABLE.note as string)).toBeNull();
  });

  it("is tied to the row's own effort control, and an undescribed row has nothing to read", () => {
    show([claude(FABLE), claude(SONNET), orRow("a/one")], directState({ view: anthropicView() }));
    const note = screen.getByText(FABLE.note as string);
    expect(screen.getByLabelText(`Effort for ${FABLE.name}`).getAttribute("aria-describedby")).toBe(
      note.id,
    );
    expect(
      screen.getByLabelText(`Effort for ${SONNET.name}`).getAttribute("aria-describedby"),
    ).toBeNull();
  });

  it("is on the same line for the Background role", () => {
    show([claude(FABLE), orRow("a/one")], directState({ view: anthropicView() }), "background");
    expect(screen.getByText(FABLE.note as string)).toBeTruthy();
  });

  it("a Gemini note that is a catalogue fact is no standing line", () => {
    const noted = { ...FLASH, note: "Preview model." };
    const view = geminiView({ models: [noted, FLASH_LITE, PRO] });
    show([geminiRow({ warning: "Preview model." }), orRow("a/one")], stateWith([view]));
    expect(screen.queryByText("Preview model.")).toBeNull();
    expect(
      screen.getByLabelText(`Effort for ${FLASH.name}`).getAttribute("aria-describedby"),
    ).toBeNull();
  });
});

describe("a row's effort is its model's own levels", () => {
  it("offers Low to Max for Opus with the model default named, for Chat", () => {
    show([claude(OPUS), orRow("a/one")], directState({ view: anthropicView() }));
    expect(optionsOf(`Effort for ${OPUS.name}`)).toEqual([
      "Model default (Medium)",
      "Low",
      "Medium",
      "High",
      "Extra high",
      "Max",
    ]);
  });

  it("offers Background no model default, and marks PM's own", () => {
    show([claude(SONNET), orRow("a/one")], directState({ view: anthropicView() }), "background");
    const options = optionsOf(`Effort for ${SONNET.name}`);
    expect(options).not.toContain("Model default");
    expect(options).toContain("Low (PM's default)");
  });

  it("Gemini 3.8 Flash has no Minimal; Flash-Lite has, with Low as PM's default for Background", () => {
    const dp = stateWith([geminiView()]);
    show([geminiRow(), orRow("a/one")], dp);
    expect(optionsOf(`Effort for ${FLASH.name}`)).not.toContain("Minimal");
    cleanup();
    show(
      [
        geminiRow({
          model: FLASH_LITE.id,
          name: FLASH_LITE.name,
          effort_options: FLASH_LITE.effort_levels,
        }),
        orRow("a/one"),
      ],
      dp,
      "background",
    );
    const options = optionsOf(`Effort for ${FLASH_LITE.name}`);
    expect(options).toContain("Minimal");
    expect(options).toContain("Low (PM's default)");
  });

  it("says Low also switches Haiku's thinking off, for Background on Haiku only", () => {
    const dp = directState({ view: anthropicView() });
    show([claude(HAIKU), orRow("a/one")], dp, "background");
    expect(screen.getByText("Low also switches Haiku's thinking off.")).toBeTruthy();
    cleanup();
    show([claude(HAIKU), orRow("a/one")], dp, "chat");
    expect(screen.queryByText(/thinking off/)).toBeNull();
    cleanup();
    show([claude(SONNET), orRow("a/one")], dp, "background");
    expect(screen.queryByText(/thinking off/)).toBeNull();
  });
});

describe("a Gemini row words its own terms", () => {
  const free = geminiView({ tier: "free" });

  it("a paid key carries the paid headline, a free one says Google may train on what is sent", () => {
    show([geminiRow(), orRow("a/one")], stateWith([geminiView()]));
    expect(screen.getByText(/Sent straight to Google, not through OpenRouter/).textContent).toMatch(
      /This isn't zero data retention/,
    );
    cleanup();
    show([geminiRow(), orRow("a/one")], stateWith([free]));
    expect(screen.getByText(/Sent straight to Google/).textContent).toMatch(
      /Google may train on what you send/,
    );
  });

  it("a paused provider's row says so in words, and the row's own Remove is the way out", () => {
    show(
      [geminiRow({ status: { kind: "paused_region" } }), orRow("a/one")],
      stateWith([
        geminiView({
          tier: "free",
          blocked: {
            kind: "uk",
            label: "the United Kingdom",
            zone: "Europe/London",
            basis: "free",
          },
        }),
      ]),
    );
    const item = screen.getAllByRole("listitem")[0];
    expect(within(item).getByText("Paused where you are")).toBeTruthy();
    expect(within(item).getByRole("button", { name: `Remove ${FLASH.name}` })).toBeTruthy();
  });
});

describe("the picker's direct rows", () => {
  it("flags a paid-only model against a key that reads free, and keeps it selectable", () => {
    show([orRow("a/one")], stateWith([geminiView({ tier: "free" })]));
    fireEvent.click(screen.getByRole("button", { name: "Add a model to Chat" }));
    const dialog = screen.getByRole("dialog");
    const pro = within(dialog).getByRole("button", {
      name: /Gemini 3\.1 Pro/,
    }) as HTMLButtonElement;
    expect(pro.textContent).toContain("Needs a paid key");
    expect(pro.disabled).toBe(false);
    const flash = within(dialog).getByRole("button", { name: /Gemini 3\.8 Flash/ });
    expect(flash.textContent).not.toContain("Needs a paid key");
  });

  it("never lists Mythos, even when a row for it reaches the view", () => {
    const view = anthropicView({
      models: [
        ...anthropicView().models,
        { ...SONNET, id: "anthropic/claude-mythos-1", name: "Claude Mythos 1" },
      ],
    });
    show([orRow("a/one")], directState({ view }));
    fireEvent.click(screen.getByRole("button", { name: "Add a model to Chat" }));
    expect(within(screen.getByRole("dialog")).queryByText(/Mythos/)).toBeNull();
  });

  it("shows Fable's warning in its picker row", () => {
    show([orRow("a/one")], directState({ view: anthropicView() }));
    fireEvent.click(screen.getByRole("button", { name: "Add a model to Chat" }));
    const fable = within(screen.getByRole("dialog")).getByRole("button", {
      name: /Claude Fable 5\.2/,
    });
    expect(fable.textContent).toContain(FABLE.note as string);
  });
});
