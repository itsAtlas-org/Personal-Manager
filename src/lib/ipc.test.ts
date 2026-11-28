// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The IPC boundary's own behaviour, loaded for real.
//
// `src/lib/ipc.ts` is the single CI-enforced seam between the webview and every Tauri command, and
// it had no test. It is not a pass-through: it normalises the backend's one structured error shape
// into a `VaultError` that five vault-recovery surfaces branch on, and it fires the Settings
// footer's "Saved ✓" on success only. Every component test replaces the module wholesale with a
// bare factory mock (`vi.mock("../lib/ipc", () => …)` with no `importOriginal`), so until this file
// the real module was never loaded by anything.
//
// So the mock here is one level DOWN — `@tauri-apps/api/core` — and the module under test is the
// genuine article.

import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invokeMock(cmd, args),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const ipc = await import("./ipc");
const { SETTING_SAVED_EVENT } = await import("./settingsSaved");

/** Count "Saved ✓" announcements while `fn` runs. */
async function savedTicks(fn: () => Promise<unknown>): Promise<number> {
  let ticks = 0;
  const onTick = () => {
    ticks++;
  };
  window.addEventListener(SETTING_SAVED_EVENT, onTick);
  try {
    await fn().catch(() => {});
  } finally {
    window.removeEventListener(SETTING_SAVED_EVENT, onTick);
  }
  return ticks;
}

beforeEach(() => {
  invokeMock.mockReset();
});

describe("the structured-fault normalisation", () => {
  const fault = {
    code: "locked",
    op: "open_vault",
    path: "/home/u/vault",
    message: "That vault is locked by another copy of PM.",
  };

  it("turns the backend's one structured shape into a VaultError callers can branch on", () => {
    // Rust's `Error::Vault` serialises `{code, op, path, message}`; every other variant is a bare
    // string. Without this, the five recovery surfaces would be string-matching a message.
    invokeMock.mockRejectedValueOnce(fault);
    return ipc.hasOpenRouterKey().then(
      () => expect.unreachable("should have rejected"),
      (e: unknown) => {
        expect(e).toBeInstanceOf(ipc.VaultError);
        expect(ipc.vaultFaultOf(e)).toEqual(fault);
        // The ~200 existing `String(e)` call sites must keep rendering a sentence, not
        // "[object Object]" and not "Error: …".
        expect(String(e)).toBe(fault.message);
        expect((e as Error).message).toBe(fault.message);
      },
    );
  });

  it("passes a bare-string rejection through untouched", () => {
    invokeMock.mockRejectedValueOnce("no key configured");
    return ipc.hasOpenRouterKey().then(
      () => expect.unreachable("should have rejected"),
      (e: unknown) => {
        expect(e).toBe("no key configured");
        expect(ipc.vaultFaultOf(e)).toBeNull();
      },
    );
  });

  it("leaves an object that is merely fault-ish alone", () => {
    // Missing `op`. Widening the predicate would wrap ordinary rejections in a VaultError and send
    // callers down a vault-recovery path for an error that has nothing to do with the vault.
    const notAFault = { code: "locked", message: "hmm" };
    invokeMock.mockRejectedValueOnce(notAFault);
    return ipc.hasOpenRouterKey().then(
      () => expect.unreachable("should have rejected"),
      (e: unknown) => {
        expect(e).toBe(notAFault);
        expect(e).not.toBeInstanceOf(ipc.VaultError);
      },
    );
  });

  it("returns a resolved value untouched", async () => {
    invokeMock.mockResolvedValueOnce(true);
    await expect(ipc.hasOpenRouterKey()).resolves.toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("has_openrouter_key", undefined);
  });
});

describe("the Saved tick", () => {
  it("announces after a settings write that succeeded", async () => {
    invokeMock.mockResolvedValueOnce(undefined);
    expect(await savedTicks(() => ipc.setReranking(true))).toBe(1);
  });

  it("stays silent when the settings write rejected", async () => {
    // A tick on a failed save is the worst outcome: the user is told their change landed when it
    // did not.
    invokeMock.mockRejectedValueOnce("disk full");
    expect(await savedTicks(() => ipc.setReranking(true))).toBe(0);
  });

  it("stays silent for a command that is not a settings write", async () => {
    invokeMock.mockResolvedValueOnce(true);
    expect(await savedTicks(() => ipc.hasOpenRouterKey())).toBe(0);
  });

  it("announces from the shared invoke, so a new setting command needs no registration", async () => {
    // The point of announcing inside `invoke` rather than per wrapper. Any `set_*` command that is
    // not a content write ticks, including one nobody has written a wrapper for yet.
    invokeMock.mockResolvedValueOnce(undefined);
    expect(await savedTicks(() => ipc.setOnboardingDone())).toBe(1);
  });
});

describe("argument marshalling", () => {
  it("passes each wrapper's arguments under the key the Rust command expects", async () => {
    // Tauri maps `camelCase` here to `snake_case` there, so the KEY is part of the contract and a
    // typo is a runtime "invalid args" nobody sees until that screen is opened.
    invokeMock.mockResolvedValue(undefined);
    await ipc.setOpenRouterKey("sk-test");
    expect(invokeMock).toHaveBeenCalledWith("set_openrouter_key", { key: "sk-test" });

    await ipc.setIndexingSpeed("gentle");
    expect(invokeMock).toHaveBeenCalledWith("set_indexing_speed", { speed: "gentle" });
  });

  it("passes the provider-key commands' arguments under the names the Rust commands take", async () => {
    invokeMock.mockResolvedValue(undefined);
    await ipc.directProvidersState();
    expect(invokeMock).toHaveBeenCalledWith("direct_providers_state", undefined);
    await ipc.setDirectOptIn("openai", true);
    expect(invokeMock).toHaveBeenCalledWith("set_direct_opt_in", {
      provider: "openai",
      enabled: true,
    });
    await ipc.saveDirectKey("openai", "typed-key");
    expect(invokeMock).toHaveBeenCalledWith("save_direct_key", {
      provider: "openai",
      key: "typed-key",
    });
    await ipc.recheckDirectKey("openai");
    expect(invokeMock).toHaveBeenCalledWith("recheck_direct_key", { provider: "openai" });
    await ipc.clearDirectKey("openai");
    expect(invokeMock).toHaveBeenCalledWith("clear_direct_key", { provider: "openai" });
    await ipc.setRoleCloudProvider("background", "openrouter");
    expect(invokeMock).toHaveBeenCalledWith("set_role_cloud_provider", {
      role: "background",
      provider: "openrouter",
    });
    await ipc.setDirectDataSharing("openai", true);
    expect(invokeMock).toHaveBeenCalledWith("set_direct_data_sharing", {
      provider: "openai",
      shares: true,
    });
  });

  it("wraps the list commands with the names and arguments Rust reads", async () => {
    invokeMock.mockResolvedValue({ route: { entries: [], derived: false }, materialised: false });
    await ipc.routeStatus();
    expect(invokeMock).toHaveBeenCalledWith("route_status", undefined);
    await ipc.routeAdd("chat", "anthropic", "anthropic/claude-sonnet-5-5", "high", 1);
    expect(invokeMock).toHaveBeenCalledWith("route_add", {
      role: "chat",
      provider: "anthropic",
      model: "anthropic/claude-sonnet-5-5",
      effort: "high",
      at: 1,
    });
    // This computer: no model, no place (the end).
    await ipc.routeAdd("background", "local", "");
    expect(invokeMock).toHaveBeenCalledWith("route_add", {
      role: "background",
      provider: "local",
      model: "",
      effort: null,
      at: null,
    });
    // Every edit that names a position names the row too (the backend refuses a stale position).
    await ipc.routeMove("chat", 2, 0, "openrouter:a/x");
    expect(invokeMock).toHaveBeenCalledWith("route_move", {
      role: "chat",
      from: 2,
      to: 0,
      key: "openrouter:a/x",
    });
    await ipc.routeRemove("chat", 1, "local");
    expect(invokeMock).toHaveBeenCalledWith("route_remove", {
      role: "chat",
      index: 1,
      key: "local",
    });
    await ipc.routeSetEffort("chat", 0, "anthropic:m", null);
    expect(invokeMock).toHaveBeenCalledWith("route_set_effort", {
      role: "chat",
      index: 0,
      key: "anthropic:m",
      effort: null,
    });
    await ipc.routeReset("background");
    expect(invokeMock).toHaveBeenCalledWith("route_reset", { role: "background" });
    await ipc.promoteEntry("chat", "gemini", "gemini/gemini-3.5-flash");
    expect(invokeMock).toHaveBeenCalledWith("promote_entry", {
      role: "chat",
      provider: "gemini",
      model: "gemini/gemini-3.5-flash",
    });
    await ipc.routeTryAgain("anthropic");
    expect(invokeMock).toHaveBeenCalledWith("route_try_again", { provider: "anthropic" });
  });

  /** A refused list edit must never show the Settings "Saved" tick: none of these is a `set_*`. */
  it("never ticks 'Saved' for a list edit, which can be refused", async () => {
    invokeMock.mockResolvedValue({ route: { entries: [], derived: false } });
    expect(await savedTicks(() => ipc.routeAdd("chat", "openrouter", "a/b"))).toBe(0);
    expect(await savedTicks(() => ipc.routeMove("chat", 1, 0, "openrouter:a/x"))).toBe(0);
    expect(await savedTicks(() => ipc.promoteEntry("chat", "openrouter", "a/b"))).toBe(0);
  });

  it("ticks 'Saved' for a provider setting and not for a key check, which can be refused", async () => {
    invokeMock.mockResolvedValue(undefined);
    expect(await savedTicks(() => ipc.setRoleCloudProvider("chat", "openrouter"))).toBe(1);
    expect(await savedTicks(() => ipc.saveDirectKey("openai", "typed-key"))).toBe(0);
    expect(await savedTicks(() => ipc.clearDirectKey("openai"))).toBe(0);
  });

  it("carries the conversation into a retrieval explain, so the panel sees the answer's pool", async () => {
    // A live turn skips this chat's own in-window turns; without the id the panel explained a
    // strictly WIDER corpus than the answer came from, and the diagnostician reasoned from those
    // rows. `undefined` is the free-text Developer-mode box, where there is no turn to mirror.
    invokeMock.mockResolvedValue({ rows: [] });
    await ipc.retrievalExplain("where are my taxes?", "Taxes", 8, 42);
    expect(invokeMock).toHaveBeenCalledWith("retrieval_explain", {
      query: "where are my taxes?",
      project: "Taxes",
      k: 8,
      conversationId: 42,
    });

    await ipc.retrievalExplain("free text");
    expect(invokeMock).toHaveBeenCalledWith("retrieval_explain", {
      query: "free text",
      project: undefined,
      k: undefined,
      conversationId: undefined,
    });
  });
});
