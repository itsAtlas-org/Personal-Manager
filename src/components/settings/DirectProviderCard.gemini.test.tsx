// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The card for Google Gemini. What it must get right is the second axis: the same key is on free or
// paid terms, and the card says which, in words, wherever the person chooses it. A free key that PM
// won't use here is refused with the zone PM read and a way to try again with the SAME typed key; a
// stored one that reads free is paused, never deleted, and re-asked with one button; and nothing in
// it reads a key, a region or a tier on its own: it words what Rust answered.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
import { FLASH, geminiView, PRO, stateWith } from "../../lib/directProvidersFixture";
import type { DirectProviderView, KeyCheckResult, RegionRefusal } from "../../lib/types";
import { DirectProviderCard } from "./DirectProviderCard";

const UK: RegionRefusal = {
  kind: "uk",
  label: "the United Kingdom",
  zone: "Europe/London",
  basis: "free",
};
const LISTED = [FLASH.id, PRO.id];
const FREE_CHECK = {
  at: "2026-11-26T10:00:00Z",
  outcome: "ok" as const,
  listed: LISTED,
  listed_total: 49,
  tier: "free" as const,
  flipped_free_at: null,
};

/** The card with the view it is handed; `reload` answers with the same state, as the hook would
 *  after a write that changed nothing. */
function Card({ view }: { view: DirectProviderView }) {
  return (
    <DirectProviderCard
      info={DIRECT_PROVIDERS.gemini}
      view={view}
      reload={() => Promise.resolve(stateWith([view]))}
      orKey={{ chat: true, background: true }}
    />
  );
}

const keyField = () => screen.getByLabelText("Gemini API key") as HTMLInputElement;
const form = () => keyField().closest("form")!;

beforeEach(() => {
  m.setDirectOptIn.mockResolvedValue([]);
  m.clearDirectKey.mockResolvedValue([]);
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("before any key", () => {
  it("names the free-tier rule and the region rule, unfolded, with no tick-box", () => {
    render(<Card view={geminiView({ key: "absent", key_check: null, tier: null })} />);
    expect(screen.getByText("This isn't zero data retention.")).toBeTruthy();
    expect(
      screen.getByText(
        "A free Gemini key lets Google train on what you send, and lets people at Google read it.",
      ),
    ).toBeTruthy();
    expect(screen.getByText(/In the UK, the EU, Iceland, Liechtenstein, Norway/)).toBeTruthy();
    // Gemini's tier is detected, never asserted: there is no tick-box to claim anything.
    expect(screen.queryByRole("checkbox")).toBeNull();
    // No key, no badge.
    expect(screen.queryByText(/Paid key|Free key|Not checked yet/)).toBeNull();
  });

  it("puts the Gemini help id on the key field", () => {
    render(<Card view={geminiView({ key: "absent", key_check: null, tier: null })} />);
    expect(keyField().getAttribute("data-help")).toBe("settings-gemini-key");
  });

  it("announces the switch in Gemini's words", async () => {
    render(<Card view={geminiView({ opted_in: false, key: "absent", key_check: null })} />);
    fireEvent.click(screen.getByRole("switch", { name: "Use my own Google Gemini key" }));
    await waitFor(() =>
      expect(
        within(screen.getAllByRole("status")[0]).getByText(
          "Google Gemini is on. Add your key. This isn't zero data retention.",
        ),
      ).toBeTruthy(),
    );
  });
});

describe("a key that checks out", () => {
  it("drops the old verdict while a replacement is checked, so an identical answer is read again", async () => {
    // A saved paid key whose stored verdict reads exactly like the new one will.
    const same = "✓ Key works. It's on Google's paid terms. 2 Gemini models are ready to use.";
    const stored = geminiView();
    render(
      <Card
        view={{
          ...stored,
          key_check: { ...stored.key_check!, listed: LISTED, listed_total: 49 },
        }}
      />,
    );
    expect(screen.queryByText(same)).toBeTruthy();
    let finish: (r: KeyCheckResult) => void = () => {};
    m.saveDirectKey.mockReturnValueOnce(new Promise<KeyCheckResult>((r) => (finish = r)));
    fireEvent.change(keyField(), { target: { value: "AIzaTypedKeyNotReal0123456789abcdef" } });
    fireEvent.submit(form());
    // While it works the old line is gone, so the same words arriving are a change in the live region.
    await waitFor(() => expect(screen.queryByText(same)).toBeNull());
    await act(async () =>
      finish({
        outcome: "ok",
        saved: true,
        listed: LISTED,
        listed_total: 49,
        tier: "paid",
      }),
    );
    expect(await screen.findByText(same)).toBeTruthy();
  });

  it("dates Google's terms as DD-MM-YYYY and says what the key check costs", () => {
    render(<Card view={geminiView({ terms_checked_on: "2026-11-26" })} />);
    expect(
      screen.getByText(
        /These are Google's terms when this version of PM was built \(checked 26-11-2026\)\. They can change\./,
      ),
    ).toBeTruthy();
    expect(
      screen.getByText(/costs a tiny fraction of a cent, and it isn't listed in Usage/),
    ).toBeTruthy();
  });

  it("shows a paid key as paid, on the warning tone", async () => {
    m.saveDirectKey.mockResolvedValue({
      outcome: "ok",
      saved: true,
      listed: LISTED,
      listed_total: 49,
      tier: "paid",
    } satisfies KeyCheckResult);
    render(<Card view={geminiView()} />);
    fireEvent.change(keyField(), { target: { value: "AIzaTypedKeyNotReal0123456789abcdef" } });
    fireEvent.submit(form());
    expect(
      await screen.findByText(
        "✓ Key works. It's on Google's paid terms. 2 Gemini models are ready to use.",
      ),
    ).toBeTruthy();
    expect(m.saveDirectKey).toHaveBeenCalledWith("gemini", "AIzaTypedKeyNotReal0123456789abcdef");
    // The badge is real text; its glyph is hidden from a screen reader.
    const badge = screen.getByText("Paid key");
    expect(badge.textContent).toBe("Paid key");
    expect(badge.previousElementSibling?.getAttribute("aria-hidden")).toBe("true");
    expect(
      screen.getByText(
        "Your key is on Google's paid terms, so Google doesn't use what you send to improve its products.",
      ),
    ).toBeTruthy();
  });

  it("shows a free key outside the region as free, with the strongest warning", () => {
    render(<Card view={geminiView({ tier: "free", key_check: FREE_CHECK })} />);
    expect(screen.getByText("Free key")).toBeTruthy();
    expect(
      screen.getByText("Google may train on what you send, and people at Google may read it."),
    ).toBeTruthy();
    // A free key might have been upgraded: it can always be asked again.
    expect(screen.getByRole("button", { name: "Check again for Google Gemini" })).toBeTruthy();
  });

  it("says 'not checked yet' for a key with no verdict, never 'couldn't tell'", () => {
    render(<Card view={geminiView({ tier: null })} />);
    expect(screen.getByText("Not checked yet (treated as free)")).toBeTruthy();
    expect(screen.getByText("PM hasn't checked this key, so it treats it as free.")).toBeTruthy();
    expect(screen.queryByText(/couldn't tell/)).toBeNull();
    expect(screen.getByRole("button", { name: "Check again for Google Gemini" })).toBeTruthy();
  });

  it("says 'couldn't tell' for an unknown verdict, and treats it as free", () => {
    render(<Card view={geminiView({ tier: "unknown" })} />);
    expect(screen.getByText("Free key (PM couldn't tell)")).toBeTruthy();
    // The warning and the stored verdict both say it.
    expect(
      screen.getAllByText(
        /PM couldn't tell whether this key is free or paid, so it treats it as free/,
      ).length,
    ).toBeGreaterThanOrEqual(1);
  });

  it("marks a key with an empty prepay balance, and offers Check again", () => {
    render(
      <Card
        view={geminiView({
          key_check: { at: "x", outcome: "no_credit", listed: [], listed_total: 49, tier: "paid" },
        })}
      />,
    );
    expect(screen.getByText("Paid key (out of credit)")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Check again for Google Gemini" })).toBeTruthy();
    expect(
      screen.getByText(
        "This key is paid, but its prepay balance is empty, so Google has stopped it. Add credit at aistudio.google.com/billing, then press Check again.",
      ),
    ).toBeTruthy();
  });
});

describe("a free key PM won't use here", () => {
  const refused = (over: Partial<KeyCheckResult> = {}): KeyCheckResult => ({
    outcome: "free_key_refused",
    saved: false,
    listed: [],
    listed_total: 0,
    tier: "free",
    refusal: UK,
    ...over,
  });

  async function typeAndSubmit() {
    render(<Card view={geminiView({ key: "absent", key_check: null, tier: null })} />);
    fireEvent.change(keyField(), { target: { value: "AIzaFreeKeyNotReal0123456789abcdefg" } });
    fireEvent.submit(form());
    return screen.findByText("PM didn't save this key.");
  }

  it("says what PM read, where it thinks you are, and the way out, with both links", async () => {
    m.saveDirectKey.mockResolvedValue(refused());
    await typeAndSubmit();
    expect(
      screen.getByText(
        /It's a free key, and PM thinks you're in the United Kingdom: it read your computer's time zone as Europe\/London\./,
      ),
    ).toBeTruthy();
    expect(
      screen.getByText(
        "Not in the United Kingdom? Set your computer's time zone to where you really are, then press Check again.",
      ),
    ).toBeTruthy();
    const billing = screen.getByRole("link", { name: "Link billing in AI Studio →" });
    expect(billing.getAttribute("href")).toBe("https://aistudio.google.com/projects");
    expect(billing.getAttribute("rel")).toBe("noreferrer");
    expect(screen.getByRole("link", { name: "Add prepay credit →" }).getAttribute("href")).toBe(
      "https://aistudio.google.com/billing",
    );
    // The callout is the whole message: the field carries no shorter second one.
    expect(
      screen.queryByText("Not saved. It's a free key and PM only uses paid keys where you are."),
    ).toBeNull();
    // The typed key is still in the field.
    expect(keyField().value).toBe("AIzaFreeKeyNotReal0123456789abcdefg");
  });

  it("sits inside the one live region, so it is announced once", async () => {
    m.saveDirectKey.mockResolvedValue(refused());
    const title = await typeAndSubmit();
    const region = title.closest('[role="status"]');
    expect(region).not.toBeNull();
    // And no alert role of its own to fight it.
    expect(title.closest('[role="alert"]')).toBeNull();
  });

  it("checks again with the SAME typed key through a real submit button", async () => {
    m.saveDirectKey.mockResolvedValue(refused());
    await typeAndSubmit();
    const again = screen.getByRole("button", { name: "Check again" });
    expect(again.getAttribute("type")).toBe("submit");
    // The upgrade went through: the same key now checks out and is kept.
    m.saveDirectKey.mockResolvedValue({
      outcome: "ok",
      saved: true,
      listed: LISTED,
      listed_total: 49,
      tier: "paid",
    });
    fireEvent.submit(form());
    await waitFor(() => expect(m.saveDirectKey).toHaveBeenCalledTimes(2));
    expect(m.saveDirectKey).toHaveBeenLastCalledWith(
      "gemini",
      "AIzaFreeKeyNotReal0123456789abcdefg",
    );
    await waitFor(() => expect(screen.queryByText("PM didn't save this key.")).toBeNull());
  });

  it("says it couldn't tell, when it couldn't, and still names the zone", async () => {
    m.saveDirectKey.mockResolvedValue(
      refused({ tier: "unknown", refusal: { ...UK, basis: "unknown" } }),
    );
    await typeAndSubmit();
    expect(
      screen.getByText(
        /^PM couldn't tell whether it's free or paid, so it treats it as free, and PM thinks you're in the United Kingdom/,
      ),
    ).toBeTruthy();
  });

  it("says it can't place the computer, and offers no 'not there?' line", async () => {
    m.saveDirectKey.mockResolvedValue(
      refused({
        refusal: {
          kind: "unplaceable",
          label: "somewhere PM can't place",
          zone: "UTC",
          basis: "free",
        },
      }),
    );
    await typeAndSubmit();
    expect(
      screen.getByText(
        "It's a free key, and PM can't tell where this computer is (its time zone reads as UTC), so it only uses paid Gemini keys. Set your computer's time zone to where you are, or make the project paid in AI Studio, then press Check again.",
      ),
    ).toBeTruthy();
    expect(screen.queryByText(/^Not in /)).toBeNull();
  });

  it("keeps the Check again on one node while it works, aria-disabled and never disabled", async () => {
    m.saveDirectKey.mockResolvedValueOnce(refused());
    await typeAndSubmit();
    let finish: (r: KeyCheckResult) => void = () => {};
    m.saveDirectKey.mockReturnValueOnce(new Promise<KeyCheckResult>((r) => (finish = r)));
    const save = screen.getByRole("button", { name: "Save and check" });
    fireEvent.submit(form());
    await waitFor(() => expect(save.getAttribute("aria-disabled")).toBe("true"));
    expect((save as HTMLButtonElement).disabled).toBe(false);
    expect(screen.getByRole("button", { name: "Checking..." })).toBe(save);
    await act(async () => finish(refused()));
  });

  it("keeps the refusal's own Check again mounted, focused and aria-disabled while it re-sends", async () => {
    m.saveDirectKey.mockResolvedValueOnce(refused());
    await typeAndSubmit();
    const button = screen.getByRole("button", { name: "Check again" });
    button.focus();
    expect(document.activeElement).toBe(button);
    let finish: (r: KeyCheckResult) => void = () => {};
    m.saveDirectKey.mockReturnValueOnce(new Promise<KeyCheckResult>((r) => (finish = r)));
    fireEvent.click(button);
    await waitFor(() => expect(button.getAttribute("aria-disabled")).toBe("true"));
    // The very same node, still in the document, still holding focus; never `disabled`.
    expect(button.isConnected).toBe(true);
    expect(document.activeElement).toBe(button);
    expect((button as HTMLButtonElement).disabled).toBe(false);
    expect(screen.getByRole("button", { name: "Check again" })).toBe(button);
    // A second refusal replaces the held one with the new answer.
    await act(async () => finish(refused()));
    expect(screen.getByText("PM didn't save this key.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Check again" }).getAttribute("aria-disabled")).toBe(
      null,
    );
  });

  it("ends the held refusal when the typed key is edited", async () => {
    m.saveDirectKey.mockResolvedValueOnce(refused());
    await typeAndSubmit();
    fireEvent.change(keyField(), { target: { value: "AIzaAnotherNotReal0123456789abcdefg" } });
    expect(screen.queryByText("PM didn't save this key.")).toBeNull();
  });

  it("names the busy answer as nothing decided, not a connection problem", async () => {
    m.saveDirectKey.mockResolvedValue({
      outcome: "busy",
      saved: false,
      listed: [],
      listed_total: 0,
    });
    render(<Card view={geminiView({ key: "absent", key_check: null, tier: null })} />);
    fireEvent.change(keyField(), { target: { value: "AIzaBusyKeyNotReal0123456789abcdefgh" } });
    fireEvent.submit(form());
    const text = await screen.findByText(/Nothing was decided/);
    expect(text.textContent).not.toMatch(/connection/);
  });
});

describe("a stored key that reads free where PM only uses paid ones", () => {
  const paused = () => geminiView({ tier: "free", key_check: FREE_CHECK, blocked: UK });

  it("shows the paused callout and the badge, and offers Check again", () => {
    render(<Card view={paused()} />);
    expect(screen.getByText("Free key (paused)")).toBeTruthy();
    expect(
      screen.getByText(
        "Gemini is paused. Your key reads as free, and PM thinks you're in the United Kingdom (it read your computer's time zone as Europe/London). Google's terms only allow paid keys there, so PM won't use it. Make the project paid in AI Studio, then press Check again.",
      ),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Check again for Google Gemini" })).toBeTruthy();
  });

  it("leads with Google's own word, as DD-MM-YYYY, when it told PM the project is free", () => {
    render(
      <Card
        view={geminiView({
          tier: "free",
          key_check: { ...FREE_CHECK, flipped_free_at: "2026-11-26T09:30:00Z" },
          blocked: UK,
        })}
      />,
    );
    expect(
      screen.getByText(
        /^Google told PM on 26-11-2026 that this project is on the free tier\. Gemini is paused\./,
      ),
    ).toBeTruthy();
  });

  it("re-asks the stored key, and says it is neutral about a key that still reads free", async () => {
    m.recheckDirectKey.mockResolvedValue({
      outcome: "ok",
      saved: true,
      listed: LISTED,
      listed_total: 49,
      tier: "free",
      refusal: UK,
    } satisfies KeyCheckResult);
    render(<Card view={paused()} />);
    fireEvent.click(screen.getByRole("button", { name: "Check again for Google Gemini" }));
    expect(await screen.findByText("Checked: this key still reads as free.")).toBeTruthy();
    expect(m.recheckDirectKey).toHaveBeenCalledWith("gemini");
    // "Saved." never sits beside a callout that says paused.
    expect(screen.queryByText(/Saved\./)).toBeNull();
    expect(screen.getByText(/^Gemini is paused\./)).toBeTruthy();
  });

  it("says out loud, once, that a view has turned paused after a re-read", async () => {
    const { rerender } = render(
      <Card view={geminiView({ tier: "free", key_check: FREE_CHECK })} />,
    );
    expect(screen.queryByText("Google Gemini is now paused.")).toBeNull();
    rerender(<Card view={paused()} />);
    await waitFor(() => expect(screen.getByText("Google Gemini is now paused.")).toBeTruthy());
    // Not at first render of an already-paused card.
    cleanup();
    render(<Card view={paused()} />);
    expect(screen.queryByText("Google Gemini is now paused.")).toBeNull();
  });
});

describe("Google said the project is free, outside the places that pause it", () => {
  it("shows the dated danger line, and no paused callout", () => {
    render(
      <Card
        view={geminiView({
          tier: "free",
          key_check: { ...FREE_CHECK, flipped_free_at: "2026-11-26T09:30:00Z" },
        })}
      />,
    );
    expect(
      screen.getByText(
        "Google told PM on 26-11-2026 that this project is now on the free tier, so Google may use what you send to improve its products. Make the project paid in AI Studio, then press Check again.",
      ),
    ).toBeTruthy();
    expect(screen.queryByText(/is paused/)).toBeNull();
  });

  it("announces that Google says the key is free when the flag first appears", async () => {
    const paid = geminiView();
    const { rerender } = render(<Card view={paid} />);
    rerender(
      <Card
        view={geminiView({
          tier: "free",
          key_check: { ...FREE_CHECK, flipped_free_at: "2026-11-26T09:30:00Z" },
        })}
      />,
    );
    await waitFor(() =>
      expect(screen.getByText("Google says this key is on the free tier.")).toBeTruthy(),
    );
  });
});
