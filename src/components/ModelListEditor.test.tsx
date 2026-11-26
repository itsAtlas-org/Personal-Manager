// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The model list editor, for both of its callers: OpenRouter's several-model list with auto-switch,
// and a direct provider's single model, which has no order, no fallbacks and nothing to switch to.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../lib/ipc", () => ({ listModels: () => Promise.resolve([]) }));
vi.mock("../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { directPickerRows } from "../lib/directProviders";
import { openaiView } from "../lib/directProvidersFixture";
import { ModelListEditor } from "./ModelListEditor";

afterEach(cleanup);

const base = {
  label: "Chat model",
  description: "Answers your chats.",
  autoSwitch: true,
  onAutoSwitchChange: () => {},
};

describe("a list on OpenRouter", () => {
  it("orders, removes and appends as before, with accessible names that say which model", () => {
    const onChange = vi.fn();
    render(
      <ModelListEditor {...base} models={["a/one", "b/two"]} onChange={onChange} roleName="chat" />,
    );
    expect(screen.getByText("fallback 1")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Move b/two up" }));
    expect(onChange).toHaveBeenLastCalledWith(["b/two", "a/one"]);
    fireEvent.click(screen.getByRole("button", { name: "Remove a/one" }));
    expect(onChange).toHaveBeenLastCalledWith(["b/two"]);
    expect(screen.getByRole("switch", { name: "Auto-switch on limit" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Add a model for chat" })).toBeTruthy();
  });

  it("gives the two editors on one page two different trigger names", () => {
    render(
      <>
        <ModelListEditor {...base} models={[]} onChange={() => {}} roleName="chat" />
        <ModelListEditor
          {...base}
          label="Background model"
          models={[]}
          onChange={() => {}}
          roleName="background"
        />
      </>,
    );
    expect(screen.getByRole("button", { name: "Add a model for chat" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Add a model for background" })).toBeTruthy();
  });
});

describe("a single-model list", () => {
  const source = {
    kind: "direct" as const,
    models: directPickerRows(openaiView()),
    caption: "Models your OpenAI key can use.",
  };

  it("replaces the model on a pick, rather than appending a fallback", async () => {
    const onChange = vi.fn();
    render(
      <ModelListEditor
        {...base}
        models={["openai/gpt-6.2-sol"]}
        onChange={onChange}
        source={source}
        maxModels={1}
        showAutoSwitch={false}
        allowCustom={false}
        roleName="chat"
        nameFor={(id) => (id === "openai/gpt-6.2-sol" ? "GPT-6.2 Sol" : undefined)}
      />,
    );
    // The row shows the friendly name, with the id as its tooltip.
    expect(screen.getByTitle("openai/gpt-6.2-sol").textContent).toBe("GPT-6.2 Sol");
    fireEvent.click(screen.getByRole("button", { name: "Choose a model for chat" }));
    fireEvent.click(await screen.findByText("GPT-6 Luna"));
    expect(onChange).toHaveBeenCalledWith(["openai/gpt-6-luna"]);
  });

  it("draws no order, no remove, no fallback label, no auto-switch and no auto-switch paragraph", () => {
    render(
      <ModelListEditor
        {...base}
        models={["openai/gpt-6.2-sol"]}
        onChange={() => {}}
        source={source}
        maxModels={1}
        showAutoSwitch={false}
        allowCustom={false}
      />,
    );
    expect(screen.queryByRole("button", { name: /^Move / })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Remove / })).toBeNull();
    expect(screen.queryByText(/fallback/)).toBeNull();
    expect(screen.queryByRole("switch")).toBeNull();
    fireEvent.click(screen.getByText("What this model does"));
    expect(screen.queryByText(/auto-switch falls through/)).toBeNull();
  });
});
