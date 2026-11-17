// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Builders for the Mods tab's tests: a mod's row, a permission, a standing, a consent screen and a
// safety-list status, each with sane defaults and an `extra` to override. Test-only.

import type {
  ConsentPermission,
  ConsentScreen,
  DevKitInfo,
  KitEndpoint,
  ModInfo,
  ModPermission,
  ModStanding,
  ModsSafetyStatus,
} from "../../lib/modsTypes";

export const RUNNING: ModStanding = {
  state: "active",
  reason: null,
  kind: "running",
  text: "Running.",
  fix: "none",
  running: true,
};

export function standing(kind: ModStanding["kind"], extra: Partial<ModStanding> = {}): ModStanding {
  return { state: "paused", reason: null, kind, text: "", fix: "none", running: false, ...extra };
}

export const perm = (id: string, extra: Partial<ModPermission> = {}): ModPermission => ({
  id,
  why: `why ${id}`,
  class: "read",
  when: "install",
  decision: null,
  access: "off",
  offBecause: null,
  ...extra,
});

export function mod(extra: Partial<ModInfo> = {}): ModInfo {
  return {
    id: "acme.tool",
    name: "Acme Tool",
    description: "Does a thing.",
    tier: "second",
    author: "Acme",
    repo: "acme/tool",
    commit: "abcdef0123456789",
    version: "1.2.0",
    installedFrom: "catalogue",
    signed: true,
    consented: true,
    state: RUNNING,
    permissions: [],
    internetHosts: [],
    navTab: "Acme Tool",
    calendarOverlay: null,
    hasIcon: false,
    update: null,
    resumed: null,
    installedAt: "",
    updatedAt: "",
    ...extra,
  };
}

export const consentPerm = (
  id: string,
  extra: Partial<ConsentPermission> = {},
): ConsentPermission => ({
  ...perm(id),
  isNew: false,
  answerRequired: true,
  defaultAnswer: true,
  ...extra,
});

export function consentScreen(extra: Partial<ConsentScreen> = {}): ConsentScreen {
  return {
    token: "tok-1",
    id: "acme.tool",
    name: "Acme Tool",
    description: "Does a thing.",
    version: "1.3.0",
    author: "Acme",
    repo: "acme/tool",
    commit: "abcdef0123456789",
    tier: "second",
    kind: "update",
    installedVersion: "1.2.0",
    from: "catalogue",
    sha256: "0".repeat(64),
    permissions: [],
    internetHosts: [],
    secrets: [],
    navTab: "Acme Tool",
    calendarOverlay: null,
    widening: [],
    unsigned: null,
    ...extra,
  };
}

export function safety(extra: Partial<ModsSafetyStatus> = {}): ModsSafetyStatus {
  return {
    checked: { kind: "checked", at: "2026-11-10T00:00:00Z", text: null },
    record: null,
    index: null,
    notices: [],
    ...extra,
  };
}

export const endpoint = (extra: Partial<KitEndpoint> = {}): KitEndpoint => ({
  name: "documents.list",
  kind: "call",
  permission: "documents.meta",
  class: "read",
  added: "1.0",
  deprecated: null,
  removed: null,
  experimental: false,
  ...extra,
});

/** The Developer kit's answer, with every limit present. */
export function devKit(extra: Partial<DevKitInfo> = {}): DevKitInfo {
  return {
    api: {
      version: "1.0",
      oldestMajor: 1,
      window: "PM serves mod API 1.0 and nothing older.",
      experimental: ["ai.complete"],
    },
    endpoints: [
      endpoint(),
      endpoint({ name: "theme.get", permission: null, class: "local" }),
      endpoint({ name: "documents.added", kind: "event" }),
    ],
    limits: {
      callsPerSecond: 50,
      callBurst: 100,
      maxInFlight: 16,
      maxArgsBytes: 1 << 20,
      maxResultBytes: 8 << 20,
      maxBinaryResultBytes: 20 << 20,
      maxDocsPerCall: 5000,
      storageQuotaBytes: 50 << 20,
      storageGrowthPerSecond: 4,
      storageGrowthBurst: 40,
      eventWindow: 256,
      maxEventBytes: 64 << 10,
      maxIdsPerEvent: 2000,
      eventCoalesceMs: 250,
      maxPendingIds: 100_000,
    },
    engine: { sealed: true, unsealedReasons: [], sinkholeAttempts: 0 },
    sessions: [],
    ...extra,
  };
}

// Characters that change how nearby text is drawn, written as code points so the test sources stay
// readable (a literal one is invisible in an editor). A right-to-left override, a left-to-right
// isolate and a line break.
export const RLO = String.fromCharCode(0x202e);
export const LRI = String.fromCharCode(0x2066);
export const LINE_BREAK = "\n";
export const LINE_SEP = String.fromCharCode(0x2028);
export const ZERO_WIDTH = String.fromCharCode(0x200b);
