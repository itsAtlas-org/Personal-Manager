// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Every sentence of a role's ordered list of models, and the small pure decisions around them, in one
// reviewable file (brief 6.11): the editor (RoleRoute, RouteRow, AddModelPicker), the footer, the
// composer chip and the Local AI tab's one-line status all word themselves from here. What a row's
// status IS comes from Rust (`route_status`); this only says it. The page never works out whether a
// row can answer, and never walks a list: PM's backend does both.

import {
  chatUpgradeApplies,
  directInfo,
  isDirect,
  isEligible,
  providerName,
  providerView,
  readsFree,
  roleName,
  termsFor,
} from "./directProviders";
import { formatClock } from "./format";
import type {
  CloudProviderId,
  DirectProvidersState,
  DirectProviderView,
  RoleName,
  RouteRow,
  RouteRowStatus,
  RouteView,
  Settings,
} from "./types";

/** The one sentence that is always on screen under a role's heading. */
export const LIST_RULE = "PM tries these top to bottom. A model that isn't here is never used.";

/** The fold under the list. Status, gating hints and warnings are NEVER in it. */
export const HOW_TITLE = "How the order works";

export const ADD_LABEL = "Add a model";

/** The paragraphs of the fold. Each says what the code does and nothing more. */
export function howParagraphs(): string[] {
  return [
    "PM starts at the top. If a row can't answer, it goes down to the next one, but only when it knows the one above did not charge for this request: a key that is turned away, a service that is busy, a model that has gone. If a reply had already started, or PM can't tell, it stops and asks you instead, so one request is never charged twice.",
    "Rows from OpenRouter that sit together are sent as one request, and OpenRouter tries them in that order.",
    "A daily limit stops a request. PM won't move it to another company's paid model by itself. This computer is the one row it does move on to, because that costs nothing.",
    "A mod that asks for AI uses the OpenRouter rows, and this computer when it sits above them, but only when your first cloud row is OpenRouter, or the list is only this computer. A mod never uses a model you reach with your own key.",
    "This computer can sit anywhere in the list, once.",
    "Effort is how hard a model thinks before it answers. More effort is slower and costs more. Background work runs a lot, so Low keeps it quick and cheap.",
  ];
}

// ---- A row ---------------------------------------------------------------------------------------

/** What a provider is called on a row's chip and in "Claude isn't working, Gemini is next". */
const SHORT: Record<string, string> = {
  anthropic: "Claude",
  gemini: "Gemini",
  openai: "OpenAI",
  openrouter: "OpenRouter",
  local: "This computer",
};

export function shortName(provider: string): string {
  return SHORT[provider] ?? providerName(provider);
}

/** What a Local row is called where it names a place. The same words as the card and the strip
 *  (Rust's `local_company`): "your Local AI server" when the server is another machine. */
export const LOCAL_ELSEWHERE_NAME = "Your Local AI server";

/** The provider chip: "Claude, your key", "OpenRouter", "This computer" ("Your Local AI server"
 *  when Local AI is on another machine). */
export function chipLabel(row: Pick<RouteRow, "provider" | "local_elsewhere">): string {
  if (row.provider === "local" && row.local_elsewhere) return LOCAL_ELSEWHERE_NAME;
  if (row.provider === "local" || row.provider === "openrouter") return shortName(row.provider);
  return `${shortName(row.provider)}, your key`;
}

/** Who gets the reply, for the quiet tag on a direct row. */
export function companyOf(provider: string): string {
  return directInfo(provider)?.company ?? providerName(provider);
}

export function directTag(provider: string): string {
  return `Sent straight to ${companyOf(provider)}, not through OpenRouter`;
}

/** The provider's own one-line warning, the same words its card and the old provider Select note
 *  used: the I-13 warning is shown wherever a direct model is chosen. Gemini's depends on the
 *  key's terms, so it is read from the provider's own view when there is one. */
export function providerWarning(provider: string, view?: DirectProviderView): string | null {
  const info = directInfo(provider);
  if (!info) return null;
  return view ? termsFor(info, view).headline : info.headline;
}

/** "14:05" for the status line. A missing or unreadable time is left out, never invented. */
function clockOf(since: string | undefined): string | null {
  if (!since) return null;
  const d = new Date(since);
  return Number.isNaN(d.getTime()) ? null : formatClock(d);
}

/** The status line, as visible words (never colour alone). */
export function statusText(status: RouteRowStatus, provider: string): string {
  const name = providerName(provider);
  switch (status.kind) {
    case "ready":
      return "Ready";
    case "no_key":
      return "No key saved";
    case "key_unreadable":
      return "PM can't read the key right now";
    case "not_on":
      return `Turn on ${name} in Your own keys`;
    case "paused_region":
      return "Paused where you are";
    case "paused_limit":
      return "Paused for today";
    case "not_working": {
      const at = clockOf(status.since);
      return `Not working${at ? ` since ${at}` : ""}${status.reason ? `: ${status.reason}` : ""}`;
    }
    case "local_off":
      return "Local AI is off";
    case "no_local_model":
      return "No model chosen on this computer";
    // The walk stops at an earlier This computer row it will not run, so this row is never tried.
    case "after_local_off":
      return "Not tried while Local AI is off";
    case "after_local_pick":
      return "Not tried until a model is chosen on this computer";
  }
}

/** Whether a status wears the attention tint. Ready is quiet; everything else is a thing to look at. */
export function statusNeedsLook(kind: RouteRowStatus["kind"]): boolean {
  return kind !== "ready";
}

/** What this computer does when it fails, said on its own row. */
export function localFailureLine(isLast: boolean, elsewhere = false): string {
  const who = elsewhere ? "your Local AI server" : "this computer";
  return isLast
    ? `If ${who} can't answer, nothing else is tried.`
    : `If ${who} can't answer, the next row is tried.`;
}

/** Under the second and later of a run of OpenRouter rows. */
export const GROUP_LINE = "Sent together with the row above; OpenRouter tries them in this order";

/** Whether a row sits directly under another OpenRouter row. */
export function groupedWithAbove(entries: RouteRow[], i: number): boolean {
  return i > 0 && entries[i].provider === "openrouter" && entries[i - 1].provider === "openrouter";
}

/** Whether a list holds exactly the default's CLOUD rows in the default's order. This computer is
 *  the person's own Local AI choice and not a thing Reset puts back or takes out, so a list that
 *  differs from the default only by it has nothing to reset. A list with no cloud row at all
 *  ("Local only") has nothing to reset either: Reset would add a cloud fallback the person never
 *  chose, so it is not offered. */
export function atDefaultCloudRows(route: RouteView, defaults: RouteView): boolean {
  const keys = (r: RouteView) => r.entries.filter((e) => e.kind !== "local").map((e) => e.key);
  const a = keys(route);
  const b = keys(defaults);
  if (a.length === 0) return true;
  return a.length === b.length && a.every((k, i) => k === b[i]);
}

/** What the row's name is, for the buttons and the announcements. */
export function rowName(row: Pick<RouteRow, "name" | "provider" | "local_elsewhere">): string {
  if (row.provider !== "local") return row.name;
  if (row.local_elsewhere) {
    return row.name === "This computer"
      ? LOCAL_ELSEWHERE_NAME
      : `${row.name} on your Local AI server`;
  }
  return row.name !== "This computer" ? `${row.name} on this computer` : row.name;
}

// ---- Buttons and what a screen reader hears ------------------------------------------------------
// The buttons carry the model and its CURRENT place, so two "Move up" buttons are two names.

export function moveName(name: string, dir: "up" | "down", index: number, count: number): string {
  return `Move ${name} ${dir}, now ${index + 1} of ${count}`;
}

export function removeName(name: string): string {
  return `Remove ${name}`;
}

export function effortName(name: string): string {
  return `Effort for ${name}`;
}

export function tryAgainName(name: string): string {
  return `Try again now, ${name}`;
}

export function movedAnnouncement(name: string, place: number, count: number): string {
  return `${name} is now number ${place} of ${count}`;
}

export function removedAnnouncement(name: string, left: number, role: RoleName): string {
  return `Removed ${name}. ${left} ${left === 1 ? "model" : "models"} left in ${roleName(role)}`;
}

export function addedAnnouncement(name: string): string {
  return `Added ${name} at the end`;
}

/** The first edit's one line: saved OpenRouter models the list leaves out because they were not in
 *  use. Null when nothing was left out. */
export function firstEditLine(unusedSaved: number): string | null {
  if (unusedSaved <= 0) return null;
  return unusedSaved === 1
    ? "1 other saved model wasn't in use and isn't in this list."
    : `${unusedSaved} other saved models weren't in use and aren't in this list.`;
}

/** The context meter's "switch to a larger model": the model goes where chat's first OpenRouter row
 *  stands, and every other row of the list stays where the person put it (the backend does the edit
 *  from the ONE model id: sending the whole OpenRouter set would pull a later OpenRouter row up
 *  ahead of a row the person put above it). `write` answers how many saved models the FIRST edit of
 *  a list nobody had edited left out (they were not in use). Resolves to the one line the list
 *  editor says for that (`firstEditLine`), or null when nothing was left out, so a larger model
 *  never drops saved models without a word. Does nothing for a chat on a provider of the person's
 *  own (that list is OpenRouter's and nothing there reads it). */
export async function upgradeChatModel(
  settings: Settings | null,
  modelId: string,
  write: (model: string) => Promise<number | void>,
): Promise<string | null> {
  if (!chatUpgradeApplies(settings)) return null;
  const unused = await write(modelId);
  return firstEditLine(typeof unused === "number" ? unused : 0);
}

export const ONE_ROW_HINT = "Add a second model so a reply doesn't stop when this one is down.";

/** The standing line for Background when its first cloud row is a provider of the person's own: mods
 *  get no AI then, and the person is told where they will notice it. */
export function modsGetNoAnswer(provider: string): string {
  return `Mods can't use your own keys. While Background is on ${providerName(provider)}, a mod that asks for AI gets no answer.`;
}

/** The heading of a role's section, and the name its controls carry. */
export function sectionName(role: RoleName): string {
  return role === "chat" ? "Chat model" : "Background model";
}

// ---- What the page asks of the list --------------------------------------------------------------

/** The first row that is a cloud model, or undefined. */
export function firstCloud(route: RouteView): RouteRow | undefined {
  return route.entries.find((e) => e.kind === "cloud");
}

/** Whether mods get no AI from this role: its first cloud row is a provider of the person's own
 *  (6.9). Only the role a mod can ask for matters to the sentence, which is Background's. */
export function modsBlocked(route: RouteView): RouteRow | undefined {
  const c = firstCloud(route);
  return c && isDirect(c.provider) ? c : undefined;
}

/** Whether something else could answer if this list's only row went down: a second provider that is
 *  on, keyed and not paused, or this computer. Only then is the one-row hint worth saying. */
export function somethingElseUsable(
  route: RouteView,
  dp: DirectProvidersState | null,
  openRouterKey: boolean,
  local: LocalFacts,
): boolean {
  const here = new Set(route.entries.map((e) => e.provider));
  if (!here.has("openrouter") && openRouterKey) return true;
  if (!here.has("local") && local.on && !!local.model) return true;
  const usableDirect = (dp?.providers ?? []).some(
    (p) => !here.has(p.provider) && isEligible(p) && !p.limit.reached,
  );
  return usableDirect;
}

// `headIndex` and `fallbackCount` (the footer's "+N") live in directProviders.ts beside `footerRow`,
// which this file's own imports come from; they are re-exported for the pages that word the list.
export { fallbackCount, headIndex } from "./directProviders";

/** "Claude isn't working, Gemini is next": the composer chip's line when the FIRST row is down and a
 *  later one can answer. Null when the head is fine, or nothing is behind it.
 *
 *  A head paused by its DAILY LIMIT is different: PM stops there (the "not working" card) and never
 *  walks to another company's paid row by itself (R3b). Only this computer, which costs nothing, can
 *  answer after a limit, so the chip says "paused for today" and names a next row only when that is a
 *  ready This-computer row; otherwise it says nothing and the provider's own chip speaks. */
export function headDownNote(route: RouteView | undefined | null): string | null {
  const head = route?.entries[0];
  if (!route || !head || head.status.kind === "ready") return null;
  if (head.status.kind === "paused_limit") {
    const local = route.entries
      .slice(1)
      .find((e) => e.kind === "local" && e.status.kind === "ready");
    if (!local) return null;
    return `${rowLabel(head)} is paused for today, ${rowLabel(local)} is next`;
  }
  const next = route.entries.slice(1).find((e) => e.status.kind === "ready");
  if (!next) return null;
  // Two rows of one provider are told apart by model, not by "OpenRouter isn't working, OpenRouter
  // is next".
  const same = head.provider === next.provider;
  const label = (r: RouteRow) => (same ? rowName(r) : rowLabel(r));
  return `${label(head)} isn't working, ${label(next)} is next`;
}

/** A row by company, for a line that names two different ones. This computer is lower-case in the
 *  middle of a sentence, and a Local AI server on another machine is never "this computer". */
function rowLabel(r: RouteRow): string {
  if (r.provider !== "local") return shortName(r.provider);
  return r.local_elsewhere ? "your Local AI server" : "this computer";
}

// ---- The wizard's backup offer (brief 7.2 P4) ----------------------------------------------------

/** The one unticked checkbox the first-run wizard may show: "If X isn't working, try Y next." */
export interface BackupOffer {
  /** The provider the roles are on, and the one the checkbox would add behind it. */
  from: CloudProviderId;
  to: CloudProviderId;
  /** The rows ticking it adds, one per role that is on `from`: the other provider's own default model
   *  for that role. */
  rows: { role: RoleName; model: string }[];
  text: string;
}

/** What the wizard may offer once a key is saved, or null for nothing. Offered only where the other
 *  provider could answer at once (a provider is on, keyed and not paused, or OpenRouter has a key),
 *  so a checkbox that is shown is one that does something; ticking writes through the same list edit
 *  as the editor, which checks it again. A free Gemini key never gets Background as a backup: it
 *  takes Chat only, as everywhere. Nothing is ever added unless the person ticks it. */
export function backupOffer(
  dp: DirectProvidersState | null,
  openRouterKey: boolean,
  routes: { chat: RouteView | null; background: RouteView | null },
  openRouterModels: { chat?: string; background?: string },
): BackupOffer | null {
  if (!dp) return null;
  const from = dp.roles.chat.effective;
  const usable = (id: CloudProviderId, atEase = true): boolean => {
    if (id === "openrouter") return openRouterKey;
    const v = providerView(dp, id);
    // A provider paused by its daily limit is fine as the one roles are ON, not as a new backup.
    return !!v && isEligible(v) && (!atEase || !v.limit.reached);
  };
  if (!usable(from, false)) return null;
  const candidates: CloudProviderId[] = ["openrouter", ...dp.providers.map((p) => p.provider)];
  const to = candidates.find((id) => id !== from && usable(id));
  if (!to) return null;
  const rows: BackupOffer["rows"] = [];
  for (const role of ["chat", "background"] as RoleName[]) {
    const route = routes[role];
    if (!route || dp.roles[role].effective !== from) continue;
    if (route.entries.some((e) => e.provider === to)) continue;
    const view = to === "openrouter" ? undefined : providerView(dp, to);
    if (view && role === "background" && readsFree(view)) continue;
    const model = view ? view.roles[role].model : openRouterModels[role];
    if (model) rows.push({ role, model });
  }
  if (rows.length === 0) return null;
  return {
    from,
    to,
    rows,
    text: `If ${providerName(from)} isn't working, try ${providerName(to)} next.`,
  };
}

// ---- The picker ----------------------------------------------------------------------------------

/** What the picker needs to know about this computer. */
export interface LocalFacts {
  /** The Local AI switch. */
  on: boolean;
  /** The model chosen for the role, or null. */
  model: string | null;
  /** Local AI points at a server on ANOTHER machine (the loopback test the rows and the card make):
   *  the section then says "Your Local AI server", because the text leaves this computer. */
  elsewhere?: boolean;
}
export interface PickerSection {
  provider: CloudProviderId | "local";
  title: string;
  /** The section's eligibility line, or null when nothing needs saying. */
  line: string | null;
  /** The line says the section can't be used. A paused-for-today provider can still be added. */
  blocked: boolean;
}

/** One section's eligibility, from facts Rust already worked out (`direct_providers_state`). The
 *  write is checked again in Rust whatever this says. */
export function sectionFor(
  provider: CloudProviderId | "local",
  dp: DirectProvidersState | null,
  openRouterKey: boolean,
  local: LocalFacts,
): PickerSection {
  const title =
    provider === "local"
      ? local.elsewhere
        ? LOCAL_ELSEWHERE_NAME
        : "This computer"
      : providerName(provider);
  if (provider === "local") {
    // Says the way out, not only the state: nothing in this dialog turns Local AI on.
    if (!local.on) {
      return {
        provider,
        title,
        line: "Local AI is off. Turn it on just below the lists first.",
        blocked: true,
      };
    }
    if (!local.model) {
      return { provider, title, line: "Pick a local model in Local AI first", blocked: true };
    }
    return { provider, title, line: null, blocked: false };
  }
  if (provider === "openrouter") {
    // OpenRouter takes a row with no key (the key may be typed above a moment later); it says so.
    return {
      provider,
      title,
      line: openRouterKey ? null : "No key saved yet. This won't answer until you add one above.",
      blocked: false,
    };
  }
  const view = dp ? providerView(dp, provider) : undefined;
  if (!view || !view.opted_in) {
    return { provider, title, line: `Turn on ${title} in Your own keys first`, blocked: true };
  }
  if (view.key === "absent") return { provider, title, line: "No key saved", blocked: true };
  if (view.key === "unreadable") {
    return { provider, title, line: "PM can't read the key right now", blocked: true };
  }
  if (view.blocked) return { provider, title, line: "Paused where you are", blocked: true };
  if (!isEligible(view)) {
    return {
      provider,
      title,
      line: `${title} can't be used right now. Check it in Your own keys.`,
      blocked: true,
    };
  }
  if (view.limit.reached) {
    return {
      provider,
      title,
      line: "Paused for today by your daily limit. You can still add it.",
      blocked: false,
    };
  }
  return { provider, title, line: null, blocked: false };
}

/** The picker's sections in the order a person reads them: OpenRouter, each provider with a card, then
 *  this computer. */
export function pickerSections(
  dp: DirectProvidersState | null,
  openRouterKey: boolean,
  local: LocalFacts,
): PickerSection[] {
  const providers: CloudProviderId[] = (dp?.providers ?? []).map((p) => p.provider);
  return [
    sectionFor("openrouter", dp, openRouterKey, local),
    ...providers.map((p) => sectionFor(p, dp, openRouterKey, local)),
    sectionFor("local", dp, openRouterKey, local),
  ];
}

/** A catalogue row's price line, per million tokens. */
export function priceLine(inPerM: number, outPerM: number): string {
  const f = (n: number) => (n < 1 ? n.toFixed(2) : n % 1 === 0 ? n.toFixed(0) : n.toFixed(2));
  return `$${f(inPerM)} in, $${f(outPerM)} out per million tokens`;
}

export const LOCAL_LINK_TEXT = "Where this computer sits in each list is set in AI & Models";

/** The Local AI tab's one line for a role: where this computer sits in that role's list. */
export function localPlaceLine(role: RoleName, route: RouteView | null): string | null {
  if (!route) return null;
  const i = route.entries.findIndex((e) => e.kind === "local");
  const who = roleName(role);
  if (i < 0) return `This computer is not in ${who}'s list.`;
  if (route.entries.length === 1) return `This computer is ${who}'s only row.`;
  return `This computer is number ${i + 1} of ${route.entries.length} in ${who}'s list.`;
}
