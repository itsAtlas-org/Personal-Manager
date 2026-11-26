// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Mods tab's wrappers in `ipc.ts`, loaded for real over a mocked `@tauri-apps/api/core`: each
// calls the command it names with the arguments Rust's parameter names expect (Tauri camel-cases
// them), and the two listeners subscribe to the events the backend emits.

import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
const listenMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invokeMock(cmd, args),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (name: string, cb: (e: { payload: unknown }) => void) => listenMock(name, cb),
  emit: vi.fn(),
}));

const ipc = await import("./ipc");
const { SETTING_SAVED_EVENT } = await import("./settingsSaved");

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
  listenMock.mockReset();
  listenMock.mockResolvedValue(() => {});
});

/** Each wrapper, called with distinctive arguments, and the command and arguments it sends. */
const CALLS: [string, () => Promise<unknown>, string, Record<string, unknown> | undefined][] = [
  ["listMods", () => ipc.listMods(), "list_mods", undefined],
  ["modIcon", () => ipc.modIcon("acme.notes"), "mod_icon", { modId: "acme.notes" }],
  ["listCatalogue", () => ipc.listCatalogue(), "list_catalogue", undefined],
  [
    "installModFile",
    () => ipc.installModFile("/tmp/a.pmmod"),
    "install_mod_file",
    { path: "/tmp/a.pmmod" },
  ],
  [
    "installModConsented",
    () => ipc.installModConsented("tok", { "documents.meta": true, ai: false }),
    "install_mod_consented",
    { token: "tok", decisions: { "documents.meta": true, ai: false } },
  ],
  [
    "installModConsented (an unsigned screen's typed id)",
    () => ipc.installModConsented("tok", { internet: true }, "acme.tool"),
    "install_mod_consented",
    { token: "tok", decisions: { internet: true }, confirmId: "acme.tool" },
  ],
  [
    "installUnsignedFile",
    () => ipc.installUnsignedFile("/tmp/a.pmmod"),
    "install_unsigned_file",
    { path: "/tmp/a.pmmod" },
  ],
  ["devModeState", () => ipc.devModeState(), "dev_mode_state", undefined],
  ["devModeSwitch", () => ipc.devModeSwitch(true), "dev_mode_switch", { on: true }],
  ["devKitInfo", () => ipc.devKitInfo(), "dev_kit_info", undefined],
  [
    "installFromCatalogue",
    () => ipc.installFromCatalogue("atlas.map"),
    "install_from_catalogue",
    { modId: "atlas.map" },
  ],
  ["approveUpdate", () => ipc.approveUpdate("atlas.map"), "approve_update", { modId: "atlas.map" }],
  ["repairMod", () => ipc.repairMod("atlas.map"), "repair_mod", { modId: "atlas.map" }],
  [
    "uninstallPreview",
    () => ipc.uninstallPreview("atlas.map"),
    "uninstall_preview",
    { modId: "atlas.map" },
  ],
  [
    "uninstallMod",
    () => ipc.uninstallMod("atlas.map", true),
    "uninstall_mod",
    { modId: "atlas.map", alsoDeleteGifts: true },
  ],
  [
    "uninstallNotesPlan",
    () => ipc.uninstallNotesPlan("atlas.pinboard"),
    "uninstall_notes_plan",
    { modId: "atlas.pinboard" },
  ],
  [
    "uninstallSaveNote",
    () => ipc.uninstallSaveNote("atlas.pinboard", "n-1"),
    "uninstall_save_note",
    { modId: "atlas.pinboard", key: "n-1" },
  ],
  [
    "uninstallExportFiles",
    () => ipc.uninstallExportFiles("atlas.pinboard", true, false),
    "uninstall_export_files",
    { modId: "atlas.pinboard", notes: true, board: false },
  ],
  ["pauseMod", () => ipc.pauseMod("atlas.map"), "pause_mod", { modId: "atlas.map" }],
  ["resumeMod", () => ipc.resumeMod("atlas.map"), "resume_mod", { modId: "atlas.map" }],
  [
    "consentMod",
    () => ipc.consentMod("atlas.map", { storage: true }),
    "consent_mod",
    { modId: "atlas.map", decisions: { storage: true } },
  ],
  [
    "modPermissionDecide",
    () => ipc.modPermissionDecide("atlas.map", "internet", false),
    "mod_permission_decide",
    { modId: "atlas.map", permission: "internet", granted: false },
  ],
  [
    "modPermissionDecide (with the money prompt's limit)",
    () => ipc.modPermissionDecide("acme.tool", "ai", true, 2.5),
    "mod_permission_decide",
    { modId: "acme.tool", permission: "ai", granted: true, capUsd: 2.5 },
  ],
  [
    "modAiPromptInfo",
    () => ipc.modAiPromptInfo("acme.tool"),
    "mod_ai_prompt_info",
    { modId: "acme.tool" },
  ],
  [
    "modAiCapSet",
    () => ipc.modAiCapSet("acme.tool", 3),
    "mod_ai_cap_set",
    { modId: "acme.tool", capUsd: 3 },
  ],
  [
    "modAiCapSet (taking the limit away)",
    () => ipc.modAiCapSet("acme.tool", null),
    "mod_ai_cap_set",
    { modId: "acme.tool", capUsd: null },
  ],
  [
    "modSecretSave",
    () => ipc.modSecretSave("acme.tool", "token", "typed"),
    "mod_secret_save",
    { modId: "acme.tool", name: "token", value: "typed" },
  ],
  [
    "modSecretClear",
    () => ipc.modSecretClear("acme.tool", "token"),
    "mod_secret_clear",
    { modId: "acme.tool", name: "token" },
  ],
  [
    "modSecretStatus",
    () => ipc.modSecretStatus("acme.tool"),
    "mod_secret_status",
    { modId: "acme.tool" },
  ],
  ["modsDisclaimerState", () => ipc.modsDisclaimerState(), "mods_disclaimer_state", undefined],
  ["acceptModsDisclaimer", () => ipc.acceptModsDisclaimer(), "accept_mods_disclaimer", undefined],
  ["modsSafetyStatus", () => ipc.modsSafetyStatus(), "mods_safety_status", undefined],
  [
    "dismissModResumed",
    () => ipc.dismissModResumed("atlas.map"),
    "dismiss_mod_resumed",
    { modId: "atlas.map" },
  ],
  ["resetSafetyRecord", () => ipc.resetSafetyRecord(), "reset_safety_record", undefined],
  ["retryAutoInstall", () => ipc.retryAutoInstall(), "retry_auto_install", undefined],
  ["modsHandoverState", () => ipc.modsHandoverState(), "mods_handover_state", undefined],
  [
    "adoptMapPrefs",
    () => ipc.adoptMapPrefs("semantic", 0.25, true),
    "adopt_map_prefs",
    { layoutMode: "semantic", cohesion: 0.25, labels: true },
  ],
  [
    "adoptPinboardPrefs",
    () => ipc.adoptPinboardPrefs(false, null, true),
    "adopt_pinboard_prefs",
    { confirmDelete: false, showPast: null, showCompleted: true },
  ],
  ["pinboardMoveState", () => ipc.pinboardMoveState(), "pinboard_move_state", undefined],
  ["pinboardUseOldBoard", () => ipc.pinboardUseOldBoard(), "pinboard_use_old_board", undefined],
  [
    "pinboardMoveDismiss",
    () => ipc.pinboardMoveDismiss("conflict"),
    "pinboard_move_dismiss",
    { note: "conflict" },
  ],
  [
    "pinboardMoveDismiss (restored)",
    () => ipc.pinboardMoveDismiss("restored"),
    "pinboard_move_dismiss",
    { note: "restored" },
  ],
];

describe("the Mods tab's wrappers", () => {
  it.each(CALLS)("%s calls its command with Rust's argument names", async (_n, call, cmd, args) => {
    await call();
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith(cmd, args);
  });

  it("none of them is a settings write (the footer's Saved tick keys on `set_`)", async () => {
    let ticks = 0;
    const onTick = () => {
      ticks++;
    };
    window.addEventListener(SETTING_SAVED_EVENT, onTick);
    try {
      for (const [, call] of CALLS) await call();
    } finally {
      window.removeEventListener(SETTING_SAVED_EVENT, onTick);
    }
    expect(ticks).toBe(0);
  });

  it("passes the typed refusal through untouched, for modsErrorOf to read", async () => {
    const refusal = { code: "consent-expired", message: "Start the install again." };
    invokeMock.mockRejectedValueOnce(refusal);
    await expect(ipc.installModConsented("old", {})).rejects.toBe(refusal);
  });
});

describe("the Mods tab's listeners", () => {
  it("onModsChanged hears mods://changed and hands over its payload", async () => {
    const seen: unknown[] = [];
    await ipc.onModsChanged((e) => seen.push(e));
    expect(listenMock).toHaveBeenCalledWith("mods://changed", expect.any(Function));
    listenMock.mock.calls[0][1]({ payload: { cause: "safety" } });
    expect(seen).toEqual([{ cause: "safety" }]);
  });

  it("onDevMode hears mods://dev-mode from any window", async () => {
    const seen: unknown[] = [];
    await ipc.onDevMode((e) => seen.push(e));
    expect(listenMock).toHaveBeenCalledWith("mods://dev-mode", expect.any(Function));
    listenMock.mock.calls[0][1]({ payload: { on: false } });
    expect(seen).toEqual([{ on: false }]);
  });

  it("onModsNotice hears mods://notice", async () => {
    const seen: unknown[] = [];
    await ipc.onModsNotice((n) => seen.push(n));
    expect(listenMock).toHaveBeenCalledWith("mods://notice", expect.any(Function));
    const notice = { kind: "recordFault", unreadable: false, text: "damaged" };
    listenMock.mock.calls[0][1]({ payload: notice });
    expect(seen).toEqual([notice]);
  });
});
