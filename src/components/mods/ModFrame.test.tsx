// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// ModFrame's contract (Mods spec §1.4): the sealed iframe and PM's band, one session per mount, a port
// only for the frame PM mounted, calls relayed under PM's own session, a navigation ends the session,
// heights clamped, and the session closed on unmount; the backend's answers read as a value, a refusal
// or a page action PM performs from its live theme; and a frame that floods has its port closed and
// its session ended. jsdom loads nothing into the frame, so the tests play the frame: they fire its
// `load` events, post its messages with the frame as `source`, and talk to PM over the port PM hands
// it.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { HelpOverlay } from "../HelpOverlay";
import { HELP as HELP_TEXT, HelpContext } from "../../lib/help";
import { ModFrame, type ModTier } from "./ModFrame";
import {
  ModPageHooksContext,
  NOTICE_MAX_CHARS,
  readNotice,
  type ModPageHooks,
} from "./modPageHooks";
import { PROMPT_GUARD_MS } from "./ModPermissionPrompt";
import {
  FLOOD_BURST,
  INTERNAL_MESSAGE,
  MAX_FRAME_HEIGHT,
  MAX_IN_FLIGHT,
  MIN_FRAME_HEIGHT,
  MOD_ERRORS,
  PORT_MESSAGE,
} from "./modBridge";

const modFrameOpen = vi.fn();
const modFrameClose = vi.fn();
const modCall = vi.fn();
const modEventsAck = vi.fn();
const modThemeChanged = vi.fn();
const modsHeartbeat = vi.fn();
const modPermissionDecide = vi.fn();
const modAiPromptInfo = vi.fn();
const modFrameFlushed = vi.fn();
const modConfirm = vi.fn();

vi.mock("../../lib/ipc", () => ({
  modsHeartbeat: (visible: boolean) => modsHeartbeat(visible),
  // Every argument is forwarded as it came, so a test sees whether a limit travelled with the yes.
  modPermissionDecide: (...args: unknown[]) => modPermissionDecide(...args),
  modAiPromptInfo: (modId: string) => modAiPromptInfo(modId),
  modFrameOpen: (modId: string, onEvent: (message: unknown) => void) =>
    modFrameOpen(modId, onEvent),
  modFrameClose: (session: string) => modFrameClose(session),
  modCall: (session: string, method: string, args: unknown) => modCall(session, method, args),
  modEventsAck: (session: string, seq: number) => modEventsAck(session, seq),
  modThemeChanged: (session: string, theme: unknown) => modThemeChanged(session, theme),
  modFrameFlushed: (session: string) => modFrameFlushed(session),
  modConfirm: (token: string, accept: boolean) => modConfirm(token, accept),
}));

// The theme a `theme.get` is answered from: the same stub the other component tests use (the real
// ThemeProvider pulls in IPC), with the axes a mod is told about.
const THEME = {
  system: "slate",
  mode: "dark",
  modePref: "system",
  modeSource: "system",
  accent: "mono",
  colorblind: false,
  depth: "standard",
  autoLocation: "",
};
vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => THEME,
}));

// The app's reader, which `ui.openDocument` opens: the real provider fetches through IPC.
const openReaderById = vi.fn();
vi.mock("../../lib/reader", () => ({
  useReader: () => ({ openReaderById: (id: number) => openReaderById(id) }),
}));

const ENTRY = "pmmod://localhost/acme.tool/app/index.html?entry=" + "a".repeat(64);
const CONNECT = { pm: "connect", version: 1 };

let ports: MessagePort[] = [];

beforeEach(() => {
  vi.clearAllMocks();
  let n = 0;
  modFrameOpen.mockImplementation(() => {
    n += 1;
    return Promise.resolve({ entryUrl: ENTRY, session: `session-${n}` });
  });
  modFrameClose.mockResolvedValue(undefined);
  modCall.mockResolvedValue({ kind: "value", value: { answer: 42 } });
  modEventsAck.mockResolvedValue(undefined);
  modThemeChanged.mockResolvedValue(undefined);
  modsHeartbeat.mockResolvedValue(undefined);
  modFrameFlushed.mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  // The clock a flood test held still, and the frame windows' spies.
  vi.restoreAllMocks();
  for (const port of ports) port.close();
  ports = [];
});

function mount(tier: ModTier = "second") {
  return render(<ModFrame modId="acme.tool" name="Acme Tool" tier={tier} />);
}

const frameNow = () => document.querySelector("iframe");

async function theFrame(): Promise<HTMLIFrameElement> {
  return waitFor(() => {
    const frame = frameNow();
    if (!frame) throw new Error("no frame yet");
    return frame;
  });
}

/** Post `data` on PM's window as if it came from `source`. */
function postFrom(source: Window | null, data: unknown, origin = "null") {
  act(() => {
    window.dispatchEvent(new MessageEvent("message", { data, origin, source }));
  });
}

/** The frame's side of the handshake: ask for the port, and take the one PM transfers. */
function connect(frame: HTMLIFrameElement): { port: MessagePort; post: ReturnType<typeof vi.fn> } {
  const win = frame.contentWindow!;
  const post = vi.fn();
  vi.spyOn(win, "postMessage").mockImplementation(post);
  postFrom(win, CONNECT);
  expect(post).toHaveBeenCalledTimes(1);
  const [message, target, transfer] = post.mock.calls[0] as [unknown, string, MessagePort[]];
  expect(message).toEqual(PORT_MESSAGE);
  expect(target).toBe("*");
  expect(transfer).toHaveLength(1);
  ports.push(transfer[0]);
  return { port: transfer[0], post };
}

/** Send one message over the frame's port and wait for PM's answer. */
function ask(port: MessagePort, message: unknown): Promise<unknown> {
  return new Promise((resolve) => {
    port.onmessage = (e: MessageEvent) => resolve(e.data);
    port.postMessage(message);
  });
}

describe("ModFrame", () => {
  it("opens one session and mounts the sealed frame on its entry URL, under PM's band", async () => {
    mount();
    const frame = await theFrame();
    expect(modFrameOpen).toHaveBeenCalledTimes(1);
    expect(modFrameOpen).toHaveBeenCalledWith("acme.tool", expect.any(Function));
    expect(frame.getAttribute("src")).toBe(ENTRY);
    expect(frame.getAttribute("sandbox")).toBe("allow-scripts");
    expect(frame.getAttribute("allow")).toBe("");
    expect(frame.hasAttribute("srcdoc")).toBe(false);
    const band = screen.getByText("Acme Tool", { selector: "span" }).parentElement!;
    expect(band.textContent).toBe("ModAcme ToolReviewed by Atlas");
    // The band is PM's, outside the frame, and offers nothing to click.
    expect(frame.contains(band)).toBe(false);
    expect(band.querySelector("a, button, input")).toBeNull();
  });

  it("shows a tier it doesn't know as the least trusted one", async () => {
    render(<ModFrame modId="acme.tool" name="Acme Tool" tier={"atlas" as ModTier} />);
    await theFrame();
    expect(screen.getByText("Third party")).toBeTruthy();
  });

  it("draws the mod's name as text, never as markup", async () => {
    render(<ModFrame modId="acme.tool" name={'<img src=x onerror="alert(1)">'} tier="third" />);
    expect((await theFrame()).title).toBe('<img src=x onerror="alert(1)">');
    expect(document.querySelector("img")).toBeNull();
    expect(screen.getByText('<img src=x onerror="alert(1)">', { selector: "span" })).toBeTruthy();
  });

  it("hands its frame a port, once", async () => {
    mount();
    const frame = await theFrame();
    const { post } = connect(frame);
    // A second request in the same session gets nothing: the frame already holds its port.
    postFrom(frame.contentWindow, CONNECT);
    expect(post).toHaveBeenCalledTimes(1);
  });

  it("ignores a message from anything but its own frame", async () => {
    mount();
    const frame = await theFrame();
    const post = vi.spyOn(frame.contentWindow!, "postMessage").mockImplementation(() => {});
    const other = document.createElement("iframe");
    document.body.appendChild(other);
    postFrom(other.contentWindow, CONNECT);
    postFrom(window, CONNECT);
    postFrom(null, CONNECT);
    expect(post).not.toHaveBeenCalled();
    other.remove();
  });

  it("ignores its own frame when the frame's origin isn't opaque, or the request is malformed", async () => {
    mount();
    const frame = await theFrame();
    const post = vi.spyOn(frame.contentWindow!, "postMessage").mockImplementation(() => {});
    postFrom(frame.contentWindow, CONNECT, "pmmod://localhost");
    postFrom(frame.contentWindow, CONNECT, "http://pmmod.localhost");
    postFrom(frame.contentWindow, { pm: "connect", version: 2 });
    postFrom(frame.contentWindow, { pm: "connect" });
    postFrom(frame.contentWindow, "connect");
    expect(post).not.toHaveBeenCalled();
  });

  it("relays a call under PM's own session, whatever the message names", async () => {
    mount();
    const frame = await theFrame();
    const { port } = connect(frame);
    const answer = await ask(port, {
      id: 7,
      method: "storage.get",
      args: { key: "x" },
      session: "someone-elses",
      modId: "atlas.map",
    });
    // The backend gets the arguments as the JSON text PM measured, so it can cap them before parsing.
    expect(modCall).toHaveBeenCalledWith("session-1", "storage.get", '{"key":"x"}');
    expect(answer).toEqual({ id: 7, ok: true, result: { answer: 42 } });
  });

  it("passes the backend's refusal back to the frame as its code and message", async () => {
    modCall.mockResolvedValue({
      kind: "error",
      error: {
        code: "MOD-4001",
        message: "That value changed since it was read: read it again.",
        revision: 3,
        extra: "never passed on",
      },
    });
    mount();
    const { port } = connect(await theFrame());
    expect(await ask(port, { id: 1, method: "storage.set", args: null })).toEqual({
      id: 1,
      ok: false,
      error: {
        code: "MOD-4001",
        message: "That value changed since it was read: read it again.",
        revision: 3,
      },
    });
  });

  it("answers its own failure, never what went wrong, when the backend or its answer breaks", async () => {
    const internal = {
      id: 1,
      ok: false,
      error: { code: MOD_ERRORS.internal, message: "PM couldn't complete that call." },
    };
    mount();
    const { port } = connect(await theFrame());
    // The IPC rejecting (its text is PM's), and answers that aren't in the reply shape.
    modCall.mockRejectedValueOnce("sqlite: no such table at /home/someone/pm.db");
    expect(await ask(port, { id: 1, method: "a.b" })).toEqual(internal);
    for (const broken of [
      { answer: 42 },
      "text",
      null,
      { kind: "error", error: { code: "nope", message: "x" } },
      { kind: "value" },
    ]) {
      modCall.mockResolvedValueOnce(broken);
      expect(await ask(port, { id: 1, method: "a.b" })).toEqual(internal);
    }
  });

  it("refuses a call it can't read, without reaching the backend", async () => {
    mount();
    const { port } = connect(await theFrame());
    expect(await ask(port, { id: 2, method: "bad method!" })).toMatchObject({
      id: 2,
      ok: false,
      error: { code: MOD_ERRORS.badArgs },
    });
    expect(await ask(port, { id: 3, method: "x.y", args: 10n })).toMatchObject({
      id: 3,
      ok: false,
      error: { code: MOD_ERRORS.badArgs },
    });
    expect(modCall).not.toHaveBeenCalled();
  });

  it("performs theme.get from PM's live theme once the backend has authorised it", async () => {
    const root = document.documentElement;
    root.style.setProperty("--bg", "oklch(0.2 0 0)");
    root.style.setProperty("--ink", "oklch(0.9 0 0)");
    // Not a custom property: never part of the answer.
    root.style.setProperty("color-scheme", "dark");
    modCall.mockResolvedValue({ kind: "page", action: "theme.get", args: null });
    mount();
    const { port } = connect(await theFrame());
    const answer = (await ask(port, { id: 1, method: "theme.get" })) as {
      ok: boolean;
      result: { vars: Record<string, string> };
    };
    expect(answer.ok).toBe(true);
    expect(answer.result).toEqual({
      vars: { "--bg": "oklch(0.2 0 0)", "--ink": "oklch(0.9 0 0)" },
      mode: "dark",
      system: "slate",
      accent: "mono",
      colorblind: false,
      depth: "standard",
    });
    root.removeAttribute("style");
  });

  it("opens PM's own reader on the document the backend authorised, and answers null", async () => {
    modCall.mockResolvedValue({ kind: "page", action: "ui.openDocument", args: { id: 12 } });
    mount();
    const { port } = connect(await theFrame());
    expect(await ask(port, { id: 1, method: "ui.openDocument", args: { id: 12 } })).toEqual({
      id: 1,
      ok: true,
      result: null,
    });
    expect(openReaderById).toHaveBeenCalledTimes(1);
    expect(openReaderById).toHaveBeenCalledWith(12);
  });

  describe("secrets.set and notify", () => {
    const SLOT = { name: "token", host: "api.example.com", header: "X-Api-Key", set: false };
    const mountWith = (hooks: ModPageHooks) =>
      render(
        <ModPageHooksContext.Provider value={hooks}>
          <ModFrame modId="acme.tool" name="Acme Tool" tier="second" />
        </ModPageHooksContext.Provider>,
      );

    it("shows PM's key field for the slot the backend named, and tells the mod only whether a key was held", async () => {
      const askSecret = vi.fn();
      modCall.mockResolvedValue({ kind: "page", action: "secrets.set", args: SLOT });
      mountWith({ askSecret });
      const { port } = connect(await theFrame());
      expect(await ask(port, { id: 1, method: "secrets.set", args: { name: "token" } })).toEqual({
        id: 1,
        ok: true,
        result: { set: false },
      });
      // The mod is named by PM's own record, never by anything the frame sent.
      expect(askSecret).toHaveBeenCalledWith(
        { modId: "acme.tool", name: "Acme Tool", tier: "second" },
        SLOT,
      );
    });

    it("shows a notice strip with the text and tone the backend checked, and answers null", async () => {
      const showNotice = vi.fn();
      modCall.mockResolvedValue({
        kind: "page",
        action: "notify",
        args: { text: "Rates updated", tone: "good" },
      });
      mountWith({ showNotice });
      const { port } = connect(await theFrame());
      expect(await ask(port, { id: 1, method: "notify", args: { text: "x" } })).toEqual({
        id: 1,
        ok: true,
        result: null,
      });
      expect(showNotice).toHaveBeenCalledWith(
        { modId: "acme.tool", name: "Acme Tool", tier: "second" },
        { text: "Rates updated", tone: "good" },
      );
    });

    it.each([
      ["secrets.set", SLOT],
      ["notify", { text: "hi", tone: "info" }],
    ])(
      "answers %s as PM's own failure, not as a success that showed nothing, when nothing draws it",
      async (action, args) => {
        modCall.mockResolvedValue({ kind: "page", action, args });
        mount();
        const { port } = connect(await theFrame());
        expect(await ask(port, { id: 1, method: action })).toEqual({
          id: 1,
          ok: false,
          error: { code: MOD_ERRORS.internal, message: INTERNAL_MESSAGE },
        });
      },
    );

    it.each([
      ["secrets.set", null],
      ["secrets.set", { ...SLOT, set: "yes" }],
      ["secrets.set", { ...SLOT, name: "" }],
      ["secrets.set", { name: "token" }],
      ["notify", null],
      ["notify", { text: "hi", tone: "loud" }],
      ["notify", { text: "", tone: "info" }],
      ["notify", { text: 5, tone: "info" }],
      ["notify", { text: "a".repeat(141), tone: "info" }],
    ])("draws nothing for %s with the arguments %j", async (action, args) => {
      const hooks = { askSecret: vi.fn(), showNotice: vi.fn() };
      modCall.mockResolvedValue({ kind: "page", action, args });
      mountWith(hooks);
      const { port } = connect(await theFrame());
      expect(await ask(port, { id: 1, method: action })).toEqual({
        id: 1,
        ok: false,
        error: { code: MOD_ERRORS.internal, message: INTERNAL_MESSAGE },
      });
      expect(hooks.askSecret).not.toHaveBeenCalled();
      expect(hooks.showNotice).not.toHaveBeenCalled();
    });

    it("counts a notice's length in characters, as the backend does", () => {
      const heavy = "é𝄞".repeat(NOTICE_MAX_CHARS / 2);
      expect(readNotice({ text: heavy, tone: "warn" }).text).toBe(heavy);
      expect(() => readNotice({ text: heavy + "x", tone: "warn" })).toThrow();
    });
  });

  describe("ui.openTaskEditor", () => {
    const mountWith = (hooks: ModPageHooks) =>
      render(
        <ModPageHooksContext.Provider value={hooks}>
          <ModFrame modId="acme.tool" name="Acme Tool" tier="second" />
        </ModPageHooksContext.Provider>,
      );
    const SOURCE = { modId: "acme.tool", name: "Acme Tool", tier: "second" };

    it("opens PM's editor on the task the backend found, and answers at once with opened", async () => {
      const openTaskEditor = vi.fn(() => true);
      modCall.mockResolvedValue({ kind: "page", action: "ui.openTaskEditor", args: { id: 7 } });
      mountWith({ openTaskEditor });
      const { port } = connect(await theFrame());
      expect(await ask(port, { id: 1, method: "ui.openTaskEditor", args: { id: 7 } })).toEqual({
        id: 1,
        ok: true,
        result: { opened: true },
      });
      // The mod is named by PM's own record, and the request is the backend's, never the frame's.
      expect(openTaskEditor).toHaveBeenCalledWith(SOURCE, { id: 7 });
    });

    it("opens it on a new task started from the backend's checked prefill", async () => {
      const openTaskEditor = vi.fn(() => true);
      const prefill = { title: "Call Sam", project: "Atlas", deadline: "2026-12-01" };
      modCall.mockResolvedValue({
        kind: "page",
        action: "ui.openTaskEditor",
        args: { new: prefill },
      });
      mountWith({ openTaskEditor });
      const { port } = connect(await theFrame());
      await ask(port, { id: 1, method: "ui.openTaskEditor", args: { new: { title: "x" } } });
      expect(openTaskEditor).toHaveBeenCalledWith(SOURCE, { new: prefill });
    });

    it("tells the mod it is asking too fast while an editor is already open (one at a time)", async () => {
      const openTaskEditor = vi.fn(() => false);
      modCall.mockResolvedValue({ kind: "page", action: "ui.openTaskEditor", args: { id: 7 } });
      mountWith({ openTaskEditor });
      const { port } = connect(await theFrame());
      expect(await ask(port, { id: 1, method: "ui.openTaskEditor", args: { id: 7 } })).toEqual({
        id: 1,
        ok: false,
        error: {
          code: MOD_ERRORS.rateLimited,
          message: "PM already has a task open for this mod.",
        },
      });
    });

    it("answers PM's own failure, not a success that opened nothing, when nothing draws the editor", async () => {
      modCall.mockResolvedValue({ kind: "page", action: "ui.openTaskEditor", args: { id: 7 } });
      mount();
      const { port } = connect(await theFrame());
      expect(await ask(port, { id: 1, method: "ui.openTaskEditor" })).toEqual({
        id: 1,
        ok: false,
        error: { code: MOD_ERRORS.internal, message: INTERNAL_MESSAGE },
      });
    });

    it.each([
      null,
      {},
      { id: 0 },
      { id: -3 },
      { id: 1.5 },
      { id: "7" },
      { new: null },
      { new: { title: 5, project: null, deadline: null } },
      { new: { title: "x", project: null, deadline: "2026-12-01T10:00" } },
      { new: { title: "x".repeat(1001), project: null, deadline: null } },
    ])("opens nothing for the arguments %j", async (args) => {
      const openTaskEditor = vi.fn(() => true);
      modCall.mockResolvedValue({ kind: "page", action: "ui.openTaskEditor", args });
      mountWith({ openTaskEditor });
      const { port } = connect(await theFrame());
      expect(await ask(port, { id: 1, method: "ui.openTaskEditor" })).toEqual({
        id: 1,
        ok: false,
        error: { code: MOD_ERRORS.internal, message: INTERNAL_MESSAGE },
      });
      expect(openTaskEditor).not.toHaveBeenCalled();
    });
  });

  it("performs no page action for a session that ended while the backend was answering", async () => {
    // Authorised, and the answer in flight, when the session ends (here the frame is unmounted, as
    // a pause, an uninstall, the lock or a vault switch unmounts it): the reader must not open.
    let answer: (reply: unknown) => void = () => {};
    modCall.mockImplementation(
      () =>
        new Promise((resolve) => {
          answer = resolve;
        }),
    );
    const view = mount();
    const { port } = connect(await theFrame());
    port.postMessage({ id: 1, method: "ui.openDocument", args: { id: 12 } });
    await waitFor(() => expect(modCall).toHaveBeenCalledTimes(1));
    view.unmount();
    answer({ kind: "page", action: "ui.openDocument", args: { id: 12 } });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(openReaderById).not.toHaveBeenCalled();
  });

  it.each(["session.closed", "mod.paused"])(
    "performs no page action once the backend has said %s, with the frame still mounted",
    async (goodbye) => {
      let answer: (reply: unknown) => void = () => {};
      modCall.mockImplementation(
        () =>
          new Promise((resolve) => {
            answer = resolve;
          }),
      );
      mount();
      const { port } = connect(await theFrame());
      port.postMessage({ id: 1, method: "ui.openDocument", args: { id: 12 } });
      await waitFor(() => expect(modCall).toHaveBeenCalledTimes(1));
      const channel = modFrameOpen.mock.calls[modFrameOpen.mock.calls.length - 1][1] as (
        message: unknown,
      ) => void;
      channel({ seq: 1, name: goodbye, payload: {} });
      answer({ kind: "page", action: "ui.openDocument", args: { id: 12 } });
      await new Promise((resolve) => setTimeout(resolve, 20));
      expect(openReaderById).not.toHaveBeenCalled();
    },
  );

  it("opens nothing for an id the frame sent that the backend didn't send back", async () => {
    // The action's arguments are the backend's, never the frame's: the frame asks for 99, the
    // backend authorised 12.
    modCall.mockResolvedValue({ kind: "page", action: "ui.openDocument", args: { id: 12 } });
    mount();
    const { port } = connect(await theFrame());
    await ask(port, { id: 1, method: "ui.openDocument", args: { id: 99 } });
    expect(openReaderById).toHaveBeenCalledTimes(1);
    expect(openReaderById).toHaveBeenCalledWith(12);
  });

  it.each([
    null,
    {},
    { id: "12" },
    { id: 0 },
    { id: -3 },
    { id: 1.5 },
    { id: Number.MAX_SAFE_INTEGER + 2 },
  ])("refuses to open the reader for the arguments %j, as PM's own failure", async (args) => {
    modCall.mockResolvedValue({ kind: "page", action: "ui.openDocument", args });
    mount();
    const { port } = connect(await theFrame());
    expect(await ask(port, { id: 1, method: "ui.openDocument" })).toEqual({
      id: 1,
      ok: false,
      error: { code: MOD_ERRORS.internal, message: INTERNAL_MESSAGE },
    });
    expect(openReaderById).not.toHaveBeenCalled();
  });

  it("transfers the vectors' buffer to the frame instead of copying it", async () => {
    // Two documents of three floats each, in the layout embeddings.rs documents.
    const buffer = new ArrayBuffer(16 + 2 * 8 + 2 * 3 * 4);
    const view = new DataView(buffer);
    view.setUint32(0, 1, true);
    view.setUint32(4, 2, true);
    view.setUint32(8, 3, true);
    view.setFloat64(16, 7, true);
    view.setFloat64(24, 5, true);
    [0.5, -1, 2, 0.25, 0, 4].forEach((x, i) => view.setFloat32(32 + 4 * i, x, true));
    modCall.mockResolvedValue(buffer);
    mount();
    const { port } = connect(await theFrame());
    const answer = (await ask(port, { id: 4, method: "embeddings.documentVectors" })) as {
      id: number;
      ok: boolean;
      result: { count: number; dimension: number; ids: Float64Array; vectors: Float32Array };
    };
    expect(answer.ok).toBe(true);
    expect(answer.result.count).toBe(2);
    expect(answer.result.dimension).toBe(3);
    expect(Array.from(answer.result.ids)).toEqual([7, 5]);
    expect(Array.from(answer.result.vectors)).toEqual([0.5, -1, 2, 0.25, 0, 4]);
    // Moved, not copied: the page's buffer is detached, and the frame's views share one new one.
    expect(buffer.byteLength).toBe(0);
    expect(answer.result.ids.buffer).toBe(answer.result.vectors.buffer);
    expect(answer.result.ids.buffer.byteLength).toBe(16 + 2 * 8 + 2 * 3 * 4);
  });

  it("answers a malformed binary reply as PM's own failure, never as vectors", async () => {
    const truncated = new ArrayBuffer(20);
    new DataView(truncated).setUint32(0, 1, true);
    new DataView(truncated).setUint32(4, 9, true);
    modCall.mockResolvedValue(truncated);
    mount();
    const { port } = connect(await theFrame());
    expect(await ask(port, { id: 1, method: "embeddings.documentVectors" })).toEqual({
      id: 1,
      ok: false,
      error: { code: MOD_ERRORS.internal, message: INTERNAL_MESSAGE },
    });
  });

  it("never performs a page action it doesn't know, whatever the answer asks for", async () => {
    modCall.mockResolvedValue({ kind: "page", action: "shell.openLink", args: { id: 1 } });
    mount();
    const { port } = connect(await theFrame());
    expect(await ask(port, { id: 1, method: "x.y" })).toEqual({
      id: 1,
      ok: false,
      error: { code: MOD_ERRORS.unknownMethod, message: "PM has no call by that name." },
    });
  });

  /** A frame that outran PM's page: the port is closed, the session ended, and PM says so where the
   *  frame was. Nothing it sends afterwards reaches the backend. */
  async function expectStopped(port: MessagePort, callsBefore: number) {
    await waitFor(() => expect(frameNow()).toBeNull());
    expect(modFrameClose).toHaveBeenCalledTimes(1);
    expect(modFrameClose).toHaveBeenCalledWith("session-1");
    expect(screen.getByRole("status").textContent).toBe(
      "This mod sent PM too many requests at once and was stopped.",
    );
    port.postMessage({ id: 5000, method: "a.b" });
    await act(() => new Promise((r) => setTimeout(r, 20)));
    expect(modCall).toHaveBeenCalledTimes(callsBefore);
    // Unmounting later doesn't end it twice.
    cleanup();
    expect(modFrameClose).toHaveBeenCalledTimes(1);
  }

  it("stops a frame at its 33rd call in flight: the port closes and the session ends", async () => {
    modCall.mockImplementation(() => new Promise(() => {}));
    mount();
    const { port } = connect(await theFrame());
    for (let id = 0; id < MAX_IN_FLIGHT; id += 1) port.postMessage({ id, method: "a.b" });
    await waitFor(() => expect(modCall).toHaveBeenCalledTimes(MAX_IN_FLIGHT));
    // The 32nd was fine: the frame is still there.
    expect(frameNow()).not.toBeNull();
    expect(modFrameClose).not.toHaveBeenCalled();
    port.postMessage({ id: 999, method: "a.b" });
    await expectStopped(port, MAX_IN_FLIGHT);
  });

  it("lets calls through again as earlier ones finish", async () => {
    const waiting: Array<() => void> = [];
    modCall.mockImplementation(
      () => new Promise((resolve) => waiting.push(() => resolve({ kind: "value", value: 1 }))),
    );
    mount();
    const { port } = connect(await theFrame());
    for (let id = 0; id < MAX_IN_FLIGHT; id += 1) port.postMessage({ id, method: "a.b" });
    await waitFor(() => expect(modCall).toHaveBeenCalledTimes(MAX_IN_FLIGHT));
    await act(async () => {
      waiting.shift()!();
      await new Promise((r) => setTimeout(r, 20));
    });
    port.postMessage({ id: 100, method: "a.b" });
    await waitFor(() => expect(modCall).toHaveBeenCalledTimes(MAX_IN_FLIGHT + 1));
    expect(frameNow()).not.toBeNull();
    expect(modFrameClose).not.toHaveBeenCalled();
  });

  it("stops a frame that bursts well past the backend's rate, even when every call finishes", async () => {
    // Time held still, so the burst is measured against the bucket alone.
    vi.spyOn(performance, "now").mockReturnValue(1000);
    mount();
    const { port } = connect(await theFrame());
    for (let id = 0; id < FLOOD_BURST; id += 1) {
      // Answered at once: never more than a few in flight, so only the rate can stop it.
      port.postMessage({ id, method: "a.b" });
      if (id % 8 === 7) await act(() => new Promise((r) => setTimeout(r, 0)));
    }
    await waitFor(() => expect(modCall).toHaveBeenCalledTimes(FLOOD_BURST));
    expect(frameNow()).not.toBeNull();
    port.postMessage({ id: 5001, method: "a.b" });
    await expectStopped(port, FLOOD_BURST);
  });

  it("counts the calls PM refuses itself against the same flood line", async () => {
    vi.spyOn(performance, "now").mockReturnValue(1000);
    mount();
    const { port } = connect(await theFrame());
    for (let id = 0; id < FLOOD_BURST + 1; id += 1) port.postMessage({ id, method: "bad method!" });
    await expectStopped(port, 0);
  });

  it("clamps a height request, and ignores one that isn't a number", async () => {
    mount();
    const frame = await theFrame();
    const { port } = connect(frame);
    const settle = () => new Promise((r) => setTimeout(r, 20));
    port.postMessage({ resize: 99_999 });
    await waitFor(() => expect(frame.style.height).toBe(`${MAX_FRAME_HEIGHT}px`));
    port.postMessage({ resize: -5 });
    await waitFor(() => expect(frame.style.height).toBe(`${MIN_FRAME_HEIGHT}px`));
    port.postMessage({ resize: 300.4 });
    await waitFor(() => expect(frame.style.height).toBe("300px"));
    port.postMessage({ resize: "900px; background: url(x)" });
    port.postMessage({ resize: Number.NaN });
    port.postMessage({ resize: Number.POSITIVE_INFINITY });
    await act(settle);
    expect(frame.style.height).toBe("300px");
    expect(frame.getAttribute("style")).toBe("height: 300px;");
  });

  it("treats a second load as a navigation: closes the session and stops the frame", async () => {
    mount();
    const frame = await theFrame();
    const { port } = connect(frame);
    fireEvent.load(frame);
    expect(modFrameClose).not.toHaveBeenCalled();
    fireEvent.load(frame);
    expect(modFrameClose).toHaveBeenCalledWith("session-1");
    expect(frameNow()).toBeNull();
    expect(screen.getByRole("status").textContent).toBe(
      "This mod reloaded itself and was stopped.",
    );
    // The port is dead: a call after the navigation reaches nothing.
    port.postMessage({ id: 1, method: "a.b" });
    await act(() => new Promise((r) => setTimeout(r, 20)));
    expect(modCall).not.toHaveBeenCalled();
    // Unmounting later doesn't close it twice.
    cleanup();
    expect(modFrameClose).toHaveBeenCalledTimes(1);
  });

  it("gives a frame that navigated before it asked no port", async () => {
    mount();
    const frame = await theFrame();
    const post = vi.spyOn(frame.contentWindow!, "postMessage").mockImplementation(() => {});
    fireEvent.load(frame);
    fireEvent.load(frame);
    postFrom(frame.contentWindow, CONNECT);
    expect(post).not.toHaveBeenCalled();
  });

  it("closes the session when it unmounts", async () => {
    const { unmount } = mount();
    const { port } = connect(await theFrame());
    unmount();
    expect(modFrameClose).toHaveBeenCalledWith("session-1");
    port.postMessage({ id: 1, method: "a.b" });
    await act(() => new Promise((r) => setTimeout(r, 20)));
    expect(modCall).not.toHaveBeenCalled();
  });

  it("closes a session that arrives after it unmounted", async () => {
    let resolve!: (v: { entryUrl: string; session: string }) => void;
    modFrameOpen.mockImplementation(() => new Promise((r) => (resolve = r)));
    const { unmount } = mount();
    unmount();
    await act(async () => resolve({ entryUrl: ENTRY, session: "late" }));
    expect(modFrameClose).toHaveBeenCalledWith("late");
  });

  it("under StrictMode, keeps exactly one session open", async () => {
    render(
      <StrictMode>
        <ModFrame modId="acme.tool" name="Acme Tool" tier="first" />
      </StrictMode>,
    );
    await theFrame();
    await waitFor(() => expect(modFrameClose).toHaveBeenCalledTimes(1));
    expect(modFrameOpen).toHaveBeenCalledTimes(2);
    expect(modFrameClose).toHaveBeenCalledWith("session-1");
    expect(document.querySelectorAll("iframe")).toHaveLength(1);
    expect(frameNow()!.getAttribute("src")).toBe(ENTRY);
  });

  it("says so when the mod can't be opened, and mounts no frame", async () => {
    modFrameOpen.mockRejectedValue("That mod isn't installed.");
    mount();
    expect((await screen.findByRole("alert")).textContent).toBe(
      "PM couldn't start this mod: That mod isn't installed.",
    );
    expect(document.querySelector("iframe")).toBeNull();
  });
});

describe("ModFrame and the frozen-page watchdog", () => {
  beforeEach(() => vi.useFakeTimers({ shouldAdvanceTime: true }));
  afterEach(() => vi.useRealTimers());

  it("starts PM's heartbeat when it mounts, keeps it going, and stops it when it unmounts", async () => {
    const { unmount } = mount();
    // At once, before its session even exists.
    expect(modsHeartbeat).toHaveBeenCalledTimes(1);
    expect(modsHeartbeat).toHaveBeenLastCalledWith(true);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(4100);
    });
    expect(modsHeartbeat.mock.calls.length).toBeGreaterThanOrEqual(3);
    unmount();
    const after = modsHeartbeat.mock.calls.length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10_000);
    });
    expect(modsHeartbeat).toHaveBeenCalledTimes(after);
  });

  it("under StrictMode beats on one timer, not two", async () => {
    render(
      <StrictMode>
        <ModFrame modId="acme.tool" name="Acme Tool" tier="first" />
      </StrictMode>,
    );
    modsHeartbeat.mockClear();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2100);
    });
    // One beat in two seconds, from one interval.
    expect(modsHeartbeat).toHaveBeenCalledTimes(1);
  });
});

describe("ModFrame events", () => {
  /** What the backend's channel hands the page for the latest session opened. */
  const channel = () =>
    modFrameOpen.mock.calls[modFrameOpen.mock.calls.length - 1][1] as (message: unknown) => void;
  const event = (seq: number, name = "documents.deleted", payload: unknown = { ids: [seq] }) => ({
    seq,
    name,
    payload,
  });

  /** Everything the frame's port receives, from now on. */
  function listen(port: MessagePort): unknown[] {
    const got: unknown[] = [];
    port.onmessage = (e: MessageEvent) => got.push(e.data);
    return got;
  }
  const settle = () => act(() => new Promise((r) => setTimeout(r, 20)));

  it("opens its session with a channel the backend's events come back on", async () => {
    mount();
    await theFrame();
    expect(modFrameOpen).toHaveBeenCalledWith("acme.tool", expect.any(Function));
  });

  it("holds events until the frame has its port, then sends them in order, then the rest as they come", async () => {
    mount();
    const frame = await theFrame();
    channel()(event(1));
    channel()(event(2, "theme.changed", { vars: {} }));
    const win = frame.contentWindow!;
    const post = vi.spyOn(win, "postMessage").mockImplementation(() => {});
    postFrom(win, CONNECT);
    const port = (post.mock.calls[0] as unknown as [unknown, string, MessagePort[]])[2][0];
    ports.push(port);
    const got = listen(port);
    await settle();
    expect(got).toEqual([
      { event: "documents.deleted", seq: 1, payload: { ids: [1] } },
      { event: "theme.changed", seq: 2, payload: { vars: {} } },
    ]);
    channel()({ ...event(3), missed: 4 });
    await settle();
    expect(got[2]).toEqual({
      event: "documents.deleted",
      seq: 3,
      payload: { ids: [3] },
      missed: 4,
    });
  });

  it("still sends the frame its goodbye when the backend ends the session", async () => {
    mount();
    const frame = await theFrame();
    const win = frame.contentWindow!;
    const post = vi.spyOn(win, "postMessage").mockImplementation(() => {});
    postFrom(win, CONNECT);
    const port = (post.mock.calls[0] as unknown as [unknown, string, MessagePort[]])[2][0];
    ports.push(port);
    const got = listen(port);
    channel()(event(1, "session.closed", { reason: "paused" }));
    await settle();
    expect(got).toEqual([{ event: "session.closed", seq: 1, payload: { reason: "paused" } }]);
  });

  it("forwards nothing it can't read: an unknown name, no sequence, not an object", async () => {
    mount();
    const { port } = connect(await theFrame());
    const got = listen(port);
    for (const bad of [
      null,
      "documents.added",
      { name: "calendar.synced", seq: 1, payload: {} },
      { name: "documents.added", payload: {} },
      { name: "documents.added", seq: 0, payload: {} },
      { name: "documents.added", seq: 1.5, payload: {} },
      { name: 7, seq: 1, payload: {} },
    ]) {
      channel()(bad);
    }
    await settle();
    expect(got).toEqual([]);
  });

  it("relays the frame's acknowledgement to the backend under PM's own session", async () => {
    mount();
    const { port } = connect(await theFrame());
    port.postMessage({ ack: 7, session: "someone-else" });
    await settle();
    expect(modEventsAck).toHaveBeenCalledTimes(1);
    expect(modEventsAck).toHaveBeenCalledWith("session-1", 7);
    // An acknowledgement that isn't a whole number is no acknowledgement.
    for (const ack of [-1, 1.5, "7", null, Number.NaN, 2 ** 60]) port.postMessage({ ack });
    await settle();
    expect(modEventsAck).toHaveBeenCalledTimes(1);
  });

  it("makes a frame that acknowledges in a loop pay for it: past the flood line it is stopped", async () => {
    mount();
    const { port } = connect(await theFrame());
    for (let i = 0; i < FLOOD_BURST + 10; i += 1) port.postMessage({ ack: i + 1 });
    await settle();
    expect(modEventsAck.mock.calls.length).toBeLessThanOrEqual(FLOOD_BURST);
    expect((await screen.findByRole("status")).textContent).toContain("too many requests");
    expect(modFrameClose).toHaveBeenCalledWith("session-1");
  });

  it("forwards events no more once the session has ended", async () => {
    const { unmount } = mount();
    const { port } = connect(await theFrame());
    const got = listen(port);
    const send = channel();
    unmount();
    send(event(1));
    await settle();
    expect(got).toEqual([]);
  });

  it("posts PM's theme for its own session when it changes, once per change", async () => {
    const root = document.documentElement;
    root.style.setProperty("--bg", "white");
    mount();
    await theFrame();
    await settle();
    // Mounting changes nothing: the frame reads the theme it starts with from `theme.get`.
    expect(modThemeChanged).not.toHaveBeenCalled();

    root.style.setProperty("--bg", "black");
    await settle();
    expect(modThemeChanged).toHaveBeenCalledTimes(1);
    const [session, snapshot] = modThemeChanged.mock.calls[0] as [string, Record<string, unknown>];
    expect(session).toBe("session-1");
    expect((snapshot.vars as Record<string, string>)["--bg"]).toBe("black");
    expect(snapshot).toMatchObject({
      mode: "dark",
      system: "slate",
      accent: "mono",
      depth: "standard",
    });

    // The same value again is no change; a different one is.
    root.style.setProperty("--bg", "black");
    await settle();
    expect(modThemeChanged).toHaveBeenCalledTimes(1);
    root.style.setProperty("--bg", "grey");
    await settle();
    expect(modThemeChanged).toHaveBeenCalledTimes(2);
    root.style.removeProperty("--bg");
  });

  it("posts no theme once its frame is gone", async () => {
    const root = document.documentElement;
    const { unmount } = mount();
    await theFrame();
    await settle();
    unmount();
    root.style.setProperty("--fg", "red");
    await settle();
    expect(modThemeChanged).not.toHaveBeenCalled();
    root.style.removeProperty("--fg");
  });
});

// FIRST USE (spec §1.5): a call the backend refuses with MOD-2002 and a permission becomes PM's own
// question, drawn outside the frame. The call is held until it is answered.
describe("the first-use question", () => {
  const NEEDS = {
    kind: "error",
    error: {
      code: "MOD-2002",
      message: "This needs your permission first (calendar.details).",
      permission: "calendar.details",
    },
  };
  const VALUE = { kind: "value", value: { events: [] } };
  const WORDS = "Read your calendar events' guests, organiser, description and links";
  const channel = () =>
    modFrameOpen.mock.calls[modFrameOpen.mock.calls.length - 1][1] as (message: unknown) => void;

  /** Everything the frame's port receives from now on. */
  function listen(port: MessagePort): unknown[] {
    const got: unknown[] = [];
    port.onmessage = (e: MessageEvent) => got.push(e.data);
    return got;
  }
  const settle = () => act(() => new Promise((r) => setTimeout(r, 20)));
  /** The dialog the moment it appears: Allow and No are still held (the pointer guard). */
  const freshQuestion = () => screen.findByRole("dialog", { name: "Let “Acme Tool” do this?" });
  /** The dialog once the guard has run out, as a person who has read it would find it. */
  const question = async () => {
    const dialog = await freshQuestion();
    act(() => void vi.advanceTimersByTime(PROMPT_GUARD_MS));
    return dialog;
  };
  const call = (port: MessagePort, id = 7) =>
    port.postMessage({ id, method: "calendar.events", args: null });

  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    modPermissionDecide.mockResolvedValue(undefined);
  });
  afterEach(() => vi.useRealTimers());

  it("holds Allow and No for a moment after the question appears, so a click meant for the mod can't answer it", async () => {
    // The mod picks when this dialog opens, so it can put "click twice" under where Allow will be.
    modCall.mockResolvedValueOnce(NEEDS).mockResolvedValueOnce(VALUE);
    mount();
    const { port } = connect(await theFrame());
    const got = listen(port);
    call(port);
    const dialog = await freshQuestion();
    const allow = within(dialog).getByRole("button", { name: "Allow" }) as HTMLButtonElement;
    const no = within(dialog).getByRole("button", { name: "No" }) as HTMLButtonElement;
    const later = within(dialog).getByRole("button", { name: "Not now" }) as HTMLButtonElement;
    expect(allow.disabled).toBe(true);
    expect(no.disabled).toBe(true);
    // The leaving answer, which records nothing, is never held.
    expect(later.disabled).toBe(false);
    fireEvent.click(allow);
    fireEvent.click(no);
    act(() => void vi.advanceTimersByTime(PROMPT_GUARD_MS - 50));
    fireEvent.click(allow);
    expect(modPermissionDecide).not.toHaveBeenCalled();
    expect(got).toEqual([]);
    // After the wait the same buttons answer.
    act(() => void vi.advanceTimersByTime(50));
    expect(allow.disabled).toBe(false);
    fireEvent.click(allow);
    await waitFor(() => expect(got).toHaveLength(1));
    expect(modPermissionDecide).toHaveBeenCalledTimes(1);
    expect(modPermissionDecide).toHaveBeenCalledWith("acme.tool", "calendar.details", true);
  });

  it("starts the wait again for the next question", async () => {
    // A second permission asked straight after the first must not arrive already armed.
    const NEEDS_B = {
      kind: "error",
      error: { code: "MOD-2002", message: "x", permission: "calendar.read" },
    };
    modCall.mockResolvedValueOnce(NEEDS).mockResolvedValueOnce(NEEDS_B).mockResolvedValue(VALUE);
    mount();
    const { port } = connect(await theFrame());
    listen(port);
    call(port, 1);
    call(port, 2);
    await waitFor(() => expect(modCall).toHaveBeenCalledTimes(2));
    fireEvent.click(within(await question()).getByRole("button", { name: "Allow" }));
    await waitFor(() => expect(modPermissionDecide).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.getByRole("dialog").textContent).toContain("time, title"));
    const allowNext = within(screen.getByRole("dialog")).getByRole("button", {
      name: "Allow",
    }) as HTMLButtonElement;
    expect(allowNext.disabled).toBe(true);
    fireEvent.click(allowNext);
    expect(modPermissionDecide).toHaveBeenCalledTimes(1);
    act(() => void vi.advanceTimersByTime(PROMPT_GUARD_MS));
    expect(allowNext.disabled).toBe(false);
  });

  it("asks in PM's own words and holds the call; Allow records the yes and retries it once", async () => {
    modCall.mockResolvedValueOnce(NEEDS).mockResolvedValueOnce(VALUE);
    mount();
    const { port } = connect(await theFrame());
    const got = listen(port);
    call(port);
    const dialog = await question();
    expect(dialog.textContent).toContain(WORDS);
    expect(dialog.textContent).toContain("Reviewed by Atlas");
    // Held: the frame has heard nothing, and the dialog is PM's, outside the frame.
    await settle();
    expect(got).toEqual([]);
    expect(modCall).toHaveBeenCalledTimes(1);
    expect(frameNow()!.contains(dialog)).toBe(false);

    fireEvent.click(within(dialog).getByRole("button", { name: "Allow" }));
    await waitFor(() => expect(got).toEqual([{ id: 7, ok: true, result: { events: [] } }]));
    expect(modPermissionDecide).toHaveBeenCalledWith("acme.tool", "calendar.details", true);
    // The same call, under the same session, once more.
    expect(modCall).toHaveBeenCalledTimes(2);
    expect(modCall.mock.calls[1]).toEqual(modCall.mock.calls[0]);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("asks the money question on a paid route with a limit field, and the limit travels with the yes", async () => {
    const NEEDS_AI = {
      kind: "error",
      error: { code: "MOD-2002", message: "x", permission: "ai" },
    };
    modAiPromptInfo.mockResolvedValue({
      route: "cloud",
      paid: true,
      unpriced: false,
      cloudModels: ["a/b"],
      localModel: null,
      suggestedCapUsd: 1,
    });
    modCall.mockResolvedValueOnce(NEEDS_AI).mockResolvedValueOnce(VALUE);
    mount();
    const { port } = connect(await theFrame());
    const got = listen(port);
    call(port);
    const dialog = await freshQuestion();
    const limit = await within(dialog).findByRole("textbox");
    expect((limit as HTMLInputElement).value).toBe("1.00");
    expect(modAiPromptInfo).toHaveBeenCalledWith("acme.tool");
    fireEvent.change(limit, { target: { value: "2.50" } });
    // Content arrived after the dialog opened: its own wait runs before the buttons answer.
    const allow = within(dialog).getByRole("button", { name: "Allow" }) as HTMLButtonElement;
    expect(allow.disabled).toBe(true);
    act(() => void vi.advanceTimersByTime(PROMPT_GUARD_MS));
    fireEvent.click(allow);
    await waitFor(() => expect(got).toHaveLength(1));
    expect(modPermissionDecide).toHaveBeenCalledWith("acme.tool", "ai", true, 2.5);
    expect(modCall).toHaveBeenCalledTimes(2);
  });

  it("No on the money question sends no limit", async () => {
    const NEEDS_AI = {
      kind: "error",
      error: { code: "MOD-2002", message: "x", permission: "ai" },
    };
    modAiPromptInfo.mockResolvedValue({
      route: "cloud",
      paid: true,
      unpriced: false,
      cloudModels: ["a/b"],
      localModel: null,
      suggestedCapUsd: 1,
    });
    modCall.mockResolvedValue(NEEDS_AI);
    mount();
    const { port } = connect(await theFrame());
    const got = listen(port);
    call(port);
    const dialog = await freshQuestion();
    await within(dialog).findByRole("textbox");
    act(() => void vi.advanceTimersByTime(PROMPT_GUARD_MS));
    fireEvent.click(within(dialog).getByRole("button", { name: "No" }));
    await waitFor(() => expect(got).toHaveLength(1));
    expect(modPermissionDecide).toHaveBeenCalledWith("acme.tool", "ai", false);
  });

  it("No records the no and tells the mod it isn't granted, without a retry", async () => {
    modCall.mockResolvedValue(NEEDS);
    mount();
    const { port } = connect(await theFrame());
    const got = listen(port);
    call(port);
    fireEvent.click(within(await question()).getByRole("button", { name: "No" }));
    await waitFor(() => expect(got).toHaveLength(1));
    expect(modPermissionDecide).toHaveBeenCalledWith("acme.tool", "calendar.details", false);
    expect(got[0]).toEqual({
      id: 7,
      ok: false,
      error: { code: MOD_ERRORS.notGranted, message: "That permission is off for this mod." },
    });
    expect(modCall).toHaveBeenCalledTimes(1);
  });

  it("Not now records nothing, gives the mod the backend's own refusal, and isn't asked again this session", async () => {
    modCall.mockResolvedValue(NEEDS);
    mount();
    const { port } = connect(await theFrame());
    const got = listen(port);
    call(port, 1);
    fireEvent.click(within(await question()).getByRole("button", { name: "Not now" }));
    await waitFor(() => expect(got).toHaveLength(1));
    expect(modPermissionDecide).not.toHaveBeenCalled();
    expect(got[0]).toMatchObject({ id: 1, ok: false, error: { code: "MOD-2002" } });
    // A mod calling in a loop gets one question, not one per call.
    call(port, 2);
    await waitFor(() => expect(got).toHaveLength(2));
    expect(got[1]).toMatchObject({ id: 2, ok: false, error: { code: "MOD-2002" } });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("Escape is Not now", async () => {
    modCall.mockResolvedValue(NEEDS);
    mount();
    const { port } = connect(await theFrame());
    const got = listen(port);
    call(port);
    await question();
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(got).toHaveLength(1));
    expect(modPermissionDecide).not.toHaveBeenCalled();
  });

  it("two calls that need the same permission share one question, and its answer releases both", async () => {
    modCall.mockResolvedValueOnce(NEEDS).mockResolvedValueOnce(NEEDS).mockResolvedValue(VALUE);
    mount();
    const { port } = connect(await theFrame());
    const got = listen(port);
    call(port, 1);
    call(port, 2);
    await waitFor(() => expect(modCall).toHaveBeenCalledTimes(2));
    expect(await screen.findAllByRole("dialog")).toHaveLength(1);
    fireEvent.click(within(await question()).getByRole("button", { name: "Allow" }));
    await waitFor(() => expect(got).toHaveLength(2));
    expect(modPermissionDecide).toHaveBeenCalledTimes(1);
    expect(new Set(got.map((m) => (m as { id: number }).id))).toEqual(new Set([1, 2]));
    expect(got.every((m) => (m as { ok: boolean }).ok)).toBe(true);
  });

  it("a save that fails keeps the question with the backend's sentence, and the call waits", async () => {
    modCall.mockResolvedValue(NEEDS);
    modPermissionDecide.mockRejectedValueOnce({
      code: "grant-refused",
      message: "A mod nobody has checked can read your data or use the internet, not both.",
    });
    mount();
    const { port } = connect(await theFrame());
    const got = listen(port);
    call(port);
    const dialog = await question();
    fireEvent.click(within(dialog).getByRole("button", { name: "Allow" }));
    expect(
      await within(dialog).findByText(
        "A mod nobody has checked can read your data or use the internet, not both.",
      ),
    ).toBeTruthy();
    expect(got).toEqual([]);
    // Still answerable: the person can leave it.
    fireEvent.click(within(dialog).getByRole("button", { name: "Not now" }));
    await waitFor(() => expect(got).toHaveLength(1));
    expect(got[0]).toMatchObject({ ok: false, error: { code: "MOD-2002" } });
  });

  it("takes the question down, and sends the mod nothing, when the session ends under it", async () => {
    modCall.mockResolvedValue(NEEDS);
    mount();
    const { port } = connect(await theFrame());
    const got = listen(port);
    call(port);
    await question();
    channel()({ seq: 1, name: "mod.paused", payload: {} });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await settle();
    expect(got.filter((m) => (m as { id?: number }).id === 7)).toEqual([]);
    expect(modPermissionDecide).not.toHaveBeenCalled();
  });

  it("answers an unusable MOD-2002 (no permission named, or an id that isn't one) as it is, asking nothing", async () => {
    modCall
      .mockResolvedValueOnce({ kind: "error", error: { code: "MOD-2002", message: "ask" } })
      .mockResolvedValueOnce({
        kind: "error",
        error: { code: "MOD-2002", message: "ask", permission: "x<img src=x>" },
      });
    mount();
    const { port } = connect(await theFrame());
    const got = listen(port);
    call(port, 1);
    call(port, 2);
    await waitFor(() => expect(got).toHaveLength(2));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(got.every((m) => (m as { ok: boolean }).ok === false)).toBe(true);
  });

  it("only the backend's reply starts a question: a refusal the frame sends for itself never does", async () => {
    mount();
    const { port } = connect(await theFrame());
    // An oversized call is refused by PM's own page, not by the backend.
    await ask(port, { id: 3, method: "calendar.events", args: "x".repeat(2_000_000) });
    expect(modCall).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("says nobody has checked a third-party mod, and still names the mod in quotes", async () => {
    modCall.mockResolvedValue(NEEDS);
    mount("third");
    const { port } = connect(await theFrame());
    call(port);
    const dialog = await question();
    expect(dialog.textContent).toContain("Third party");
    expect(dialog.textContent).toContain("Nobody has checked this mod");
  });

  it("goes away with the frame, releasing the held call quietly", async () => {
    modCall.mockResolvedValue(NEEDS);
    const view = mount();
    const { port } = connect(await theFrame());
    const got = listen(port);
    call(port);
    await question();
    view.unmount();
    await settle();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(got).toEqual([]);
  });
});

// B7: the mod's own help, shown by PM over the frame. The mod's words are drawn as text or through
// the sanitizing Markdown component, never as markup, and the frame keeps its session underneath.
describe("About this mod", () => {
  const HELP = [
    { id: "map-overview", title: "The map", body: "Hover a dot.\nClick it to read." },
    { id: "map-names", title: "Names", body: "Shows **each** file name." },
  ];

  function mountAbout(about: { description: string; help: typeof HELP }) {
    return render(<ModFrame modId="acme.tool" name="Acme Tool" tier="second" about={about} />);
  }

  const aboutButton = () => screen.queryByRole("button", { name: "About this mod" });

  it("offers no button when the mod wrote neither a description nor any help", async () => {
    mountAbout({ description: "", help: [] });
    await theFrame();
    expect(aboutButton()).toBeNull();
    cleanup();
    // Help that is only blank or invisible characters is nothing.
    mountAbout({ description: " \u200b ", help: [{ id: "x", title: "T", body: "\u200b\n " }] });
    await theFrame();
    expect(aboutButton()).toBeNull();
  });

  it("opens the maker's help over the frame, keeps the session running, and Close gives the page back", async () => {
    mountAbout({ description: "Draws your documents.", help: HELP });
    const frame = await theFrame();
    expect(frame.closest("section")?.getAttribute("data-help")).toBe("nav-mod-tab");
    const button = aboutButton()!;
    fireEvent.click(button);

    const panel = document.querySelector('[data-help="mod-about"]') as HTMLElement;
    expect(panel).not.toBeNull();
    expect(within(panel).getByRole("heading", { name: "About this mod" })).toBeTruthy();
    expect(panel.textContent).toContain("Written by the mod's maker, not by PM");
    expect(panel.textContent).toContain("Draws your documents.");
    expect(within(panel).getByRole("heading", { name: "The map" })).toBeTruthy();
    expect(within(panel).getByRole("heading", { name: "Names" })).toBeTruthy();
    // Markdown is rendered (bold), through the sanitizing component.
    expect(panel.querySelector("strong")?.textContent).toBe("each");
    // Over the frame, and the frame is still there under one session, not reopened.
    expect(frameNow()).toBe(frame);
    expect(modFrameOpen).toHaveBeenCalledTimes(1);
    expect(modFrameClose).not.toHaveBeenCalled();
    // PM's band is still drawn above the panel, and the panel's own sits over the one with the button.
    expect(screen.getAllByText("Acme Tool", { selector: "span" }).length).toBeGreaterThan(0);

    fireEvent.click(within(panel).getByRole("button", { name: "Close" }));
    expect(document.querySelector('[data-help="mod-about"]')).toBeNull();
    expect(frameNow()).toBe(frame);
    expect(document.activeElement).toBe(aboutButton());
    expect(modFrameClose).not.toHaveBeenCalled();
  });

  it("takes focus when it opens, and Escape closes it", async () => {
    mountAbout({ description: "", help: HELP });
    await theFrame();
    fireEvent.click(aboutButton()!);
    const panel = document.querySelector('[data-help="mod-about"]') as HTMLElement;
    expect(document.activeElement).toBe(within(panel).getByRole("button", { name: "Close" }));
    fireEvent.keyDown(window, { key: "Escape" });
    expect(document.querySelector('[data-help="mod-about"]')).toBeNull();
  });

  it("draws a hostile title and body as text: no markup, no link, no image, no handler reaches the page", async () => {
    mountAbout({
      description: "<img src=x onerror=alert(1)> desc",
      help: [
        {
          id: "evil",
          title: "<b>Bold</b><script>alert(1)</script>",
          body: [
            "<script>alert(2)</script><img src=x onerror=alert(3)>",
            '<a href="https://evil.test" onclick="alert(4)">raw anchor</a>',
            "[markdown link](https://evil.test/phish) and [js](javascript:alert(5))",
            "![tracker](https://track.test/pixel.png)",
            "https://bare.test/autolink",
          ].join("\n\n"),
        },
      ],
    });
    await theFrame();
    fireEvent.click(aboutButton()!);
    const panel = document.querySelector('[data-help="mod-about"]') as HTMLElement;
    expect(
      panel.querySelector("script, img, iframe, object, embed, style, form, input"),
    ).toBeNull();
    // No link at all: not even a safe-looking one a click could send to the OS browser.
    expect(panel.querySelector("a, [href], [src], [target]")).toBeNull();
    for (const el of panel.querySelectorAll("*")) {
      for (const attr of el.getAttributeNames()) expect(attr, el.outerHTML).not.toMatch(/^on/i);
    }
    // The title's markup is its literal text.
    expect(within(panel).getByRole("heading", { name: /<b>Bold<\/b>/ })).toBeTruthy();
    // The words that were links are still there to read, and an image is its alt text.
    expect(panel.textContent).toContain("markdown link");
    expect(panel.textContent).toContain("[tracker]");
    expect(panel.textContent).toContain("https://bare.test/autolink");
    // The description is a plain line, so its tag is text too.
    expect(panel.textContent).toContain("<img src=x onerror=alert(1)> desc");
  });

  it("speaks for the frame in help mode (which can't see inside it) and for the panel", async () => {
    render(
      <HelpContext.Provider value={{ enabled: true, setEnabled: () => {} }}>
        <HelpOverlay />
        <ModFrame
          modId="acme.tool"
          name="Acme Tool"
          tier="second"
          about={{ description: "", help: HELP }}
        />
      </HelpContext.Provider>,
    );
    const frame = await theFrame();
    // A hover over the frame lands on the iframe element itself, never on anything inside it.
    fireEvent.mouseOver(frame);
    expect(await screen.findByText(HELP_TEXT["nav-mod-tab"].title)).toBeTruthy();
    expect(screen.getByText(/About this mod button above the page/)).toBeTruthy();
    fireEvent.click(aboutButton()!);
    fireEvent.mouseOver(document.querySelector('[data-help="mod-about"] h3')!);
    expect(await screen.findByText(/simple text, with no links/)).toBeTruthy();
  });

  it("says so when only a description was written", async () => {
    mountAbout({ description: "Draws your documents.", help: [] });
    await theFrame();
    fireEvent.click(aboutButton()!);
    const panel = document.querySelector('[data-help="mod-about"]') as HTMLElement;
    expect(panel.textContent).toContain("hasn't written any help");
    expect(panel.textContent).toContain("Draws your documents.");
  });

  it("keeps the line breaks a maker typed and cleans invisible characters out of the body", async () => {
    mountAbout({
      description: "",
      help: [
        {
          id: "a",
          title: "T\u202e",
          body: "First line.\nSecond\u200bline.\r\n\r\n\r\n\r\nThird.",
        },
      ],
    });
    await theFrame();
    fireEvent.click(aboutButton()!);
    const panel = document.querySelector('[data-help="mod-about"]') as HTMLElement;
    const paragraphs = [...panel.querySelectorAll("p")].map((p) => p.textContent);
    expect(paragraphs).toContain("First line.\nSecond line.");
    expect(paragraphs).toContain("Third.");
    expect(panel.textContent).not.toMatch(/[\u200b\u202e]/);
  });

  it("leaves an Escape to a question PM asks over the frame, which it answers, and doesn't close with it", async () => {
    modCall.mockResolvedValue({
      kind: "error",
      error: { code: "MOD-2002", message: "ask", permission: "calendar.details" },
    });
    mountAbout({ description: "", help: HELP });
    const { port } = connect(await theFrame());
    fireEvent.click(aboutButton()!);
    port.postMessage({ id: 1, method: "calendar.events", args: null });
    await screen.findByRole("dialog");
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(document.querySelector('[data-help="mod-about"]')).not.toBeNull();
    // With the question gone, the next Escape is the panel's.
    fireEvent.keyDown(window, { key: "Escape" });
    expect(document.querySelector('[data-help="mod-about"]')).toBeNull();
  });
});
