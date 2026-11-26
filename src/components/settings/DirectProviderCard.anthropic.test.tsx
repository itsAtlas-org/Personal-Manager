// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The same card as OpenAI's, fed Anthropic's descriptor. What is Anthropic's own: no data-sharing
// tick-box, a warning that names Fable, the key link and help id, and the shared no-credit sentence
// with its billing place. A key is still checked before it is kept.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
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
import { anthropicView, directState } from "../../lib/directProvidersFixture";
import type { DirectProviderView, KeyCheckResult } from "../../lib/types";
import { DirectProviderCard } from "./DirectProviderCard";

const nextView: { current: DirectProviderView | null } = { current: null };

function Harness({ initial }: { initial: DirectProviderView }) {
  const [view, setView] = useState(initial);
  return (
    <DirectProviderCard
      info={DIRECT_PROVIDERS.anthropic}
      view={view}
      reload={async () => {
        const next = (nextView.current ?? view) as DirectProviderView;
        setView(next);
        return directState({ view: next });
      }}
      orKey={{ chat: true, background: true }}
    />
  );
}

const OK: KeyCheckResult = {
  outcome: "ok",
  saved: true,
  listed: [
    "anthropic/claude-fable-5-2",
    "anthropic/claude-opus-5-5",
    "anthropic/claude-sonnet-5-5",
    "anthropic/claude-haiku-5-5",
  ],
  listed_total: 15,
};

const keyField = () => screen.getByLabelText("Anthropic API key") as HTMLInputElement;
const submit = () => fireEvent.submit(keyField().closest("form")!);
const FAKE = "sk-ant-api03-test-key-not-real-0123456789";

beforeEach(() => {
  nextView.current = null;
  m.setDirectOptIn.mockResolvedValue([]);
  m.clearDirectKey.mockResolvedValue([]);
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("Anthropic's card", () => {
  it("is off by default, in Anthropic's words, with no key field", () => {
    render(
      <Harness initial={anthropicView({ opted_in: false, key: "absent", key_check: null })} />,
    );
    expect(screen.getByRole("switch", { name: "Use my own Anthropic key" })).toBeTruthy();
    expect(screen.queryByLabelText("Anthropic API key")).toBeNull();
    expect(
      screen.getByText(
        "Off. PM uses OpenRouter, which asks for zero data retention on every request.",
      ),
    ).toBeTruthy();
  });

  it("shows the warning with Fable named, and draws no data-sharing tick-box", () => {
    render(<Harness initial={anthropicView({ key: "absent", key_check: null })} />);
    expect(screen.getByText("This isn't zero data retention.")).toBeTruthy();
    expect(
      screen.getByText(
        "Claude Fable always needs 30-day retention: Anthropic won't run it on a workspace set to zero retention.",
      ),
    ).toBeTruthy();
    expect(screen.queryByRole("checkbox")).toBeNull();
    // The headline is always the plain one, even if a stray shares_data came back true.
    cleanup();
    render(<Harness initial={anthropicView({ shares_data: true })} />);
    expect(screen.getByText("This isn't zero data retention.")).toBeTruthy();
    expect(screen.queryByRole("checkbox")).toBeNull();
  });

  it("has Anthropic's key link and help id on the field", () => {
    render(<Harness initial={anthropicView({ key: "absent", key_check: null })} />);
    const link = screen.getByRole("link", { name: "Get a key at platform.claude.com →" });
    expect(link.getAttribute("href")).toBe("https://platform.claude.com/settings/keys");
    expect(keyField().getAttribute("data-help")).toBe("settings-anthropic-key");
    expect(keyField().getAttribute("placeholder")).toBe("sk-ant-...");
  });

  it("folds the terms and the test-message line behind one disclosure", () => {
    render(<Harness initial={anthropicView()} />);
    expect(screen.getByText("What Anthropic keeps")).toBeTruthy();
    expect(screen.getByText(/Checking a key sends Anthropic one tiny test message/)).toBeTruthy();
    expect(screen.getByText(/Claude Fable 5\.2 is a model Anthropic only runs/)).toBeTruthy();
  });

  it("checks a typed key once on Enter, and keeps it only when Anthropic accepts it", async () => {
    m.saveDirectKey.mockResolvedValue(OK);
    nextView.current = anthropicView();
    render(<Harness initial={anthropicView({ key: "absent", key_check: null })} />);
    fireEvent.change(keyField(), { target: { value: FAKE } });
    submit();
    submit();
    await waitFor(() =>
      expect(screen.getByText(/Key works\. 4 Anthropic models are ready to use\./)).toBeTruthy(),
    );
    expect(m.saveDirectKey).toHaveBeenCalledTimes(1);
    expect(m.saveDirectKey).toHaveBeenCalledWith("anthropic", FAKE);
    expect(keyField().value).toBe("");
  });

  it("never stores a refused key and keeps what was typed", async () => {
    m.saveDirectKey.mockResolvedValue({
      outcome: "rejected",
      saved: false,
      listed: [],
      listed_total: 0,
    });
    render(<Harness initial={anthropicView({ key: "absent", key_check: null })} />);
    fireEvent.change(keyField(), { target: { value: FAKE } });
    submit();
    expect(
      await screen.findByText(
        "Not saved. Anthropic says this key isn't right. Check you copied the whole key from your Anthropic Console workspace (Anthropic keys start with sk-ant-). If it's an Admin key, make a normal API key instead.",
      ),
    ).toBeTruthy();
    expect(keyField().value).toBe(FAKE);
    expect(screen.queryByRole("button", { name: "Check again for Anthropic" })).toBeNull();
  });

  it("reads another provider's key as not a key, never as Anthropic refusing it", async () => {
    m.saveDirectKey.mockResolvedValue({
      outcome: "malformed",
      saved: false,
      listed: [],
      listed_total: 0,
    });
    render(<Harness initial={anthropicView({ key: "absent", key_check: null })} />);
    fireEvent.change(keyField(), { target: { value: "sk-proj-an-openai-key-0123456789" } });
    submit();
    expect(
      await screen.findByText(
        "Not saved. That doesn't look like a key. Paste the whole key, with no spaces.",
      ),
    ).toBeTruthy();
  });

  it("keeps a key with no credit, names platform.claude.com, and offers Check again", async () => {
    m.saveDirectKey.mockResolvedValue({
      outcome: "no_credit",
      saved: true,
      listed: OK.listed,
      listed_total: 15,
    });
    nextView.current = anthropicView({
      key_check: { at: "x", outcome: "no_credit", listed: OK.listed, listed_total: 15 },
    });
    render(<Harness initial={anthropicView({ key: "absent", key_check: null })} />);
    fireEvent.change(keyField(), { target: { value: FAKE } });
    submit();
    const line = await screen.findByText(
      "Saved. Your Anthropic account is out of credit or at a spend limit, so chats won't work until you add credit or raise the limit at platform.claude.com. Then press Check again.",
    );
    expect(line.closest('[role="status"]')).toBeTruthy();
    expect(screen.getByRole("button", { name: "Check again for Anthropic" })).toBeTruthy();
  });

  it("words a read-only key with 'workspace', and a restricted one with its hint", async () => {
    m.saveDirectKey.mockResolvedValue({
      outcome: "restricted",
      note: "read_only",
      saved: true,
      listed: OK.listed,
      listed_total: 15,
    });
    nextView.current = anthropicView({
      key_check: {
        at: "x",
        outcome: "restricted",
        note: "read_only",
        listed: OK.listed,
        listed_total: 15,
      },
    });
    render(<Harness initial={anthropicView({ key: "absent", key_check: null })} />);
    fireEvent.change(keyField(), { target: { value: FAKE } });
    submit();
    expect(
      await screen.findByText(
        "Saved. This key can list models but can't chat. Give it access to chat in your Anthropic workspace, then press Check again.",
      ),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Check again for Anthropic" })).toBeTruthy();
  });

  it("offers Check again when the test chat couldn't be tried, and not for a plain working key", () => {
    const { unmount } = render(<Harness initial={anthropicView()} />);
    expect(screen.queryByRole("button", { name: "Check again for Anthropic" })).toBeNull();
    unmount();
    render(
      <Harness
        initial={anthropicView({
          key_check: {
            at: "x",
            outcome: "ok",
            note: "chat_untried",
            listed: OK.listed,
            listed_total: 15,
          },
        })}
      />,
    );
    expect(screen.getByRole("button", { name: "Check again for Anthropic" })).toBeTruthy();
  });

  it("turns no credit into working with one Check again, on the same aria-disabled button", async () => {
    const noCredit = anthropicView({
      key_check: { at: "x", outcome: "no_credit", listed: OK.listed, listed_total: 15 },
    });
    let release: (r: KeyCheckResult) => void = () => {};
    m.recheckDirectKey.mockReturnValue(new Promise<KeyCheckResult>((r) => (release = r)));
    render(<Harness initial={noCredit} />);
    const btn = screen.getByRole("button", { name: "Check again for Anthropic" });
    btn.focus();
    fireEvent.click(btn);
    await waitFor(() => expect(btn.getAttribute("aria-disabled")).toBe("true"));
    expect(btn.hasAttribute("disabled")).toBe(false);
    fireEvent.click(btn);
    expect(m.recheckDirectKey).toHaveBeenCalledTimes(1);
    nextView.current = anthropicView();
    release(OK);
    await waitFor(() => expect(screen.getByText(/Key works\./)).toBeTruthy());
    expect(m.recheckDirectKey).toHaveBeenCalledWith("anthropic");
  });
});
