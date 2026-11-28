// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// ModelPicker is a hand-rolled menu inside the Settings dialog (Settings › AI › "Add a model…"). Its
// Escape listener sits on `document` and the dialog's on `window`, so without stopping the key one
// Escape closed the menu AND Settings (or raised the unsaved-changes guard). It follows the Popover
// primitive's rule: a modal dialog over it owns the key, a non-modal layer only a key pressed in it.

import { useRef } from "react";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useDialogLayer } from "../lib/useDialogLayer";

const listModels = vi.hoisted(() => vi.fn());
vi.mock("../lib/ipc", () => ({
  listModels: () => listModels(),
}));

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

import { directPickerRows } from "../lib/directProviders";
import { openaiView } from "../lib/directProvidersFixture";
import { ModelPicker, type ModelSource } from "./ModelPicker";
import { Modal } from "./ui/Modal";

beforeEach(() => listModels.mockResolvedValue([]));
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("ModelPicker inside a dialog", () => {
  it("closes only itself on Escape", async () => {
    const onDialogClose = vi.fn();
    render(
      <Modal open onClose={onDialogClose} label="Settings">
        <ModelPicker value="" onChange={() => {}} triggerLabel="Add a model…" />
      </Modal>,
    );
    fireEvent.click(screen.getByText("Add a model…"));
    const search = await screen.findByPlaceholderText("Search models…");
    fireEvent.keyDown(search, { key: "Escape" });
    await waitFor(() => expect(screen.queryByPlaceholderText("Search models…")).toBeNull());
    expect(onDialogClose).not.toHaveBeenCalled();
    // With the menu closed, the next Escape reaches the dialog.
    fireEvent.keyDown(screen.getByText("Add a model…"), { key: "Escape" });
    expect(onDialogClose).toHaveBeenCalledTimes(1);
  });

  // The calendar's quick create stays open through a click in the sidebar, so it can sit behind
  // Settings: a non-modal layer, which owns only a key pressed inside it.
  it("closes only itself on Escape beside a non-modal layer", async () => {
    function Layer() {
      const ref = useRef<HTMLDivElement>(null);
      useDialogLayer(true, ref);
      return <div ref={ref} role="dialog" aria-label="New event" />;
    }
    const onDialogClose = vi.fn();
    render(
      <>
        <Layer />
        <Modal open onClose={onDialogClose} label="Settings">
          <ModelPicker value="" onChange={() => {}} triggerLabel="Add a model…" />
        </Modal>
      </>,
    );
    fireEvent.click(screen.getByText("Add a model…"));
    const search = await screen.findByPlaceholderText("Search models…");
    fireEvent.keyDown(search, { key: "Escape" });
    await waitFor(() => expect(screen.queryByPlaceholderText("Search models…")).toBeNull());
    expect(onDialogClose).not.toHaveBeenCalled();
    fireEvent.keyDown(screen.getByText("Add a model…"), { key: "Escape" });
    expect(onDialogClose).toHaveBeenCalledTimes(1);
  });

  // A modal dialog opened over the open menu (one of Settings' own questions) holds the keyboard.
  it("leaves Escape to a modal dialog opened over it", async () => {
    const onDialogClose = vi.fn();
    const onQuestionClose = vi.fn();
    const page = (asking: boolean) => (
      <Modal open onClose={onDialogClose} label="Settings">
        <ModelPicker value="" onChange={() => {}} triggerLabel="Add a model…" />
        {asking && (
          <Modal open onClose={onQuestionClose} label="Reset these settings?">
            <button type="button">Reset</button>
          </Modal>
        )}
      </Modal>
    );
    const { rerender } = render(page(false));
    fireEvent.click(screen.getByText("Add a model…"));
    await screen.findByPlaceholderText("Search models…");
    rerender(page(true));
    fireEvent.keyDown(screen.getByText("Reset"), { key: "Escape" });
    expect(onQuestionClose).toHaveBeenCalledTimes(1);
    expect(screen.queryByPlaceholderText("Search models…")).not.toBeNull();
    expect(onDialogClose).not.toHaveBeenCalled();
  });
});

// A model already in the person's list stays in the menu, reachable by keyboard and screen reader
// (`aria-disabled`, not `disabled`), and does nothing when pressed.
describe("ModelPicker rows already in the list", () => {
  it("keeps a taken row focusable, says why, and never picks it", async () => {
    listModels.mockResolvedValue([
      {
        id: "a/one",
        name: "One",
        description: "",
        context_length: 1000,
        prompt_price: 1,
        completion_price: 1,
        input_modalities: ["text"],
      },
      {
        id: "a/two",
        name: "Two",
        description: "",
        context_length: 1000,
        prompt_price: 1,
        completion_price: 1,
        input_modalities: ["text"],
      },
    ]);
    const onChange = vi.fn();
    render(<ModelPicker value="" onChange={onChange} disabledIds={["a/one"]} />);
    fireEvent.click(screen.getByRole("button", { name: "Choose a model…" }));
    const taken = (await screen.findByText("One")).closest("button")!;
    expect(taken.getAttribute("aria-disabled")).toBe("true");
    expect(taken.hasAttribute("disabled")).toBe(false);
    expect(taken.textContent).toContain("Already in your list");
    fireEvent.click(taken);
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText("Two").closest("button")!);
    expect(onChange).toHaveBeenCalledWith("a/two");
  });
});

// A person who only uses their own key hasn't opted into OpenRouter, so a direct list is handed in
// and the picker never asks OpenRouter for anything.
describe("ModelPicker with a provider's own list", () => {
  const view = openaiView({
    key_check: {
      at: "2026-11-26T10:00:00Z",
      outcome: "ok",
      listed: ["openai/gpt-6.2-sol"],
      listed_total: 40,
    },
  });
  const source: ModelSource = {
    kind: "direct",
    models: directPickerRows(view),
    caption: "Models your OpenAI key can use.",
  };

  it("never asks OpenRouter for its catalogue, however it is opened", async () => {
    render(<ModelPicker value="" onChange={() => {}} source={source} allowCustom={false} />);
    fireEvent.click(screen.getByRole("button", { name: "Choose a model…" }));
    await screen.findByText("GPT-6.2 Sol");
    expect(listModels).not.toHaveBeenCalled();
  });

  it("lists PM's own rows with the key's caption, and marks one the key's list leaves out", async () => {
    render(<ModelPicker value="" onChange={() => {}} source={source} allowCustom={false} />);
    fireEvent.click(screen.getByRole("button", { name: "Choose a model…" }));
    expect(await screen.findByText("Models your OpenAI key can use.")).toBeTruthy();
    expect(screen.getByText("GPT-6 Luna")).toBeTruthy();
    // Sol is in the key's list; Luna and Terra are not, and stay selectable.
    expect(screen.getAllByText("Not in your key's list")).toHaveLength(2);
  });

  it("takes no id typed by hand, and says why", async () => {
    render(<ModelPicker value="" onChange={() => {}} source={source} allowCustom={false} />);
    fireEvent.click(screen.getByRole("button", { name: "Choose a model…" }));
    fireEvent.change(await screen.findByPlaceholderText("Search models…"), {
      target: { value: "openai/made-up" },
    });
    expect(screen.queryByText(/Use custom model id/)).toBeNull();
    expect(screen.getByText("PM can only use models it knows the price of.")).toBeTruthy();
  });

  it("still takes a typed id on OpenRouter, as it always did", async () => {
    render(<ModelPicker value="" onChange={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Choose a model…" }));
    fireEvent.change(await screen.findByPlaceholderText("Search models…"), {
      target: { value: "vendor/some-model" },
    });
    expect(screen.getByText(/Use custom model id/)).toBeTruthy();
    expect(listModels).toHaveBeenCalledTimes(1);
  });
});

describe("ModelPicker as a disclosure", () => {
  it("says whether it is open, and gives focus back to the trigger on Escape and on a pick", async () => {
    const onChange = vi.fn();
    const source: ModelSource = {
      kind: "direct",
      models: directPickerRows(openaiView()),
      caption: "x",
    };
    render(
      <ModelPicker value="" onChange={onChange} source={source} triggerLabel="Add a model…" />,
    );
    const trigger = screen.getByRole("button", { name: "Add a model…" });
    expect(trigger.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(trigger);
    expect(trigger.getAttribute("aria-expanded")).toBe("true");
    const search = await screen.findByPlaceholderText("Search models…");
    fireEvent.keyDown(search, { key: "Escape" });
    await waitFor(() => expect(trigger.getAttribute("aria-expanded")).toBe("false"));
    expect(document.activeElement).toBe(trigger);

    fireEvent.click(trigger);
    fireEvent.click(await screen.findByText("GPT-6 Luna"));
    expect(onChange).toHaveBeenCalledWith("openai/gpt-6-luna");
    expect(document.activeElement).toBe(trigger);
  });

  it("keeps the visible words in a longer accessible name", () => {
    render(
      <ModelPicker
        value=""
        onChange={() => {}}
        triggerLabel="Add a model…"
        ariaLabel="Add a model for chat"
      />,
    );
    expect(screen.getByRole("button", { name: "Add a model for chat" })).toBeTruthy();
  });
});
