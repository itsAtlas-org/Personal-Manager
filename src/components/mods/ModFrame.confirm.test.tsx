// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// What ModFrame does for PR C's three page-side rules (K1, K5), over the same stand-in frame as
// ModFrame.test.tsx: PM's own yes/no for a MOD-2003 (the frame is told nothing while it is open, never
// sees the token, and gets what Rust answers to the person's word), the "Set it up" strip for MOD-5007,
// and the relay of a frame's `flushed` to Rust.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ModFrame } from "./ModFrame";
import { ModPageHooksContext, type ModPageHooks } from "./modPageHooks";
import { FLOOD_BURST, MOD_ERRORS, PORT_MESSAGE } from "./modBridge";
import { PROMPT_GUARD_MS } from "./ModPermissionPrompt";

const modFrameOpen = vi.fn();
const modFrameClose = vi.fn();
const modCall = vi.fn();
const modConfirm = vi.fn();
const modFrameFlushed = vi.fn();

vi.mock("../../lib/ipc", () => ({
  modsHeartbeat: () => Promise.resolve(),
  modPermissionDecide: () => Promise.resolve(),
  modAiPromptInfo: () => Promise.resolve(),
  modFrameOpen: (modId: string, onEvent: (message: unknown) => void) =>
    modFrameOpen(modId, onEvent),
  modFrameClose: (session: string) => modFrameClose(session),
  modCall: (session: string, method: string, args: unknown) => modCall(session, method, args),
  modEventsAck: () => Promise.resolve(),
  modThemeChanged: () => Promise.resolve(),
  modConfirm: (token: string, accept: boolean) => modConfirm(token, accept),
  modFrameFlushed: (session: string) => modFrameFlushed(session),
}));

vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    modePref: "system",
    modeSource: "system",
    accent: "mono",
    colorblind: false,
    depth: "standard",
    autoLocation: "",
  }),
}));
vi.mock("../../lib/reader", () => ({ useReader: () => ({ openReaderById: vi.fn() }) }));

const ENTRY = "pmmod://localhost/acme.tool/app/index.html?entry=" + "a".repeat(64);
// Token-shaped (32 hex digits) but plainly fake: a random-looking literal reads as an API key to the
// secret scan (gitleaks).
const TOKEN = "ab".repeat(16);
const TOKEN_2 = "cd".repeat(16);
const ADDRESS = "https://example.com/some/long/path?with=query";

let ports: MessagePort[] = [];

beforeEach(() => {
  vi.clearAllMocks();
  let n = 0;
  modFrameOpen.mockImplementation(() => {
    n += 1;
    return Promise.resolve({ entryUrl: ENTRY, session: `session-${n}` });
  });
  modFrameClose.mockResolvedValue(undefined);
  modFrameFlushed.mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
  for (const port of ports) port.close();
  ports = [];
});

const asking = (token = TOKEN, summary = `Open this address in your browser?\n${ADDRESS}`) => ({
  kind: "error",
  error: {
    code: MOD_ERRORS.needsConfirmation,
    message: "PM needs the person's OK before this can go ahead.",
    confirm: { token, summary },
  },
});

function mount(hooks: ModPageHooks = {}) {
  return render(
    <ModPageHooksContext.Provider value={hooks}>
      <ModFrame modId="acme.tool" name="Acme Tool" tier="second" />
    </ModPageHooksContext.Provider>,
  );
}

async function theFrame(): Promise<HTMLIFrameElement> {
  return waitFor(() => {
    const frame = document.querySelector("iframe");
    if (!frame) throw new Error("no frame yet");
    return frame;
  });
}

function connect(frame: HTMLIFrameElement): MessagePort {
  const win = frame.contentWindow!;
  const post = vi.fn();
  vi.spyOn(win, "postMessage").mockImplementation(post);
  act(() => {
    window.dispatchEvent(
      new MessageEvent("message", {
        data: { pm: "connect", version: 1 },
        origin: "null",
        source: win,
      }),
    );
  });
  const [message, , transfer] = post.mock.calls[0] as [unknown, string, MessagePort[]];
  expect(message).toEqual(PORT_MESSAGE);
  ports.push(transfer[0]);
  return transfer[0];
}

/** Every message the frame's port receives, in order. */
function listen(port: MessagePort): unknown[] {
  const got: unknown[] = [];
  port.onmessage = (e: MessageEvent) => got.push(e.data);
  return got;
}

const tick = (ms = 20) => act(() => new Promise((r) => setTimeout(r, ms)));

/** Let the guard that holds Accept for half a second after the question appears pass. */
async function pastGuard() {
  await act(() => new Promise((r) => setTimeout(r, PROMPT_GUARD_MS + 60)));
}

const events = () =>
  modFrameOpen.mock.calls[modFrameOpen.mock.calls.length - 1][1] as (message: unknown) => void;

describe("PM's own yes/no for a MOD-2003", () => {
  it("draws the question in an alert dialog, tells the frame nothing, and never gives it the token", async () => {
    modCall.mockResolvedValue(asking());
    mount();
    const port = connect(await theFrame());
    const got = listen(port);
    port.postMessage({ id: 1, method: "shell.openLink", args: { url: ADDRESS } });
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    // Named by its title (the mod and its tier), described by the question.
    expect(dialog.getAttribute("aria-labelledby")).toBeTruthy();
    expect(document.getElementById(dialog.getAttribute("aria-labelledby")!)?.textContent).toContain(
      "Acme Tool",
    );
    expect(dialog.textContent).toContain("Reviewed by Atlas");
    const described = document.getElementById(dialog.getAttribute("aria-describedby")!)!;
    expect(described.textContent).toContain("Open this address in your browser?");
    // The address is on its own line, in monospace.
    const line = within(described).getByText(ADDRESS);
    expect(line.className).toContain("font-mono");
    // Nothing reached the frame: it is still waiting for its answer.
    await tick();
    expect(got).toEqual([]);
    expect(modConfirm).not.toHaveBeenCalled();
  });

  it("starts with focus on Decline, and Accept is held for half a second", async () => {
    modCall.mockResolvedValue(asking());
    mount();
    const port = connect(await theFrame());
    port.postMessage({ id: 1, method: "shell.openLink", args: {} });
    const dialog = await screen.findByRole("alertdialog");
    const accept = within(dialog).getByRole("button", { name: "Accept" }) as HTMLButtonElement;
    const decline = within(dialog).getByRole("button", { name: "Decline" });
    await waitFor(() => expect(document.activeElement).toBe(decline));
    expect(accept.disabled).toBe(true);
    fireEvent.click(accept);
    expect(modConfirm).not.toHaveBeenCalled();
    await pastGuard();
    expect(accept.disabled).toBe(false);
  });

  it("Accept sends the person's yes with PM's token and relays what Rust answered, whatever the frame sent", async () => {
    modCall.mockResolvedValue(asking());
    modConfirm.mockResolvedValue({ kind: "value", value: { opened: true } });
    mount();
    const port = connect(await theFrame());
    const got = listen(port);
    port.postMessage({ id: 4, method: "shell.openLink", args: { url: "https://evil.example" } });
    const dialog = await screen.findByRole("alertdialog");
    await pastGuard();
    fireEvent.click(within(dialog).getByRole("button", { name: "Accept" }));
    await waitFor(() => expect(got).toHaveLength(1));
    expect(modConfirm).toHaveBeenCalledWith(TOKEN, true);
    expect(got[0]).toEqual({ id: 4, ok: true, result: { opened: true } });
    expect(screen.queryByRole("alertdialog")).toBeNull();
    // The frame's own arguments went to the backend with the first call and nowhere else.
    expect(modCall).toHaveBeenCalledTimes(1);
  });

  it("Decline, Escape and closing the dialog all say no, and the frame is told what Rust answers", async () => {
    const declined = {
      kind: "error",
      error: { code: MOD_ERRORS.declined, message: "The person chose not to allow that." },
    };
    for (const how of ["button", "escape", "scrim"] as const) {
      cleanup();
      modConfirm.mockReset();
      modConfirm.mockResolvedValue(declined);
      modCall.mockResolvedValue(asking());
      mount();
      const port = connect(await theFrame());
      const got = listen(port);
      port.postMessage({ id: 9, method: "milestones.delete", args: { id: 1 } });
      const dialog = await screen.findByRole("alertdialog");
      if (how === "button")
        fireEvent.click(within(dialog).getByRole("button", { name: "Decline" }));
      else if (how === "escape") fireEvent.keyDown(window, { key: "Escape" });
      else fireEvent.mouseDown(dialog.parentElement!);
      await waitFor(() => expect(got).toHaveLength(1));
      expect(modConfirm).toHaveBeenCalledTimes(1);
      expect(modConfirm).toHaveBeenCalledWith(TOKEN, false);
      expect(got[0]).toEqual({
        id: 9,
        ok: false,
        error: { code: MOD_ERRORS.declined, message: "The person chose not to allow that." },
      });
      ports.forEach((p) => p.close());
      ports = [];
    }
  });

  it("keeps the focus trapped in the dialog, and hands it back when the dialog closes", async () => {
    modCall.mockResolvedValue(asking());
    modConfirm.mockResolvedValue({ kind: "value", value: null });
    // Something outside holds focus when the question arrives.
    const opener = document.createElement("button");
    document.body.appendChild(opener);
    opener.focus();
    mount();
    const port = connect(await theFrame());
    opener.focus();
    port.postMessage({ id: 1, method: "shell.openLink", args: {} });
    const dialog = await screen.findByRole("alertdialog");
    await pastGuard();
    const accept = within(dialog).getByRole("button", { name: "Accept" });
    const decline = within(dialog).getByRole("button", { name: "Decline" });
    // Tab off the last control wraps to the first; Shift+Tab off the first wraps to the last.
    accept.focus();
    fireEvent.keyDown(accept, { key: "Tab" });
    expect(document.activeElement).toBe(decline);
    decline.focus();
    fireEvent.keyDown(decline, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(accept);
    fireEvent.click(decline);
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(opener));
    opener.remove();
  });

  it("is on top of another modal dialog, which stays put: one Escape declines only the question", async () => {
    modCall.mockResolvedValue(asking());
    modConfirm.mockResolvedValue({
      kind: "error",
      error: { code: MOD_ERRORS.declined, message: "no" },
    });
    const { Dialog } = await import("../ui");
    const onLowerClose = vi.fn();
    render(
      <Dialog open title="Under it" onClose={onLowerClose}>
        <p>below</p>
      </Dialog>,
    );
    mount();
    const port = connect(await theFrame());
    listen(port);
    port.postMessage({ id: 1, method: "shell.openLink", args: {} });
    await screen.findByRole("alertdialog");
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(modConfirm).toHaveBeenCalledWith(TOKEN, false));
    expect(onLowerClose).not.toHaveBeenCalled();
  });

  it("asks one question at a time per frame: a second is never drawn and its token is burnt with a no", async () => {
    modCall
      .mockResolvedValueOnce(asking(TOKEN, "First question?"))
      .mockResolvedValueOnce(asking(TOKEN_2, "Second question?"));
    modConfirm.mockImplementation((token: string) =>
      Promise.resolve(
        token === TOKEN_2
          ? { kind: "error", error: { code: MOD_ERRORS.declined, message: "no" } }
          : { kind: "value", value: { done: 1 } },
      ),
    );
    mount();
    const port = connect(await theFrame());
    const got = listen(port);
    port.postMessage({ id: 1, method: "milestones.delete", args: {} });
    await screen.findByText("First question?");
    port.postMessage({ id: 2, method: "milestones.delete", args: {} });
    await waitFor(() => expect(modConfirm).toHaveBeenCalledWith(TOKEN_2, false));
    expect(screen.queryByText("Second question?")).toBeNull();
    // The first is still waiting for the person.
    expect(screen.getByText("First question?")).toBeTruthy();
    await waitFor(() => expect(got).toHaveLength(1));
    expect(got[0]).toMatchObject({ id: 2, ok: false, error: { code: MOD_ERRORS.declined } });
  });

  it.each(["session.closed", "mod.paused"])(
    "takes the question down, answering nothing, when the session ends (%s)",
    async (goodbye) => {
      modCall.mockResolvedValue(asking());
      mount();
      const port = connect(await theFrame());
      const got = listen(port);
      port.postMessage({ id: 1, method: "shell.openLink", args: {} });
      await screen.findByRole("alertdialog");
      act(() => events()({ seq: 1, name: goodbye, payload: {} }));
      await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
      await tick();
      // Rust revoked the token with the session: PM doesn't send it, and the frame gets no answer.
      expect(modConfirm).not.toHaveBeenCalled();
      expect(got.filter((m) => (m as { id?: number }).id === 1)).toEqual([]);
    },
  );

  it("takes the question down when the frame goes (a tab switch unmounts it)", async () => {
    modCall.mockResolvedValue(asking());
    const view = mount();
    const port = connect(await theFrame());
    port.postMessage({ id: 1, method: "shell.openLink", args: {} });
    await screen.findByRole("alertdialog");
    view.unmount();
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(modConfirm).not.toHaveBeenCalled();
  });

  it("takes the question down when the frame is stopped for flooding", async () => {
    modCall.mockResolvedValue(asking());
    mount();
    const port = connect(await theFrame());
    port.postMessage({ id: 1, method: "shell.openLink", args: {} });
    await screen.findByRole("alertdialog");
    // A burst past the flood line stops the frame and ends its session.
    for (let i = 0; i < FLOOD_BURST + 5; i++) port.postMessage({ ack: 1 });
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(modConfirm).not.toHaveBeenCalled();
  });

  it("draws a question that is not PM's shape as the refusal it is, with no dialog", async () => {
    for (const confirm of [
      { token: "short", summary: "Q?" },
      { token: TOKEN.toUpperCase(), summary: "Q?" },
      { token: TOKEN, summary: "" },
      { token: TOKEN },
      "not an object",
    ]) {
      cleanup();
      modCall.mockResolvedValue({
        kind: "error",
        error: {
          code: MOD_ERRORS.needsConfirmation,
          message: "PM needs the person's OK.",
          confirm,
        },
      });
      mount();
      const port = connect(await theFrame());
      const got = listen(port);
      port.postMessage({ id: 3, method: "shell.openLink", args: {} });
      await waitFor(() => expect(got).toHaveLength(1));
      expect(screen.queryByRole("alertdialog")).toBeNull();
      expect(got[0]).toEqual({
        id: 3,
        ok: false,
        error: { code: MOD_ERRORS.needsConfirmation, message: "PM needs the person's OK." },
      });
      expect(modConfirm).not.toHaveBeenCalled();
      ports.forEach((p) => p.close());
      ports = [];
    }
  });

  it("draws an address as plain monospace text, however it is written, and clamps what crossed IPC", async () => {
    const hostile = "<img src=x onerror=alert(1)>" + "a".repeat(2_000);
    modCall.mockResolvedValue(asking(TOKEN, `Q?\n${hostile}`));
    mount();
    const port = connect(await theFrame());
    port.postMessage({ id: 1, method: "shell.openLink", args: {} });
    const dialog = await screen.findByRole("alertdialog");
    expect(document.querySelector("img")).toBeNull();
    const line = dialog.querySelector("p.font-mono")!;
    expect(line.textContent!.startsWith("<img src=x onerror=alert(1)>")).toBe(true);
    expect(line.textContent!.length).toBeLessThanOrEqual(401);
  });
});

describe("a summary with more lines than an address has", () => {
  it("says how many it did not draw, and draws no control or direction-changing character", async () => {
    const lines = Array.from({ length: 9 }, (_, i) => `line ${i}\u202e`);
    modCall.mockResolvedValue(asking(TOKEN, `Delete it\u0007?\n${lines.join("\n")}`));
    mount();
    const port = connect(await theFrame());
    port.postMessage({ id: 1, method: "milestones.delete", args: {} });
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog.querySelectorAll("p.font-mono")).toHaveLength(6);
    expect(dialog.textContent).toContain("3 more lines are not shown");
    expect(dialog.textContent).not.toContain("\u202e");
    expect(dialog.textContent).not.toContain("\u0007");
  });
});

describe("MOD-5007: the document engine isn't set up", () => {
  const notReady = {
    kind: "error",
    error: { code: MOD_ERRORS.engineNotReady, message: "The document engine isn't set up yet." },
  };

  it("tells the frame as it is, and offers PM's own Set it up button", async () => {
    modCall.mockResolvedValue(notReady);
    const openEngineSetup = vi.fn();
    mount({ openEngineSetup });
    const port = connect(await theFrame());
    const got = listen(port);
    port.postMessage({ id: 2, method: "documents.saveNote", args: {} });
    await waitFor(() => expect(got).toHaveLength(1));
    expect(got[0]).toMatchObject({ id: 2, ok: false, error: { code: MOD_ERRORS.engineNotReady } });
    fireEvent.click(await screen.findByRole("button", { name: "Set it up" }));
    expect(openEngineSetup).toHaveBeenCalledTimes(1);
    // It goes away once pressed.
    expect(screen.queryByRole("button", { name: "Set it up" })).toBeNull();
  });

  it("can be dismissed, and says nothing when no call was refused", async () => {
    modCall.mockResolvedValue(notReady);
    mount({ openEngineSetup: vi.fn() });
    expect(screen.queryByText(/document engine/)).toBeNull();
    const port = connect(await theFrame());
    port.postMessage({ id: 2, method: "documents.saveNote", args: {} });
    fireEvent.click(await screen.findByRole("button", { name: "Dismiss" }));
    expect(screen.queryByText(/document engine/)).toBeNull();
  });

  it("offers no button where nothing can take the person there, but still says why", async () => {
    modCall.mockResolvedValue(notReady);
    mount();
    const port = connect(await theFrame());
    port.postMessage({ id: 2, method: "documents.saveNote", args: {} });
    expect(await screen.findByText(/document engine/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Set it up" })).toBeNull();
  });

  it("is not drawn for a call the engine had nothing to do with", async () => {
    modCall.mockResolvedValue({
      kind: "error",
      error: { code: MOD_ERRORS.notGranted, message: "no" },
    });
    mount({ openEngineSetup: vi.fn() });
    const port = connect(await theFrame());
    const got = listen(port);
    port.postMessage({ id: 2, method: "documents.saveNote", args: {} });
    await waitFor(() => expect(got).toHaveLength(1));
    expect(screen.queryByText(/document engine/)).toBeNull();
  });
});

describe("the suspend answer", () => {
  it("relays a frame's flushed to Rust for its own session, and only that", async () => {
    mount();
    const port = connect(await theFrame());
    port.postMessage({ flushed: true, session: "someone-elses" });
    await waitFor(() => expect(modFrameFlushed).toHaveBeenCalledTimes(1));
    expect(modFrameFlushed).toHaveBeenCalledWith("session-1");
    // Anything that isn't exactly flushed: true is not an answer.
    port.postMessage({ flushed: "true" });
    port.postMessage({ flushed: false });
    await tick();
    expect(modFrameFlushed).toHaveBeenCalledTimes(1);
  });

  it("charges the answer to the frame's flood line like any message", async () => {
    mount();
    const port = connect(await theFrame());
    for (let i = 0; i < FLOOD_BURST + 5; i++) port.postMessage({ flushed: true });
    await waitFor(() => expect(modFrameClose).toHaveBeenCalledWith("session-1"));
    expect(modFrameFlushed.mock.calls.length).toBeLessThanOrEqual(FLOOD_BURST);
  });
});
