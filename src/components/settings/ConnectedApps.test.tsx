// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Connected apps in Settings > Data & Security: hidden where PM doesn't offer them, the switch, the
// warnings that stay on screen, the scope defaults (documents search OFF), the token shown once and
// gone after Done, the list, and the re-pairing label with its revoke confirm.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { McpClient, McpState } from "../../lib/modsTypes";

const mcpOffered = vi.fn<() => Promise<boolean>>();
const mcpState = vi.fn<() => Promise<McpState>>();
const mcpSetOn = vi.fn<(on: boolean) => Promise<McpState>>();
const mcpConnectApp = vi.fn();
const mcpRevoke = vi.fn<(id: number) => Promise<McpState>>();
vi.mock("../../lib/ipc", () => ({
  mcpOffered: () => mcpOffered(),
  mcpState: () => mcpState(),
  mcpSetOn: (on: boolean) => mcpSetOn(on),
  mcpConnectApp: (name: string, scopes: string[]) => mcpConnectApp(name, scopes),
  mcpRevoke: (id: number) => mcpRevoke(id),
}));

vi.mock("../../kit/theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { HELP } from "../../lib/help";
import { ConnectedApps } from "./ConnectedApps";
import {
  DEFAULT_SCOPES,
  NAME_MAX,
  NOTE_UNTRUSTED,
  SCOPE_CHOICES,
  WARN_CONFIG,
  WARN_PROVIDER,
} from "./connectedAppsWords";
import { MCP_SECTION_ID, SETTINGS_TABS, shownSections } from "./registry";

const TOKEN = `pmcp_3_${"ab".repeat(32)}`;
const SNIPPET = JSON.stringify(
  {
    mcpServers: {
      "personal-manager": {
        command: "/home/me/Apps/PM.AppImage",
        args: ["--mcp"],
        env: { PM_MCP_TOKEN: TOKEN },
      },
    },
  },
  null,
  2,
);

function client(over: Partial<McpClient> = {}): McpClient {
  return {
    id: 1,
    name: "Desk",
    scopes: ["tasks.read"],
    createdAt: "2020-01-02T10:00:00.000Z",
    lastUsedAt: null,
    paired: true,
    ...over,
  };
}

function state(over: Partial<McpState> = {}): McpState {
  return { offered: true, on: false, clients: [], listening: false, error: null, ...over };
}

beforeEach(() => {
  mcpOffered.mockReset().mockResolvedValue(true);
  mcpState.mockReset().mockResolvedValue(state());
  mcpSetOn.mockReset();
  mcpConnectApp.mockReset();
  mcpRevoke.mockReset();
});
afterEach(cleanup);

/** Render with the server on, ready to connect an app. */
async function renderOn(over: Partial<McpState> = {}) {
  mcpState.mockResolvedValue(state({ on: true, listening: true, ...over }));
  render(<ConnectedApps />);
  await screen.findByRole("form", { name: "Connect an app" });
}

describe("whether it is shown", () => {
  it("is hidden when PM does not offer it, and reads nothing", async () => {
    mcpOffered.mockResolvedValue(false);
    const { container } = render(<ConnectedApps />);
    await waitFor(() => expect(mcpOffered).toHaveBeenCalled());
    expect(container.innerHTML).toBe("");
    expect(mcpState).not.toHaveBeenCalled();
  });

  it("is hidden until Rust has answered", () => {
    mcpOffered.mockReturnValue(new Promise(() => {}));
    const { container } = render(<ConnectedApps />);
    expect(container.innerHTML).toBe("");
  });

  it("is shown when offered, as the section the rail links to", async () => {
    const { container } = render(<ConnectedApps />);
    await screen.findByText("Connected apps");
    const section = container.querySelector(`#${MCP_SECTION_ID}[data-settings-section]`);
    expect(section).not.toBeNull();
    expect(HELP[section!.getAttribute("data-help")!]).toBeDefined();
  });

  it("leaves the rail's link out exactly when it is not offered", () => {
    const data = SETTINGS_TABS.find((t) => t.id === "data")!;
    expect(data.sections.map((s) => s.id)).toContain(MCP_SECTION_ID);
    expect(shownSections(data.sections, true).map((s) => s.id)).toContain(MCP_SECTION_ID);
    expect(shownSections(data.sections, false).map((s) => s.id)).not.toContain(MCP_SECTION_ID);
    expect(shownSections(data.sections, false)).toHaveLength(data.sections.length - 1);
  });
});

describe("the switch", () => {
  it("asks Rust for the opposite of what it shows, and shows what Rust answers", async () => {
    mcpSetOn.mockResolvedValue(state({ on: true, listening: true }));
    render(<ConnectedApps />);
    const toggle = await screen.findByRole("switch");
    await waitFor(() => expect(toggle.hasAttribute("disabled")).toBe(false));
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(toggle);
    expect(mcpSetOn).toHaveBeenCalledWith(true);
    await screen.findByRole("form", { name: "Connect an app" });
    expect(screen.getByRole("switch").getAttribute("aria-checked")).toBe("true");
  });

  it("says why when it did not change, and stays where it was", async () => {
    mcpSetOn.mockRejectedValue({ code: "not-allowed", message: "Not here yet." });
    render(<ConnectedApps />);
    const toggle = await screen.findByRole("switch");
    await waitFor(() => expect(toggle.hasAttribute("disabled")).toBe(false));
    fireEvent.click(toggle);
    await screen.findByText("Not here yet.");
    expect(screen.getByRole("switch").getAttribute("aria-checked")).toBe("false");
  });

  it("offers no way to connect an app while it is off", async () => {
    render(<ConnectedApps />);
    await screen.findByRole("switch");
    expect(screen.queryByRole("form", { name: "Connect an app" })).toBeNull();
  });

  it("says plainly when it is on but the connection point is not open", async () => {
    mcpState.mockResolvedValue(
      state({ on: true, listening: false, error: "PM can't open its connection point." }),
    );
    render(<ConnectedApps />);
    await screen.findByText("PM can't open its connection point.");
  });
});

describe("the warnings", () => {
  it("are on screen, off or on, and never inside a collapsed explanation", async () => {
    for (const on of [false, true]) {
      cleanup();
      mcpState.mockResolvedValue(state({ on, listening: on }));
      render(<ConnectedApps />);
      await screen.findByRole("switch");
      for (const text of [WARN_PROVIDER, WARN_CONFIG, NOTE_UNTRUSTED]) {
        const line = screen.getByText(text);
        // The explanation folds behind a caret and is `inert` while closed; a warning is never there.
        expect(line.closest("[inert]")).toBeNull();
        expect(line.closest("[aria-expanded]")).toBeNull();
      }
    }
  });

  it("say what the design promised, word for word", () => {
    expect(WARN_PROVIDER).toBe(
      "The AI provider of any app you connect is outside PM's zero-data-retention promise: what PM returns to it leaves PM.",
    );
    expect(WARN_CONFIG).toBe(
      "Any program running as you can read a connected app's saved configuration and use its token.",
    );
  });

  it("tell the person apps can read and not change", async () => {
    render(<ConnectedApps />);
    await screen.findByText(/Apps can read, not change/);
  });
});

describe("connecting an app", () => {
  it("starts with documents search unticked and the three others ticked", async () => {
    await renderOn();
    expect(DEFAULT_SCOPES).toEqual(["tasks.read", "projects.read", "calendar.read"]);
    const boxes = Object.fromEntries(
      SCOPE_CHOICES.map((s) => [
        s.id,
        (screen.getByLabelText(s.label, { exact: false }) as HTMLInputElement).checked,
      ]),
    );
    expect(boxes).toEqual({
      "tasks.read": true,
      "projects.read": true,
      "calendar.read": true,
      "documents.text": false,
    });
    // Its own plain line.
    expect(screen.getByText("Sends text from your documents.")).toBeTruthy();
    // The tasks line does not understate: a task's "why" note goes too, and names only with a tick.
    const tasks = SCOPE_CHOICES.find((s) => s.id === "tasks.read")!.detail;
    expect(tasks).toMatch(/notes/);
    expect(tasks).toMatch(/only if you tick/);
    // And nothing offers to change anything.
    expect(screen.queryByText(/write/i)).toBeNull();
  });

  it("sends the trimmed name and exactly the ticked scopes", async () => {
    await renderOn();
    mcpConnectApp.mockResolvedValue({ clientId: 3, token: TOKEN, snippet: SNIPPET });
    fireEvent.change(screen.getByLabelText("App name"), { target: { value: "  Desk  " } });
    fireEvent.click(screen.getByLabelText(/Documents search/));
    fireEvent.click(screen.getByLabelText(/^Tasks/));
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await screen.findByLabelText("Configuration to paste into the app");
    expect(mcpConnectApp).toHaveBeenCalledWith("Desk", [
      "projects.read",
      "calendar.read",
      "documents.text",
    ]);
  });

  it("will not connect a blank name, an unticked set, and caps the name's length", async () => {
    await renderOn();
    const connect = screen.getByRole("button", { name: "Connect" });
    expect(connect.hasAttribute("disabled")).toBe(true);
    const name = screen.getByLabelText("App name") as HTMLInputElement;
    expect(name.maxLength).toBe(NAME_MAX);
    fireEvent.change(name, { target: { value: "   " } });
    expect(connect.hasAttribute("disabled")).toBe(true);
    fireEvent.change(name, { target: { value: "Desk" } });
    expect(connect.hasAttribute("disabled")).toBe(false);
    for (const s of DEFAULT_SCOPES) {
      fireEvent.click(
        screen.getByLabelText(SCOPE_CHOICES.find((c) => c.id === s)!.label, { exact: false }),
      );
    }
    expect(connect.hasAttribute("disabled")).toBe(true);
    screen.getByText("Choose at least one thing the app may read.");
    expect(mcpConnectApp).not.toHaveBeenCalled();
  });

  it("shows the token once, copies the config, and drops it when dismissed", async () => {
    await renderOn();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    mcpConnectApp.mockResolvedValue({ clientId: 3, token: TOKEN, snippet: SNIPPET });
    // The refreshed list that follows holds the app and, as Rust's state always is, no token.
    mcpState.mockResolvedValue(state({ on: true, listening: true, clients: [client({ id: 3 })] }));
    fireEvent.change(screen.getByLabelText("App name"), { target: { value: "Desk" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));

    const shown = await screen.findByLabelText("Configuration to paste into the app");
    expect(shown.textContent).toContain(TOKEN);
    // The command is the app's own file, never an AppImage's temporary mount.
    expect(shown.textContent).toContain("/home/me/Apps/PM.AppImage");
    expect(shown.textContent).not.toContain("/tmp/.mount_");
    // The form is replaced, not shown beside it: one thing to do at a time.
    expect(screen.queryByRole("form", { name: "Connect an app" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    await screen.findByText("Copied.");
    expect(writeText).toHaveBeenCalledWith(SNIPPET);

    fireEvent.click(screen.getByRole("button", { name: /Done/ }));
    expect(document.body.textContent).not.toContain(TOKEN);
    expect(document.body.textContent).not.toContain("PM_MCP_TOKEN");
    // The list that remains shows the app and nothing of its token.
    const list = screen.getByRole("list", { name: "Connected apps" });
    expect(within(list).getByText("Desk")).toBeTruthy();
    expect(list.textContent).not.toContain("pmcp_");
    // The form starts again from its defaults.
    expect((screen.getByLabelText("App name") as HTMLInputElement).value).toBe("");
    expect((screen.getByLabelText(/Documents search/) as HTMLInputElement).checked).toBe(false);
  });

  it("loses the token when the switch goes off, and never draws it again when it goes on", async () => {
    await renderOn();
    mcpConnectApp.mockResolvedValue({ clientId: 3, token: TOKEN, snippet: SNIPPET });
    fireEvent.change(screen.getByLabelText("App name"), { target: { value: "Desk" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await screen.findByLabelText("Configuration to paste into the app");

    mcpSetOn.mockResolvedValueOnce(state({ on: false }));
    fireEvent.click(screen.getByRole("switch"));
    await waitFor(() =>
      expect(screen.getByRole("switch").getAttribute("aria-checked")).toBe("false"),
    );
    expect(document.body.textContent).not.toContain(TOKEN);

    mcpSetOn.mockResolvedValueOnce(state({ on: true, listening: true }));
    fireEvent.click(screen.getByRole("switch"));
    // The connect form is back, not an old config block.
    await screen.findByRole("form", { name: "Connect an app" });
    expect(screen.queryByLabelText("Configuration to paste into the app")).toBeNull();
    expect(document.body.textContent).not.toContain(TOKEN);
    expect(document.body.textContent).not.toContain("PM_MCP_TOKEN");
  });

  it("says when a copy failed, without printing the token anywhere else", async () => {
    await renderOn();
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: vi.fn().mockRejectedValue(new Error("denied pmcp_secret")) },
      configurable: true,
    });
    mcpConnectApp.mockResolvedValue({ clientId: 3, token: TOKEN, snippet: SNIPPET });
    fireEvent.change(screen.getByLabelText("App name"), { target: { value: "Desk" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await screen.findByLabelText("Configuration to paste into the app");
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).not.toContain("pmcp_");
    expect(alert.textContent).toContain("copy it yourself");
  });

  it("shows PM's own sentence when connecting is refused, and no token", async () => {
    await renderOn();
    mcpConnectApp.mockRejectedValue({
      code: "bad-request",
      message: "Too many apps are connected.",
    });
    fireEvent.change(screen.getByLabelText("App name"), { target: { value: "Desk" } });
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    await screen.findByText("Too many apps are connected.");
    expect(screen.queryByLabelText("Configuration to paste into the app")).toBeNull();
  });
});

describe("the list", () => {
  it("shows name, what it may read, when it was added and last used", async () => {
    mcpState.mockResolvedValue(
      state({
        clients: [
          client({
            scopes: ["tasks.read", "calendar.read"],
            // Stored as UTC instants; built from a local noon so the local day is the same in any zone.
            createdAt: new Date(2020, 0, 2, 12).toISOString(),
            lastUsedAt: new Date(2020, 2, 4, 12).toISOString(),
          }),
        ],
      }),
    );
    render(<ConnectedApps />);
    const row = (await screen.findByRole("list", { name: "Connected apps" })).querySelector("li")!;
    expect(row.textContent).toContain("Desk");
    expect(row.textContent).toContain("Tasks, Calendar events (no attendees)");
    expect(row.textContent).toContain("Added 02-01-2020");
    expect(row.textContent).toContain("Last used 04-03-2020");
    expect(row.textContent).not.toContain("re-pairing");
  });

  it("shows the LOCAL day of a stored UTC instant, not its UTC day", async () => {
    // 23:30 local on 4 March 2020 is already the 5th in UTC in any zone west of Greenwich, and the
    // person used the app on the 4th.
    const lateEvening = new Date(2020, 2, 4, 23, 30).toISOString();
    mcpState.mockResolvedValue(
      state({ clients: [client({ createdAt: lateEvening, lastUsedAt: lateEvening })] }),
    );
    render(<ConnectedApps />);
    const row = (await screen.findByRole("list", { name: "Connected apps" })).querySelector("li")!;
    expect(row.textContent).toContain("Added 04-03-2020");
    expect(row.textContent).toContain("Last used 04-03-2020");
  });

  it("revokes a paired app at once and shows what Rust answers", async () => {
    mcpState.mockResolvedValue(state({ clients: [client()] }));
    mcpRevoke.mockResolvedValue(state());
    render(<ConnectedApps />);
    fireEvent.click(await screen.findByRole("button", { name: "Revoke Desk" }));
    expect(mcpRevoke).toHaveBeenCalledWith(1);
    await screen.findByText("No apps are connected.");
  });

  it("calls an app without a pairing here 'needs re-pairing' and asks before revoking it", async () => {
    mcpState.mockResolvedValue(
      state({ clients: [client({ id: 9, name: "Elsewhere", paired: false })] }),
    );
    mcpRevoke.mockResolvedValue(state());
    render(<ConnectedApps />);
    await screen.findByText(/Needs re-pairing on this device/);
    fireEvent.click(screen.getByRole("button", { name: "Revoke Elsewhere" }));
    // Nothing happened yet: the confirm says what it may do on another computer.
    expect(mcpRevoke).not.toHaveBeenCalled();
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("may also disconnect it on another computer");
    // Cancelling revokes nothing.
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(mcpRevoke).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Revoke Elsewhere" }));
    fireEvent.click(
      await within(await screen.findByRole("dialog")).findByRole("button", { name: "Revoke" }),
    );
    await waitFor(() => expect(mcpRevoke).toHaveBeenCalledWith(9));
  });

  it("draws an app's name as plain text", async () => {
    mcpState.mockResolvedValue(
      state({ clients: [client({ name: "<img src=x onerror=alert(1)>" })] }),
    );
    const { container } = render(<ConnectedApps />);
    await screen.findByRole("list", { name: "Connected apps" });
    expect(container.querySelector("img")).toBeNull();
  });
});
