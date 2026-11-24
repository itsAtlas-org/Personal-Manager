// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The first-use question, and its money form for `ai` (spec §1.5, §1.6): the 500 ms hold, which
// restarts whenever the content under the pointer changes; the route facts the backend gives; the
// monthly limit field that rides with the Allow on a route that can cost money; and what each
// answer sends. The sentences of the money form are placeholders (aiMoneyWords), so these tests look
// for facts and fields, not for the final words.

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { AiPromptInfo } from "../../lib/modsTypes";

const modAiPromptInfo = vi.fn();
vi.mock("../../lib/ipc", () => ({
  modAiPromptInfo: (modId: string) => modAiPromptInfo(modId),
}));
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { ModPermissionPrompt, PROMPT_GUARD_MS } from "./ModPermissionPrompt";

const info = (extra: Partial<AiPromptInfo> = {}): AiPromptInfo => ({
  route: "cloud",
  paid: true,
  unpriced: false,
  cloudModels: ["a/cheap"],
  localModel: null,
  suggestedCapUsd: 1,
  ...extra,
});

const onAnswer = vi.fn();
const onLater = vi.fn();

function draw(permission: string, extra: { working?: boolean; tier?: string } = {}) {
  return render(
    <ModPermissionPrompt
      modId="acme.tool"
      name="Acme Tool"
      tier={extra.tier ?? "second"}
      permission={permission}
      working={extra.working ?? false}
      error={null}
      onAnswer={onAnswer}
      onLater={onLater}
    />,
  );
}

const button = (name: string) => screen.getByRole("button", { name }) as HTMLButtonElement;
const waitOut = () => act(() => void vi.advanceTimersByTime(PROMPT_GUARD_MS));
/** Let the pending ipc promise settle inside React's act. */
const flush = () => act(async () => void (await Promise.resolve()));

beforeEach(() => {
  vi.useFakeTimers();
  modAiPromptInfo.mockReset();
  onAnswer.mockReset();
  onLater.mockReset();
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("an ordinary question", () => {
  it("holds Allow and No for the guard, never Not now, and asks PM about no route", async () => {
    draw("calendar.details");
    expect(button("Allow").disabled).toBe(true);
    expect(button("No").disabled).toBe(true);
    expect(button("Not now").disabled).toBe(false);
    fireEvent.click(button("Allow"));
    expect(onAnswer).not.toHaveBeenCalled();
    act(() => void vi.advanceTimersByTime(PROMPT_GUARD_MS - 1));
    expect(button("Allow").disabled).toBe(true);
    act(() => void vi.advanceTimersByTime(1));
    fireEvent.click(button("Allow"));
    expect(onAnswer).toHaveBeenCalledWith(true, undefined);
    expect(modAiPromptInfo).not.toHaveBeenCalled();
    expect(screen.queryByRole("textbox")).toBeNull();
  });
});

describe("the money question", () => {
  it("on a route that can cost money shows the limit field from PM's suggestion, and Allow sends it", async () => {
    modAiPromptInfo.mockResolvedValue(info());
    draw("ai");
    expect(modAiPromptInfo).toHaveBeenCalledWith("acme.tool");
    await flush();
    const field = screen.getByRole("textbox") as HTMLInputElement;
    expect(field.value).toBe("1.00");
    const said = screen.getByTestId("money-prompt").textContent ?? "";
    expect(said).toContain("This mod wants to ask an AI model to do things for it.");
    expect(said).toContain(
      "It uses your Background model (Settings → AI & Models): a/cheap. If you change it, it uses the new one.",
    );
    expect(said).toContain("It can cost real money.");
    expect(said).toContain("You can change it any time in the Mods tab.");
    expect(screen.getByRole("textbox", { name: "Monthly limit" })).toBe(field);
    // The limit field already says it, so the dialog's own footer does not repeat it.
    expect(screen.getByRole("dialog").textContent?.split("any time in the Mods tab").length).toBe(
      2,
    );
    expect(screen.getByRole("dialog").textContent).not.toContain("whenever you like");
    waitOut();
    fireEvent.change(field, { target: { value: "2.50" } });
    fireEvent.click(button("Allow"));
    expect(onAnswer).toHaveBeenCalledWith(true, 2.5);
  });

  it("0 is a limit (the AI, no spending), and the suggestion is sent as it stands", async () => {
    modAiPromptInfo.mockResolvedValue(info({ suggestedCapUsd: 1 }));
    draw("ai");
    await flush();
    waitOut();
    fireEvent.click(button("Allow"));
    expect(onAnswer).toHaveBeenLastCalledWith(true, 1);
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "0" } });
    fireEvent.click(button("Allow"));
    expect(onAnswer).toHaveBeenLastCalledWith(true, 0);
  });

  it("will not allow with a limit PM would not store, and says so", async () => {
    modAiPromptInfo.mockResolvedValue(info());
    draw("ai");
    await flush();
    waitOut();
    for (const bad of ["", "abc", "-1", "1e3", "$2", "1,5", "100001"]) {
      fireEvent.change(screen.getByRole("textbox"), { target: { value: bad } });
      expect(button("Allow").disabled, JSON.stringify(bad)).toBe(true);
      expect(screen.getByRole("alert").textContent).toContain("0 to 100,000");
      expect(screen.getByRole("textbox").getAttribute("aria-invalid")).toBe("true");
    }
    fireEvent.click(button("Allow"));
    expect(onAnswer).not.toHaveBeenCalled();
    // A bad limit never stops the answer that is not a yes.
    fireEvent.click(button("No"));
    expect(onAnswer).toHaveBeenCalledWith(false);
  });

  it("No and Not now send no limit", async () => {
    modAiPromptInfo.mockResolvedValue(info({ route: "local-then-cloud", localModel: "llama" }));
    draw("ai");
    await flush();
    waitOut();
    fireEvent.click(button("No"));
    expect(onAnswer).toHaveBeenCalledWith(false);
    expect(onAnswer.mock.calls[0]).toHaveLength(1);
    fireEvent.click(button("Not now"));
    expect(onLater).toHaveBeenCalledTimes(1);
  });

  it("on a free route asks for no limit, so a later paid route asks then", async () => {
    modAiPromptInfo.mockResolvedValue(
      info({ route: "local", paid: false, cloudModels: [], localModel: "llama" }),
    );
    draw("ai");
    await flush();
    expect(screen.queryByRole("textbox")).toBeNull();
    const said = screen.getByTestId("money-prompt").textContent ?? "";
    expect(said).toContain("your local model llama");
    expect(said).toContain("A local model costs nothing.");
    waitOut();
    fireEvent.click(button("Allow"));
    expect(onAnswer).toHaveBeenCalledWith(true, undefined);
  });

  it("with no model set up, or a route PM cannot price, asks for no limit either", async () => {
    const cases: Partial<AiPromptInfo>[] = [
      { route: "none", paid: false, cloudModels: [], localModel: null },
      { route: "local", paid: false, unpriced: true },
    ];
    for (const extra of cases) {
      modAiPromptInfo.mockResolvedValue(info(extra));
      const { unmount } = draw("ai");
      await flush();
      expect(screen.queryByRole("textbox")).toBeNull();
      waitOut();
      fireEvent.click(button("Allow"));
      expect(onAnswer).toHaveBeenLastCalledWith(true, undefined);
      unmount();
    }
  });

  it("cannot be allowed until PM has said what the route is, and a failed lookup never can", async () => {
    let resolve: (i: AiPromptInfo) => void = () => {};
    modAiPromptInfo.mockReturnValue(new Promise<AiPromptInfo>((r) => (resolve = r)));
    draw("ai");
    waitOut();
    // Loading: No is open (it records only a refusal), Allow is not.
    expect(button("Allow").disabled).toBe(true);
    expect(button("No").disabled).toBe(false);
    await act(async () => resolve(info()));
    expect(screen.getByRole("textbox")).toBeTruthy();

    cleanup();
    modAiPromptInfo.mockRejectedValue(new Error("TEXT-THAT-MUST-NOT-SHOW provider said no"));
    draw("ai");
    await flush();
    waitOut();
    expect(button("Allow").disabled).toBe(true);
    expect(button("No").disabled).toBe(false);
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(document.body.textContent).not.toContain("TEXT-THAT-MUST-NOT-SHOW");
    expect(screen.getByRole("dialog").textContent).toContain("PM couldn't check which AI model");
  });

  it("restarts the hold when the content changes under the pointer", async () => {
    let resolve: (i: AiPromptInfo) => void = () => {};
    modAiPromptInfo.mockReturnValue(new Promise<AiPromptInfo>((r) => (resolve = r)));
    draw("ai");
    waitOut();
    // The first content has been on screen long enough: No answers.
    expect(button("No").disabled).toBe(false);
    // The route arrives and adds a field and sentences: the buttons are held again, for a full guard.
    await act(async () => resolve(info()));
    expect(button("No").disabled).toBe(true);
    expect(button("Allow").disabled).toBe(true);
    fireEvent.click(button("No"));
    fireEvent.click(button("Allow"));
    expect(onAnswer).not.toHaveBeenCalled();
    act(() => void vi.advanceTimersByTime(PROMPT_GUARD_MS - 1));
    expect(button("No").disabled).toBe(true);
    act(() => void vi.advanceTimersByTime(1));
    expect(button("No").disabled).toBe(false);
    expect(button("Allow").disabled).toBe(false);
  });

  it("draws the model's name and the mod's name as text, never as markup", async () => {
    modAiPromptInfo.mockResolvedValue(
      info({ cloudModels: ['<img src=x onerror="alert(1)">'], localModel: "<b>x</b>" }),
    );
    draw("ai");
    await flush();
    const money = screen.getByTestId("money-prompt");
    expect(money.querySelector("img")).toBeNull();
    expect(money.querySelector("b")).toBeNull();
    expect(money.textContent).toContain("your local model <b>x</b>");
    expect(money.textContent).toContain('<img src=x onerror="alert(1)">');
    // Nothing the mod said is in it but its name, in the title.
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText(/Acme Tool/)).toBeTruthy();
  });

  it("is still held while an answer is being saved", async () => {
    modAiPromptInfo.mockResolvedValue(info());
    draw("ai", { working: true });
    await flush();
    waitOut();
    expect(button("Saving…").disabled).toBe(true);
    expect(button("No").disabled).toBe(true);
    expect((screen.getByRole("textbox") as HTMLInputElement).disabled).toBe(true);
  });
});
