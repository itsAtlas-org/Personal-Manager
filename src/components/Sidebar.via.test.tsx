// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The sidebar's model footer never names a model without the route it takes: a role answered
// straight by a provider of the person's own says so beside the model, and a local model answering
// needs no route said at all.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  listProjects: () => Promise.resolve([]),
  listArchivedProjects: () => Promise.resolve([]),
  calendarOverview: () => Promise.resolve(null),
  onCalendarSynced: () => Promise.resolve(() => {}),
}));
vi.mock("../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    accent: "mono",
    depth: "standard",
    modelsVisible: true,
    colorblind: false,
  }),
}));
vi.mock("../lib/capabilities", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useDevMode: () => ({ devMode: false, setDevMode: () => {} }),
}));
vi.mock("./Briefing", () => ({ Briefing: () => null }));
// The sidebar's project menu archives through the shared hook, which needs the briefing; the footer
// under test never touches it.
vi.mock("../lib/briefing", () => ({
  useBriefing: () => ({ briefing: null, busy: false, refresh: async () => {} }),
}));

import type { LocalLlmStatus } from "../lib/types";
import { Sidebar } from "./Sidebar";

afterEach(cleanup);

function renderSidebar(props: Partial<Parameters<typeof Sidebar>[0]> = {}) {
  return render(
    <Sidebar
      view="focus"
      onNavigate={() => {}}
      conversations={[]}
      activeId={null}
      reviewCount={0}
      onSelect={() => {}}
      onDelete={() => {}}
      onMove={() => {}}
      onNew={() => {}}
      onNewProjectChat={() => {}}
      onOpenProject={() => {}}
      onOpenSettings={() => {}}
      onOpenWhatsNew={() => {}}
      onOpenPalette={() => {}}
      betterFit={false}
      sheetsNudge={false}
      chatModel="openai/gpt-6.2-sol"
      backgroundModel="google/gemini-2.5-flash"
      chatFallbacks={0}
      backgroundFallbacks={0}
      localAi={null}
      width={240}
      onStartResize={() => {}}
      resizing={false}
      {...props}
    />,
  );
}

describe("the model footer's route", () => {
  it("says OpenAI direct beside a role answered by the person's own key, and not beside the other", () => {
    renderSidebar({ chatVia: "OpenAI direct", backgroundVia: null });
    expect(screen.getAllByText("OpenAI direct")).toHaveLength(1);
    // The row's title carries the same words for a hover or a screen reader.
    expect(
      screen.getAllByTitle("openai/gpt-6.2-sol, straight to OpenAI with your own key"),
    ).toHaveLength(2);
  });

  it("says nothing extra on OpenRouter, as before", () => {
    renderSidebar({ chatModel: "anthropic/claude-sonnet-4.6" });
    expect(screen.queryByText("OpenAI direct")).toBeNull();
    expect(screen.getByTitle("anthropic/claude-sonnet-4.6")).toBeTruthy();
  });

  it("says nothing about the route while a local model is the one answering", () => {
    const local = {
      chat_local_model: "qwen3:8b",
      background_local_model: null,
    } as unknown as LocalLlmStatus;
    renderSidebar({ chatVia: "OpenAI direct", localAi: local });
    expect(screen.queryByText("OpenAI direct")).toBeNull();
  });

  it("shows no auto-switch count for a direct role, which has no fallbacks", () => {
    renderSidebar({ chatVia: "OpenAI direct", chatFallbacks: 0 });
    expect(screen.queryByText(/^\+\d/)).toBeNull();
  });
});
