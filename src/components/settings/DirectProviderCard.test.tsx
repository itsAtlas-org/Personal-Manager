// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The card for a provider the person reaches with their own key. What it must get right is not
// looks: a key is CHECKED before it is kept, a refused one is never stored, the not-zero-data-
// retention warning is on screen the moment the switch is, and a check can be read and heard.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const m = vi.hoisted(() => ({
  setDirectOptIn: vi.fn(),
  saveDirectKey: vi.fn(),
  recheckDirectKey: vi.fn(),
  clearDirectKey: vi.fn(),
  setDirectDataSharing: vi.fn(),
}));

vi.mock("../../lib/ipc", () => ({
  setDirectOptIn: (...a: unknown[]) => m.setDirectOptIn(...a),
  saveDirectKey: (...a: unknown[]) => m.saveDirectKey(...a),
  recheckDirectKey: (...a: unknown[]) => m.recheckDirectKey(...a),
  clearDirectKey: (...a: unknown[]) => m.clearDirectKey(...a),
  setDirectDataSharing: (...a: unknown[]) => m.setDirectDataSharing(...a),
}));
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { DIRECT_PROVIDERS } from "../../lib/directProviders";
import { directState, openaiView } from "../../lib/directProvidersFixture";
import type { DirectProviderView, KeyCheckResult } from "../../lib/types";
import { DirectProviderCard } from "./DirectProviderCard";

const reload = vi.fn();

/** Holds the view the way the hook does: the card never works out a move itself, it asks again. */
function Harness({ initial }: { initial: DirectProviderView }) {
  const [view, setView] = useState(initial);
  return (
    <DirectProviderCard
      info={DIRECT_PROVIDERS.openai}
      view={view}
      reload={async () => {
        reload();
        const next = (nextView.current ?? view) as DirectProviderView;
        setView(next);
        return directState({ view: next });
      }}
      orKey={{ chat: true, background: true }}
    />
  );
}
const nextView: { current: DirectProviderView | null } = { current: null };

const OK: KeyCheckResult = {
  outcome: "ok",
  saved: true,
  listed: ["openai/gpt-6.2-sol", "openai/gpt-6-luna"],
  listed_total: 40,
};

const keyField = () => screen.getByLabelText("OpenAI API key") as HTMLInputElement;
const submit = () => fireEvent.submit(keyField().closest("form")!);

beforeEach(() => {
  nextView.current = null;
  m.setDirectOptIn.mockResolvedValue([]);
  m.clearDirectKey.mockResolvedValue([]);
  m.setDirectDataSharing.mockResolvedValue(undefined);
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("the switch", () => {
  it("is off by default, and draws no key field and no warning", () => {
    render(<Harness initial={openaiView({ opted_in: false, key: "absent", key_check: null })} />);
    expect(screen.queryByLabelText("OpenAI API key")).toBeNull();
    expect(screen.queryByText("This isn't zero data retention.")).toBeNull();
    expect(
      screen.getByText(
        "Off. PM uses OpenRouter, which asks for zero data retention on every request.",
      ),
    ).toBeTruthy();
    expect(screen.getByRole("switch", { name: "Use my own OpenAI key" })).toBeTruthy();
  });

  it("says a kept key is not in use while it is off, and still lets it be removed", () => {
    render(<Harness initial={openaiView({ opted_in: false })} />);
    expect(screen.getByText("Off. Your key is saved but PM isn't using it.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Remove key for OpenAI" })).toBeTruthy();
  });

  it("shows the warning the moment it is on, and leaves focus on the switch", async () => {
    nextView.current = openaiView({ key: "absent", key_check: null });
    render(<Harness initial={openaiView({ opted_in: false, key: "absent", key_check: null })} />);
    const toggle = screen.getByRole("switch", { name: "Use my own OpenAI key" });
    toggle.focus();
    fireEvent.click(toggle);
    await waitFor(() => expect(screen.getByText("This isn't zero data retention.")).toBeTruthy());
    expect(m.setDirectOptIn).toHaveBeenCalledWith("openai", true);
    expect(document.activeElement).toBe(
      screen.getByRole("switch", { name: "Use my own OpenAI key" }),
    );
    // The announcement carries the warning too, for someone who never reads the callout.
    expect(
      within(screen.getAllByRole("status")[0]).getByText(
        "OpenAI is on. Add your key. This isn't zero data retention.",
      ),
    ).toBeTruthy();
  });

  it("says which roles went back to OpenRouter when it is turned off, and keeps saying so", async () => {
    m.setDirectOptIn.mockResolvedValue([
      { role: "chat", from: "openai", to: "openrouter" },
      { role: "background", from: "openai", to: "openrouter" },
    ]);
    nextView.current = openaiView({ opted_in: false });
    render(<Harness initial={openaiView()} />);
    fireEvent.click(screen.getByRole("switch", { name: "Use my own OpenAI key" }));
    await waitFor(() =>
      expect(screen.getByText("Chat and Background are back on OpenRouter.")).toBeTruthy(),
    );
    expect(m.setDirectOptIn).toHaveBeenCalledWith("openai", false);
  });

  it("forgets a half-typed key when it is turned off, so turning it back on starts empty", async () => {
    render(<Harness initial={openaiView({ key: "absent", key_check: null })} />);
    fireEvent.change(keyField(), { target: { value: "half-typed-key-0123456789" } });
    nextView.current = openaiView({ opted_in: false, key: "absent", key_check: null });
    fireEvent.click(screen.getByRole("switch", { name: "Use my own OpenAI key" }));
    await waitFor(() => expect(screen.queryByLabelText("OpenAI API key")).toBeNull());
    nextView.current = openaiView({ key: "absent", key_check: null });
    fireEvent.click(screen.getByRole("switch", { name: "Use my own OpenAI key" }));
    await waitFor(() => expect(keyField().value).toBe(""));
  });

  it("changes the headline when the project shares data", () => {
    render(<Harness initial={openaiView({ shares_data: true })} />);
    expect(screen.getByText("OpenAI may train on what you send.")).toBeTruthy();
    expect(screen.queryByText("This isn't zero data retention.")).toBeNull();
  });

  it("writes the tick-box through the sharing command", async () => {
    render(<Harness initial={openaiView()} />);
    fireEvent.click(screen.getByLabelText(/My OpenAI project shares data/));
    await waitFor(() => expect(m.setDirectDataSharing).toHaveBeenCalledWith("openai", true));
  });
});

describe("checking a key", () => {
  it("shows a good key as a status with a tick and words, and keeps it", async () => {
    m.saveDirectKey.mockResolvedValue(OK);
    nextView.current = openaiView();
    render(<Harness initial={openaiView({ key: "absent", key_check: null })} />);
    fireEvent.change(keyField(), { target: { value: "sk-test-key-not-real-0123456789" } });
    submit();
    await waitFor(() =>
      expect(screen.getByText(/Key works\. 2 OpenAI models are ready to use\./)).toBeTruthy(),
    );
    expect(m.saveDirectKey).toHaveBeenCalledWith("openai", "sk-test-key-not-real-0123456789");
    // The typed key leaves the field once it is kept.
    expect(keyField().value).toBe("");
    expect(screen.getByText(/Key works\./).closest('[role="status"]')).toBeTruthy();
  });

  it("does not call a store for a key that was refused, and keeps what was typed", async () => {
    m.saveDirectKey.mockResolvedValue({
      outcome: "rejected",
      saved: false,
      listed: [],
      listed_total: 0,
    });
    render(<Harness initial={openaiView({ key: "absent", key_check: null })} />);
    fireEvent.change(keyField(), { target: { value: "sk-wrong-key-0123456789abcdef" } });
    submit();
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toMatch(/^Not saved\. OpenAI says this key isn't right/);
    expect(alert.textContent).toContain("Admin key");
    // Nothing was kept, so nothing is re-read as if it had been.
    expect(reload).not.toHaveBeenCalled();
    expect(keyField().value).toBe("sk-wrong-key-0123456789abcdef");
    expect(keyField().getAttribute("aria-invalid")).toBe("true");
  });

  it.each([
    ["region_blocked", /doesn't serve requests from where you are/],
    ["malformed", /doesn't look like a key/],
    ["unreachable", /couldn't reach OpenAI/],
  ] as const)("says %s in its own words, as an alert", async (outcome, text) => {
    m.saveDirectKey.mockResolvedValue({ outcome, saved: false, listed: [], listed_total: 0 });
    render(<Harness initial={openaiView({ key: "absent", key_check: null })} />);
    fireEvent.change(keyField(), { target: { value: "sk-anything-0123456789abcdef" } });
    submit();
    expect((await screen.findByRole("alert")).textContent).toMatch(text);
  });

  it("says an unexpected status", async () => {
    m.saveDirectKey.mockResolvedValue({
      outcome: { unexpected: 418 },
      saved: false,
      listed: [],
      listed_total: 0,
    });
    render(<Harness initial={openaiView({ key: "absent", key_check: null })} />);
    fireEvent.change(keyField(), { target: { value: "sk-anything-0123456789abcdef" } });
    submit();
    expect((await screen.findByRole("alert")).textContent).toBe(
      "Not saved. OpenAI answered with an error (418). Try again shortly.",
    );
  });

  it("keeps a restricted key and says what it can't tell", async () => {
    m.saveDirectKey.mockResolvedValue({
      outcome: "restricted",
      saved: true,
      listed: [],
      listed_total: 0,
    });
    nextView.current = openaiView({
      key_check: { at: "x", outcome: "restricted", listed: [], listed_total: 0 },
    });
    render(<Harness initial={openaiView({ key: "absent", key_check: null })} />);
    fireEvent.change(keyField(), { target: { value: "sk-restricted-0123456789abcdef" } });
    submit();
    await waitFor(() => expect(screen.getByText(/^Saved\. This key is restricted/)).toBeTruthy());
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("keeps a key with no credit, says so as a status and not an alert, and offers Check again", async () => {
    m.saveDirectKey.mockResolvedValue({
      outcome: "no_credit",
      saved: true,
      listed: ["openai/gpt-6.2-sol"],
      listed_total: 90,
    });
    nextView.current = openaiView({
      key_check: {
        at: "x",
        outcome: "no_credit",
        listed: ["openai/gpt-6.2-sol"],
        listed_total: 90,
      },
    });
    render(<Harness initial={openaiView({ key: "absent", key_check: null })} />);
    fireEvent.change(keyField(), { target: { value: "sk-nocredit-0123456789abcdef" } });
    submit();
    const line = await screen.findByText(
      "Saved. Your OpenAI account has no credit yet, so chats won't work until you add some at platform.openai.com. Then press Check again.",
    );
    expect(line.closest('[role="status"]')).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("button", { name: "Check again for OpenAI" })).toBeTruthy();
  });

  it("says a read-only key can't chat, and a chat that couldn't be tried is a note, not a failure", async () => {
    m.saveDirectKey.mockResolvedValue({
      outcome: "restricted",
      saved: true,
      listed: ["openai/gpt-6.2-sol"],
      listed_total: 90,
      note: "read_only",
    });
    nextView.current = openaiView({
      key_check: {
        at: "x",
        outcome: "restricted",
        listed: ["openai/gpt-6.2-sol"],
        listed_total: 90,
        note: "read_only",
      },
    });
    render(<Harness initial={openaiView({ key: "absent", key_check: null })} />);
    fireEvent.change(keyField(), { target: { value: "sk-readonly-0123456789abcdef" } });
    submit();
    expect(
      await screen.findByText(
        "Saved. This key can list models but can't chat. Give it access to chat in your OpenAI project, then press Check again.",
      ),
    ).toBeTruthy();
    cleanup();

    m.saveDirectKey.mockResolvedValue({ ...OK, note: "chat_untried" });
    nextView.current = openaiView({
      key_check: {
        at: "x",
        outcome: "ok",
        listed: OK.listed,
        listed_total: 40,
        note: "chat_untried",
      },
    });
    render(<Harness initial={openaiView({ key: "absent", key_check: null })} />);
    fireEvent.change(keyField(), { target: { value: "sk-untried-0123456789abcdef" } });
    submit();
    expect(
      await screen.findByText(
        /Key works\. 2 OpenAI models are ready to use\. PM couldn't try a chat just now\./,
      ),
    ).toBeTruthy();
    expect(screen.queryByText(/credit/)).toBeNull();
    // A caveat, not a verified tick.
    expect(screen.queryByText(/✓/)).toBeNull();
    expect(screen.getByRole("button", { name: "Check again for OpenAI" })).toBeTruthy();
  });

  it("has no Check again for a key that plainly works", () => {
    render(<Harness initial={openaiView()} />);
    expect(screen.queryByRole("button", { name: "Check again for OpenAI" })).toBeNull();
  });

  it("turns no credit into working with Check again: one call, the same busy button, focus kept", async () => {
    const noCredit = openaiView({
      key_check: { at: "x", outcome: "no_credit", listed: OK.listed, listed_total: 40 },
    });
    let finish: (r: KeyCheckResult) => void = () => {};
    m.recheckDirectKey.mockReturnValue(new Promise<KeyCheckResult>((r) => (finish = r)));
    nextView.current = openaiView();
    render(<Harness initial={noCredit} />);
    // A key saved earlier: the stored line has no "Saved." in front of it.
    expect(screen.getByText(/^Your OpenAI account has no credit yet/)).toBeTruthy();
    const button = screen.getByRole("button", { name: "Check again for OpenAI" });
    fireEvent.click(button);
    const busy = await screen.findByRole("button", { name: "Check again for OpenAI" });
    expect(busy).toBe(button);
    expect(busy.textContent).toBe("Checking...");
    expect(busy.getAttribute("aria-disabled")).toBe("true");
    expect((busy as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(busy);
    fireEvent.click(busy);
    expect(m.recheckDirectKey).toHaveBeenCalledTimes(1);
    expect(m.recheckDirectKey).toHaveBeenCalledWith("openai");
    await act(async () => finish(OK));
    await waitFor(() => expect(screen.getByText(/Key works\. 2 OpenAI models/)).toBeTruthy());
    expect(screen.queryByText(/no credit yet/)).toBeNull();
    // Nothing left to recheck: the button goes, and focus lands on the key field and not <body>.
    expect(screen.queryByRole("button", { name: "Check again for OpenAI" })).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(keyField()));
  });

  it("keeps Check again when the credit isn't there yet, and says a refusal of a saved key without 'Not saved'", async () => {
    const noCredit = openaiView({
      key_check: { at: "x", outcome: "no_credit", listed: OK.listed, listed_total: 40 },
    });
    m.recheckDirectKey.mockResolvedValueOnce({
      outcome: "no_credit",
      saved: true,
      listed: OK.listed,
      listed_total: 40,
    });
    nextView.current = noCredit;
    render(<Harness initial={noCredit} />);
    fireEvent.click(screen.getByRole("button", { name: "Check again for OpenAI" }));
    await waitFor(() => expect(m.recheckDirectKey).toHaveBeenCalledTimes(1));
    expect(await screen.findByRole("button", { name: "Check again for OpenAI" })).toBeTruthy();

    m.recheckDirectKey.mockResolvedValueOnce({
      outcome: "unreachable",
      saved: true,
      listed: [],
      listed_total: 0,
    });
    fireEvent.click(screen.getByRole("button", { name: "Check again for OpenAI" }));
    expect(
      await screen.findByText(
        "PM couldn't reach OpenAI to check it. Check your connection and try again.",
      ),
    ).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByText(/Not saved/)).toBeNull();
    // Nothing was stored, so the stored verdict's button still stands, and focus has not moved off it.
    const again = screen.getByRole("button", { name: "Check again for OpenAI" });
    expect(again).toBeTruthy();
    expect(document.activeElement).not.toBe(keyField());
  });

  it("announces a repeated verdict: the old one leaves while the check is out and comes back new", async () => {
    const noCredit = openaiView({
      key_check: { at: "x", outcome: "no_credit", listed: OK.listed, listed_total: 40 },
    });
    let finish: (r: KeyCheckResult) => void = () => {};
    m.recheckDirectKey.mockReturnValue(new Promise<KeyCheckResult>((r) => (finish = r)));
    nextView.current = noCredit;
    render(<Harness initial={noCredit} />);
    const line = /^Your OpenAI account has no credit yet/;
    expect(screen.getByText(line)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Check again for OpenAI" }));
    await waitFor(() =>
      expect(
        screen
          .getByRole("button", { name: "Check again for OpenAI" })
          .getAttribute("aria-disabled"),
      ).toBe("true"),
    );
    // Without this the identical answer would change nothing in the live region.
    expect(screen.queryByText(line)).toBeNull();
    await act(async () =>
      finish({ outcome: "no_credit", saved: true, listed: OK.listed, listed_total: 40 }),
    );
    expect(await screen.findByText(line)).toBeTruthy();
  });

  it("asks for a key before checking an empty field", () => {
    render(<Harness initial={openaiView({ key: "absent", key_check: null })} />);
    submit();
    expect(screen.getByRole("alert").textContent).toBe("Paste your key first.");
    expect(m.saveDirectKey).not.toHaveBeenCalled();
  });

  it("sends one check however many times Enter is pressed, and keeps the same button", async () => {
    let finish: (r: KeyCheckResult) => void = () => {};
    m.saveDirectKey.mockReturnValue(new Promise<KeyCheckResult>((r) => (finish = r)));
    nextView.current = openaiView();
    render(<Harness initial={openaiView({ key: "absent", key_check: null })} />);
    fireEvent.change(keyField(), { target: { value: "sk-slow-0123456789abcdef" } });
    const button = screen.getByRole("button", { name: "Save and check" });
    submit();
    const checking = await screen.findByRole("button", { name: "Checking..." });
    // The very same element, kept in the tab order, told it is busy: swapping or `disabled` would
    // drop a focused button's focus in WebKitGTK.
    expect(checking).toBe(button);
    expect(checking.getAttribute("aria-disabled")).toBe("true");
    expect((checking as HTMLButtonElement).disabled).toBe(false);
    // A form still submits through an aria-disabled button, and through Enter in the field.
    submit();
    submit();
    expect(m.saveDirectKey).toHaveBeenCalledTimes(1);
    await act(async () => finish(OK));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Save and check" })).toBe(button),
    );
    expect(button.getAttribute("aria-disabled")).toBeNull();
  });

  it("announces that it is checking", async () => {
    let finish: (r: KeyCheckResult) => void = () => {};
    m.saveDirectKey.mockReturnValue(new Promise<KeyCheckResult>((r) => (finish = r)));
    render(<Harness initial={openaiView({ key: "absent", key_check: null })} />);
    fireEvent.change(keyField(), { target: { value: "sk-slow-0123456789abcdef" } });
    submit();
    await waitFor(() => expect(screen.getByText("Checking your key")).toBeTruthy());
    await act(async () =>
      finish({ outcome: "unreachable", saved: false, listed: [], listed_total: 0 }),
    );
  });

  it("never stops a paste or a password manager, and names the field for what it is", () => {
    render(<Harness initial={openaiView({ key: "absent", key_check: null })} />);
    const field = keyField();
    expect(field.getAttribute("type")).toBe("password");
    expect(field.getAttribute("maxlength")).toBeNull();
    const paste = fireEvent.paste(field);
    expect(paste).toBe(true);
  });
});

describe("a key already there", () => {
  it("draws the stored verdict beside a key that is really present", () => {
    render(<Harness initial={openaiView()} />);
    expect(screen.getByText(/Key works\. 2 OpenAI models are ready to use\./)).toBeTruthy();
    expect(keyField().getAttribute("placeholder")).toBe("•••••••• (saved, type to replace)");
  });

  it("hides a verdict carried over from a backup when there is no key on this device", () => {
    render(<Harness initial={openaiView({ key: "absent" })} />);
    expect(screen.queryByText(/Key works/)).toBeNull();
  });

  it("says the keychain can't be read, without calling it a missing key", () => {
    render(<Harness initial={openaiView({ key: "unreadable", key_check: null })} />);
    expect(screen.getByText("PM can't read your saved OpenAI key right now.")).toBeTruthy();
    expect(screen.queryByText(/Key works/)).toBeNull();
  });
});

describe("removing the key", () => {
  it("removes it straight away when no role uses the provider", async () => {
    nextView.current = openaiView({ key: "absent", key_check: null });
    render(<Harness initial={openaiView()} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove key for OpenAI" }));
    await waitFor(() => expect(m.clearDirectKey).toHaveBeenCalledWith("openai"));
    expect(screen.queryByText("Remove your OpenAI key?")).toBeNull();
  });

  it("asks first when a role uses it, naming the roles, with Keep it focused", async () => {
    const used = directState({ chat: "openai", background: "openai" }).providers[0];
    render(<Harness initial={used} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove key for OpenAI" }));
    expect(await screen.findByText("Remove your OpenAI key?")).toBeTruthy();
    expect(
      screen.getByText(
        "Chat and Background use it now. They'll go back to OpenRouter. You can paste the key again any time.",
      ),
    ).toBeTruthy();
    expect(m.clearDirectKey).not.toHaveBeenCalled();
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole("button", { name: "Keep it" })),
    );
    fireEvent.click(screen.getByRole("button", { name: "Keep it" }));
    expect(m.clearDirectKey).not.toHaveBeenCalled();
  });

  it("names only the role that uses it", async () => {
    const used = directState({ background: "openai" }).providers[0];
    render(<Harness initial={used} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove key for OpenAI" }));
    expect(
      await screen.findByText(
        "Background uses it now. It'll go back to OpenRouter. You can paste the key again any time.",
      ),
    ).toBeTruthy();
  });

  it("puts focus on the key field once the button that had it is gone", async () => {
    m.clearDirectKey.mockResolvedValue([{ role: "chat", from: "openai", to: "openrouter" }]);
    nextView.current = openaiView({ key: "absent", key_check: null });
    render(<Harness initial={directState({ chat: "openai" }).providers[0]} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove key for OpenAI" }));
    fireEvent.click(await screen.findByRole("button", { name: "Remove key" }));
    await waitFor(() => expect(m.clearDirectKey).toHaveBeenCalled());
    await waitFor(() => expect(document.activeElement).toBe(keyField()));
    expect(screen.queryByRole("button", { name: "Remove key for OpenAI" })).toBeNull();
    expect(screen.getByText("Chat is back on OpenRouter.")).toBeTruthy();
  });
});
