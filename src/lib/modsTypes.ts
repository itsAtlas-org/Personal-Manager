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
  | "dev-mode-off"
  | "confirm-mismatch"
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

/** A plain sentence for every code, for the day the backend sends one with no message of its own
 *  (the backend always words its refusals, so this is the page's last resort and never overrides
 *  it). `Record<ModsErrorCode, …>` makes a code added to the union without words a compile error. */
export const ERROR_WORDS: Record<ModsErrorCode, string> = {
  "not-main-window": "This can only be done from PM's main window.",
  "no-store":
    "PM's data isn't open on this computer right now: it is locked, or another PM window is in charge.",
  locked: "PM is locked. Unlock it and try again.",
  "not-installed": "That mod isn't installed.",
  "not-in-catalogue": "The catalogue doesn't list that mod, or PM hasn't downloaded it yet.",
  "bad-request": "PM couldn't make sense of that request.",
  "consent-expired":
    "That screen ran out of time, so nothing was installed. Start again to get a fresh one.",
  "file-too-large": "That file is too big to be a mod.",
  "file-unreadable": "PM couldn't read that file.",
  "not-a-mod": "That file isn't a mod PM can use.",
  unsigned: "That file isn't signed. PM only takes an unsigned file when developer mode is on.",
  "safety-list": "PM's safety list says no to this one.",
  "needs-newer-pm": "This mod needs a newer PM than this one.",
  "api-retired": "This mod was made for an older PM, and its maker hasn't updated it.",
  refused: "PM won't install this.",
  "not-from-catalogue":
    "That mod didn't come from the catalogue, so PM can't fetch it again. Install its file again.",
  changed: "That mod changed while PM was working on it. Nothing was changed. Try again.",
  "grant-refused": "PM can't allow that combination of permissions.",
  "not-allowed": "PM doesn't allow that right now.",
  offline: "PM couldn't reach the internet. Check your connection and try again.",
  "download-refused": "PM refused the download because it wasn't what the catalogue promised.",
  "dev-mode-off":
    "Developer mode is off on this computer. Switch it on in Settings, under Developer.",
  "confirm-mismatch": "That isn't the mod's id. Type it exactly as shown.",
  internal: "Something went wrong inside PM. Try again.",
};

/** The sentence for a typed refusal: the backend's own words, else PM's for its code. */
export function modsErrorSentence(err: ModsError): string | null {
  if (err.message.trim()) return err.message;
  return Object.prototype.hasOwnProperty.call(ERROR_WORDS, err.code) ? ERROR_WORDS[err.code] : null;
}

/** What to tell the user about any caught rejection of a Mods command: the backend's own sentence
 *  when it sent a typed refusal (PM's own words for its code if it sent none), else an error's
 *  message, else `fallback`. */
export function failureText(e: unknown, fallback: string): string {
  const typed = modsErrorOf(e);
  if (typed) return modsErrorSentence(typed) ?? fallback;
  return e instanceof Error && e.message ? e.message : fallback;
}

// ---- The installed list ---------------------------------------------------------------------------

/** How a row stands, in kinds to branch on. */
export type ModStandingKind =
  | "running"
  | "waiting-for-you"
  | "paused-by-you"
  | "paused-frozen"
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

/** Whether PM paused this mod because developer mode is off (the standing the backend calls
 *  `paused-dev-mode`, state reason `dev-mode`). The page never works a standing out for itself, so a
 *  mod paused for any other reason, or running, is never this. */
export function pausedForDevMode(mod: { state: Pick<ModStanding, "kind"> }): boolean {
  return mod.state.kind === "paused-dev-mode";
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
  /** `third` is an unsigned file: `unsigned` is then set, and author, repo and commit are only what
   *  the file claims about itself. */
  tier: "first" | "second" | "third";
  kind: "fresh" | "update" | "repair";
  installedVersion: string | null;
  /** `folder` is a developer's test folder (`loadTestMod`): `unsigned` is set, and `tier` is `third`
   *  whatever the manifest says. */
  from: "file" | "catalogue" | "folder";
  sha256: string;
  permissions: ConsentPermission[];
  internetHosts: string[];
  secrets: { name: string; host: string }[];
  navTab: string | null;
  calendarOverlay: string | null;
  /** What an update asks for that the installed version didn't. */
  widening: string[];
  /** Set for an unsigned file: the stark screen's extras. `null` for a signed mod. */
  unsigned: UnsignedScreen | null;
}

/** The stark screen of an unsigned file (`installUnsignedFile`): nobody has checked it, it is judged
 *  together with the other unsigned mods (D3), and installing asks the user to type its id. */
export interface UnsignedScreen {
  /** What the user types, exactly, to confirm: send it as `installModConsented`'s `confirmId`. */
  confirmId: string;
  /** It replaces an earlier unsigned install of this id: a fresh install, grants and secrets reset
   *  unless approved again, its stored data kept. */
  reinstall: boolean;
  /** The web engine is sealed (an unsigned mod can't be granted a read otherwise). */
  engineSealed: boolean;
  /** Set when the file asks for both reading the user's data and reaching outside PM: the user may
   *  allow one side, never both. */
  readOrInternet: {
    read: string[];
    internet: string[];
    /** Which side another unsigned mod already holds, if any. */
    heldByOthers: "read" | "internet" | null;
  } | null;
  /** Set when this is a developer's test folder and not a file: the folder, as a path to show. The
   *  screen then says the mod, and what it stores, last until PM restarts, and that the folder is
   *  read in place and never copied or changed. */
  testFolder: string | null;
}

/** What `devModeState` answers: developer mode for THIS device in this vault. */
export interface DevModeState {
  on: boolean;
  /** Why it reads as off when this device couldn't be named. */
  problem: string | null;
}

/** One row of the dispatch table, as `devKitInfo` lists it. */
export interface KitEndpoint {
  name: string;
  /** A frame asks (`call`) or PM tells a frame (`event`). */
  kind: "call" | "event";
  /** The permission it needs; null for what any session has. */
  permission: string | null;
  class: PermissionClass;
  /** `M.m` the API version it arrived in, was deprecated in, or was removed in. */
  added: string;
  deprecated: string | null;
  removed: string | null;
  /** Outside the API freeze (D13): may change in a minor. */
  experimental: boolean;
}

/** The build-time limits, in the units their names say. */
export interface KitLimits {
  callsPerSecond: number;
  callBurst: number;
  maxInFlight: number;
  maxArgsBytes: number;
  maxResultBytes: number;
  maxBinaryResultBytes: number;
  maxDocsPerCall: number;
  storageQuotaBytes: number;
  storageGrowthPerSecond: number;
  storageGrowthBurst: number;
  eventWindow: number;
  maxEventBytes: number;
  maxIdsPerEvent: number;
  eventCoalesceMs: number;
  maxPendingIds: number;
  // The experimental calls' limits (PR B); times in milliseconds.
  callTimeoutMs: number;
  aiMaxInFlight: number;
  aiCallsPerMinute: number;
  aiCallBurst: number;
  maxAiPromptBytes: number;
  maxAiMessages: number;
  maxAiOutputTokens: number;
  netMaxInFlight: number;
  netFetchesPerSecond: number;
  netFetchBurst: number;
  maxFetchRequestBytes: number;
  maxFetchResponseBytes: number;
  fetchConnectTimeoutMs: number;
  fetchTotalTimeoutMs: number;
  maxFetchRedirects: number;
  notifyMaxChars: number;
  notifyPerDay: number;
  secretAsksPerSession: number;
}

/** What `devKitInfo` answers: the Developer kit's rail sections. */
export interface DevKitInfo {
  api: {
    /** `M.m`, the mod API this PM serves. */
    version: string;
    /** The oldest major still served: the deprecation window's far end. */
    oldestMajor: number;
    /** The window, in words. */
    window: string;
    /** Names of the methods and events outside the API freeze. */
    experimental: string[];
  };
  endpoints: KitEndpoint[];
  limits: KitLimits;
  engine: {
    sealed: boolean;
    /** Why it isn't sealed, one sentence each. */
    unsealedReasons: string[];
    /** Connections that reached the sinkhole since launch. */
    sinkholeAttempts: number;
  };
  /** Each mounted frame's event counters. */
  sessions: { modId: string; sent: number; acked: number; dropped: number; closed: boolean }[];
}

/** What `devModeSwitch` did: developer mode now, and which unsigned mods it paused or resumed, and
 *  which test folders it unloaded (switching off ends every one: they last only for the session). */
export interface DevSwitch {
  on: boolean;
  paused: string[];
  resumed: string[];
  unloaded: string[];
}

/** A loaded test folder (`listTestMods`). The mod's own row, in `listMods`, says its name and state. */
export interface TestModStatus {
  id: string;
  /** The folder, as a path to show. */
  folder: string;
  /** Why the last reload didn't take, in words, until one does. The old files keep running. */
  problem: string | null;
  /** PM is watching the folder, so a save reloads it. Otherwise the Reload button is the way. */
  watching: boolean;
}

/** What `reloadTestMod` did. */
export interface Reloaded {
  /** What was read differs from what was running (a scan that matches restarts no frame). */
  changed: boolean;
  version: string;
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
  | "decided"
  | "dev-mode"
  | "test-mod";

export interface ModsChanged {
  cause: ModsChangedCause;
}
