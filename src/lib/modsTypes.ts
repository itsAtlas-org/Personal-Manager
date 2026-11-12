// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Mods tab's wire types (Mods spec §1.13): what the backend's `mods::tab` commands answer, field
// for field (camelCase, as serde writes them). The page draws these and decides nothing: every
// sentence, standing, access and refusal below is worded in Rust, and `ipc.ts` carries the wrappers.
//
// Two of these are pinned to Rust by tests: `ModsErrorCode` to `mods::tab::error::Code`, and the
// `ModsChangedCause` union to `mods::refresh`'s `CHANGED_*` constants.

/** Why a Mods command refused: the stable code to branch on (what button to draw, whether to retry)
 *  and the sentence to show as it is. */
export type ModsErrorCode =
  | "not-main-window"
  | "no-store"
  | "locked"
  | "not-installed"
  | "not-in-catalogue"
  | "bad-request"
  | "consent-expired"
  | "file-too-large"
  | "file-unreadable"
  | "not-a-mod"
  | "unsigned"
  | "safety-list"
  | "needs-newer-pm"
  | "api-retired"
  | "refused"
  | "not-from-catalogue"
  | "changed"
  | "grant-refused"
  | "not-allowed"
  | "offline"
  | "download-refused"
  | "internal";

export interface ModsError {
  code: ModsErrorCode;
  message: string;
}

/** The typed refusal behind a caught rejection of a Mods command, or null for any other rejection.
 *  The backend rejects with `{code, message}` (no `op`, so `ipc.ts` passes it through untouched). */
export function modsErrorOf(e: unknown): ModsError | null {
  if (typeof e !== "object" || e === null) return null;
  const f = e as Record<string, unknown>;
  if (typeof f.code !== "string" || typeof f.message !== "string" || "op" in f) return null;
  return { code: f.code as ModsErrorCode, message: f.message };
}

// ---- The installed list ---------------------------------------------------------------------------

/** How a row stands, in kinds to branch on. */
export type ModStandingKind =
  | "running"
  | "waiting-for-you"
  | "paused-by-you"
  | "paused-by-safety-list"
  | "removed-by-safety-list"
  | "paused-dev-mode"
  | "files-missing"
  | "files-changed"
  | "record-unreadable"
  | "api-unsupported"
  | "parked"
  | "installing"
  | "paused-other";

/** The one thing offered for a row: `turn-on` opens the first-open screen, `repair` is
 *  `repairMod`, `install-file-again` picks the file again, `uninstall` opens the uninstall dialog. */
export type ModFix =
  "none" | "turn-on" | "resume" | "repair" | "install-file-again" | "uninstall" | "update-pm";

export interface ModStanding {
  /** `mods.state`: active, paused, parked or installing. */
  state: string;
  /** `mods.state_reason`, as stored. */
  reason: string | null;
  kind: ModStandingKind;
  /** The sentence to show. */
  text: string;
  fix: ModFix;
  /** Active and turned on. */
  running: boolean;
}

export type PermissionClass = "local" | "read" | "write" | "money" | "egress";
export type PermissionWhen = "auto" | "install" | "install-then-first-use" | "first-use" | "ask";

export interface ModPermission {
  id: string;
  /** The maker's reason, shown at install (author-written plain text). */
  why: string;
  class: PermissionClass;
  when: PermissionWhen;
  /** The stored decision, if any. */
  decision: "granted" | "denied" | null;
  /** What the dispatcher answers now: `asks` is a prompt at first use. */
  access: "allowed" | "asks" | "off";
  /** Why it is off, as a sentence. */
  offBecause: string | null;
}

export interface ModUpdateWaiting {
  version: string;
  kind: "approval" | "newer-pm";
  /** What it asks for beyond what was granted (`permission:ai`, `host:example.com`). */
  widenings: string[];
  needsApi: string | null;
  text: string;
}

export interface ModResumed {
  version: string;
  at: string;
  text: string;
}

export interface ModInfo {
  id: string;
  name: string;
  description: string;
  /** What the row says; an unsigned row is third party whatever it claims (`signed`). */
  tier: "first" | "second" | "third";
  author: string;
  /** `owner/repo`. */
  repo: string;
  commit: string;
  version: string;
  installedFrom: "catalogue" | "file" | "folder";
  signed: boolean;
  consented: boolean;
  state: ModStanding;
  permissions: ModPermission[];
  internetHosts: string[];
  /** The sidebar tab's label, when the mod has one. */
  navTab: string | null;
  calendarOverlay: string | null;
  /** `modIcon` answers it. */
  hasIcon: boolean;
  update: ModUpdateWaiting | null;
  resumed: ModResumed | null;
  installedAt: string;
  updatedAt: string;
}

/** A mod's icon: a media type (`image/png` or `image/webp`) and its bytes in base64. */
export interface ModIcon {
  mediaType: "image/png" | "image/webp";
  data: string;
}

/** Turn a {@link ModIcon} into a `Blob` for `URL.createObjectURL`. The main window's CSP has no
 *  `pmmod:` in `img-src`, and a `blob:` URL is how the tab shows the icon without widening it. */
export function iconBlob(icon: ModIcon): Blob {
  const binary = atob(icon.data);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return new Blob([bytes], { type: icon.mediaType });
}

// ---- Installing -----------------------------------------------------------------------------------

export interface ConsentPermission extends ModPermission {
  /** An update asks for this and the installed version didn't. */
  isNew: boolean;
  /** The screen must send an answer for it (it is asked at install). */
  answerRequired: boolean;
  /** What the screen should start on: an earlier decision, else yes. */
  defaultAnswer: boolean;
}

/** What the consent screen shows before anything is installed (`installModFile`,
 *  `installFromCatalogue`, `approveUpdate`). Hand `token` and the user's answers to
 *  `installModConsented`. */
export interface ConsentScreen {
  token: string;
  id: string;
  name: string;
  description: string;
  version: string;
  author: string;
  repo: string;
  /** The commit the maker's signed build came from (second party shows "reviewed at commit …"). */
  commit: string;
  tier: "first" | "second";
  kind: "fresh" | "update" | "repair";
  installedVersion: string | null;
  from: "file" | "catalogue";
  sha256: string;
  permissions: ConsentPermission[];
  internetHosts: string[];
  secrets: { name: string; host: string }[];
  navTab: string | null;
  calendarOverlay: string | null;
  /** What an update asks for that the installed version didn't. */
  widening: string[];
}

export interface InstallResult {
  id: string;
  version: string;
  state: string;
  kind: "fresh" | "update" | "repair";
  pruned: string[];
  /** The user's answers were written as grants. When false, `grantsProblem` says why and the mod is
   *  installed with every install-time permission off until it is granted in the tab. */
  grantsSaved: boolean;
  grantsProblem: ModsError | null;
}

export interface Repaired {
  id: string;
  version: string;
  state: string;
}

/** Where a row stands after a resume or a first-open "Turn on". */
export interface ModStandingAfter {
  state: string;
  reason: string | null;
  /** The registry now serves it. */
  serving: boolean;
}

// ---- The catalogue ----------------------------------------------------------------------------------

export interface CatalogueEntry {
  id: string;
  name: string;
  description: string;
  tier: "first" | "second";
  author: string;
  repo: string;
  commit: string;
  version: string;
  permissions: { id: string; class: PermissionClass | "unknown" }[];
  installed: boolean;
  installedVersion: string | null;
  updateAvailable: boolean;
  needsNewerPm: boolean;
  apiRetired: boolean;
  /** On PM's safety list, in the list's words. */
  blocked: string | null;
  installable: boolean;
  whyNot: string | null;
}

export interface Catalogue {
  entries: CatalogueEntry[];
  /** The catalogue can install now (an index inside its life). */
  usable: boolean;
  unavailable: string | null;
}

// ---- Uninstalling -----------------------------------------------------------------------------------

export interface UninstallPreview {
  id: string;
  name: string;
  leaves: {
    contentBytes: number;
    cacheBytes: number;
    documentDataBytes: number;
    filesBytes: number;
    navTab: string | null;
    calendarOverlay: string | null;
  };
  /** What stays: the tasks, milestones and documents the mod made. */
  stays: { tasks: number; milestones: number; documents: number };
  removedBySafetyList: boolean;
}

export interface GiftsDeleted {
  tasks: number;
  milestones: number;
  documents: number;
  /** The first thing that couldn't be deleted; the rest went regardless. */
  problem: string | null;
}

/** What the page removes from this device's `localStorage` after an uninstall. */
export interface ModPrune {
  /** Drop this tab id from `pm.nav.placement`'s `hidden` and `pinned`. */
  tab: string;
  /** Drop every `pm.fold.` key that, after the prefix, is exactly this or starts with this and a
   *  `:` (the Mods tab keys a mod's folds `mods.mod.<id>` and `mods.mod.<id>:<section>`). */
  foldPrefix: string;
}

export interface ModUninstalled {
  id: string;
  filesRemoved: boolean;
  giftsDeleted: GiftsDeleted | null;
  prune: ModPrune;
}

// ---- The disclaimer and the safety list ---------------------------------------------------------------

export interface ModsDisclaimerState {
  /** This device accepted it in this vault. */
  accepted: boolean;
  problem: string | null;
}

/** One strip, as `mods://notice` sends it and `modsSafetyStatus` derives it. */
export type ModNotice =
  | {
      kind: "paused";
      id: string;
      name: string;
      version: string;
      reason: string;
      fixedIn: string | null;
    }
  | { kind: "resumed"; id: string; name: string; version: string }
  | { kind: "removed"; id: string; name: string; version: string; reason: string }
  | { kind: "updateWaiting"; id: string; name: string; version: string }
  | { kind: "updateNeedsNewerPm"; id: string; name: string; version: string; needsApi: string }
  | { kind: "recordFault"; unreadable: boolean; text: string };

export interface ModsSafetyStatus {
  checked: {
    kind: "never" | "checked" | "stale";
    at: string | null;
    /** "Safety list not checked since 03-11-2026." for `stale`. */
    text: string | null;
  };
  /** This machine's record of the safety list is unusable. `damaged` offers the reset. */
  record: { kind: "damaged" | "unreadable"; text: string } | null;
  index: { seq: number; issued: string; expires: string; current: boolean } | null;
  notices: ModNotice[];
}

export interface ModsResetResult {
  reset: boolean;
  fetched: boolean;
  problem: string | null;
}

/** Why `mods://changed` fired. The page's answer is the same for every cause: read the rows again. */
export type ModsChangedCause =
  | "recovered"
  | "installed"
  | "uninstalled"
  | "paused"
  | "resumed"
  | "consented"
  | "safety"
  | "updated"
  | "loaded"
  | "decided";

export interface ModsChanged {
  cause: ModsChangedCause;
}
