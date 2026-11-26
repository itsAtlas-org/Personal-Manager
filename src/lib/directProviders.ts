// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Everything the UI says or decides about a provider the person reaches with their own key, kept
// pure so it is tested rather than seen. Anything provider-specific in the COPY lives in the one
// descriptor row below, so Gemini and Anthropic add a row (and a catalogue section in Rust), not a
// component. What a request does, where it goes and what a move is stays in Rust: this file only
// words what the backend's own answers say (`direct_providers_state`, the key check, a `RoleMove`).

import type {
  CloudProviderId,
  DirectModelView,
  DirectProvidersState,
  DirectProviderView,
  KeyOutcome,
  ModelInfo,
  RoleMove,
  RoleName,
  Settings,
  StoredKeyCheck,
} from "./types";

/** The providers that have a card in this version. 2b adds "gemini", 3 adds "anthropic". */
export type DirectProviderId = "openai";

export interface DirectProviderInfo {
  id: DirectProviderId;
  name: string;
  toggleLabel: string;
  /** The line under the toggle while it is off, and when it is off with a key kept. */
  offLine: string;
  offWithKeyLine: string;
  keyLabel: string;
  keyHint: string;
  keyDescription: string;
  keyUrl: string;
  keyLinkText: string;
  /** The unfolded warning: a headline and short points. */
  headline: string;
  sharedHeadline: string;
  points: string[];
  /** The folded SectionInfo, paragraph by paragraph. */
  detailsTitle: string;
  details: string[];
  sharesDataOption: { label: string; note: string };
  /** What the composer chip and the sidebar row say. */
  chipLabel: string;
  chipSharedLabel: string;
}

export const DIRECT_PROVIDERS: Record<DirectProviderId, DirectProviderInfo> = {
  openai: {
    id: "openai",
    name: "OpenAI",
    toggleLabel: "Use my own OpenAI key",
    offLine: "Off. PM uses OpenRouter, which asks for zero data retention on every request.",
    offWithKeyLine: "Off. Your key is saved but PM isn't using it.",
    keyLabel: "OpenAI API key",
    keyHint: "sk-...",
    keyDescription:
      "Paste a key from your OpenAI project. PM keeps it in your computer's keychain.",
    keyUrl: "https://platform.openai.com/api-keys",
    keyLinkText: "Get a key at platform.openai.com/api-keys →",
    headline: "This isn't zero data retention.",
    sharedHeadline: "OpenAI may train on what you send.",
    points: [
      "OpenAI doesn't train on what you send by default.",
      "OpenAI keeps requests in its abuse logs for up to 30 days. Zero retention only comes from OpenAI, by agreement with them.",
      "PM asks OpenAI not to store your requests. If your OpenAI project joined its data-sharing or free-tokens programme, OpenAI does share what you send.",
    ],
    detailsTitle: "What OpenAI keeps",
    details: [
      "OpenAI's API doesn't use your data to train its models unless your organisation chooses to share it.",
      "Requests are kept in abuse-monitoring logs for up to 30 days, longer if the law says so. You can't switch that off yourself. OpenAI may also use content from a limited group of API customers to train safety filters, without any opt-in.",
      "Zero data retention and Modified Abuse Monitoring need OpenAI's approval. Ask OpenAI if you need them.",
      "PM asks for no stored responses and never uses OpenAI's files, threads or batch tools. Prompt caching can keep parts of a request on OpenAI's machines for up to 24 hours.",
      "These are OpenAI's terms when this version of PM was built. They can change. OpenAI's data page is the one to trust.",
    ],
    sharesDataOption: {
      label: "My OpenAI project shares data to get free tokens",
      note: "PM can't see this setting, so tell it. If you tick it, PM warns you that OpenAI may train on what you send.",
    },
    chipLabel: "OpenAI direct",
    chipSharedLabel: "OpenAI direct, shared",
  },
};

/** The descriptor for a provider token, or null for OpenRouter and any token this version has no
 *  card for (a backup from a newer PM). */
export function directInfo(id: string): DirectProviderInfo | null {
  return Object.prototype.hasOwnProperty.call(DIRECT_PROVIDERS, id)
    ? DIRECT_PROVIDERS[id as DirectProviderId]
    : null;
}

/** Anything but OpenRouter is a provider the person brought, and so is not zero data retention. An
 *  unknown token counts: the cautious reading of a word this version can't place. */
export function isDirect(id: CloudProviderId | string): boolean {
  return id !== "openrouter";
}

/** "OpenRouter", a direct provider's own name, or the raw token (never throws on a word it doesn't
 *  know). */
export function providerName(id: CloudProviderId | string): string {
  if (id === "openrouter") return "OpenRouter";
  return directInfo(id)?.name ?? id;
}

export function roleName(role: RoleName): string {
  return role === "chat" ? "Chat" : "Background";
}

// ---- Effort -------------------------------------------------------------------------------------

export const EFFORT_LABEL: Record<string, string> = {
  none: "Off",
  minimal: "Minimal",
  low: "Low",
  medium: "Medium",
  high: "High",
  xhigh: "Extra high",
  max: "Max",
};

/** A level's name. A word the map hasn't met (a catalogue row can ship one) is Title-cased, never an
 *  error. */
export function effortLabel(level: string): string {
  const known = Object.prototype.hasOwnProperty.call(EFFORT_LABEL, level)
    ? EFFORT_LABEL[level]
    : null;
  if (known) return known;
  const words = level.replace(/[_-]+/g, " ").trim();
  return words.length === 0 ? level : words.charAt(0).toUpperCase() + words.slice(1);
}

/** What a role uses when nothing is stored: chat omits the field (the model's own default), and
 *  background starts on `low`, or on the model's lowest level when `low` isn't offered. Mirrors
 *  `direct_providers::effort_for`; the backend is the one that sends it, this only words it. */
export function roleDefaultEffort(
  role: RoleName,
  model: DirectModelView | undefined,
): string | null {
  if (role === "chat" || !model) return null;
  const levels = model.effort_levels;
  if (levels.includes("low")) return "low";
  return levels[0] ?? null;
}

export interface EffortOption {
  value: string;
  label: string;
}

/** The effort Select's options, in the model's own levels. Chat's first is "Model default" (value
 *  ""); background has no such option, and marks its own default instead. */
export function effortOptions(role: RoleName, model: DirectModelView | undefined): EffortOption[] {
  if (!model || model.effort_levels.length === 0) return [];
  const def = roleDefaultEffort(role, model);
  const levels = model.effort_levels.map((l) => ({
    value: l,
    label: l === def ? `${effortLabel(l)} (PM's default)` : effortLabel(l),
  }));
  return role === "chat" ? [{ value: "", label: "Model default" }, ...levels] : levels;
}

/** The effort to WRITE when the person picks `value` in the Select. The role default is written as
 *  null, so a role on its default keeps tracking it. */
export function effortToWrite(
  role: RoleName,
  model: DirectModelView | undefined,
  value: string,
): string | null {
  if (value === "") return null;
  return value === roleDefaultEffort(role, model) ? null : value;
}

/** What a model change does to the effort. A role on its default stays on the default; a chosen
 *  level the new model also offers is kept; one it doesn't offer goes back to the default and is
 *  reported (`coercedFrom`), never left for the wire to refuse. */
export function effortAfterModelChange(
  role: RoleName,
  current: string | null,
  from: DirectModelView | undefined,
  to: DirectModelView | undefined,
): { effort: string | null; coercedFrom: string | null } {
  if (current === null) return { effort: null, coercedFrom: null };
  if (current === roleDefaultEffort(role, from)) return { effort: null, coercedFrom: null };
  if (to?.effort_levels.includes(current)) return { effort: current, coercedFrom: null };
  return { effort: null, coercedFrom: current };
}

/** "Effort is now Low. GPT-6 Luna doesn't have High." */
export function effortCoercedLine(
  role: RoleName,
  to: DirectModelView | undefined,
  oldLevel: string,
): string {
  const def = roleDefaultEffort(role, to);
  const now = def === null ? "Model default" : effortLabel(def);
  return `Effort is now ${now}. ${to?.name ?? "That model"} doesn't have ${effortLabel(oldLevel)}.`;
}

// ---- The catalogue as the picker's rows ------------------------------------------------------------

/** A catalogue row as the model picker draws it. */
export interface DirectPickerRow extends ModelInfo {
  /** The row's own note (null on every OpenAI row today). */
  note: string | null;
  /** The key's own list was read and does not name this model. Still selectable. */
  unlisted: boolean;
  /** The model's retirement date, DD-MM-YYYY, when it has one. */
  retires: string | null;
  /** The tooltip: the cached-input price, which the row has no room for. */
  tip: string;
}

/** DD-MM-YYYY from a YYYY-MM-DD, built from the fields (a bare date read as a Date is UTC and slips a
 *  day in a negative zone). Anything else is returned as it came. */
export function dayMonthYear(iso: string): string {
  const m = /^(\d{4})-(\d{2})-(\d{2})/.exec(iso);
  return m ? `${m[3]}-${m[2]}-${m[1]}` : iso;
}

function perM(n: number): string {
  return `$${n < 1 ? n.toFixed(2) : n % 1 === 0 ? n.toFixed(0) : n.toFixed(2)}`;
}

/** The catalogue as `ModelInfo` rows. Prices are per MILLION on the wire and per TOKEN in
 *  `ModelInfo`, which is what the picker multiplies back. */
export function directPickerRows(p: DirectProviderView): DirectPickerRow[] {
  const listed = p.key_check?.outcome === "ok" ? p.key_check.listed : null;
  return p.models.map((m) => ({
    id: m.id,
    name: m.name,
    description: "",
    context_length: m.context,
    prompt_price: m.price_in / 1_000_000,
    completion_price: m.price_out / 1_000_000,
    input_modalities: m.input_modalities,
    note: m.note,
    unlisted: listed !== null && !listed.includes(m.id),
    retires: m.retires_on ? dayMonthYear(m.retires_on) : null,
    tip:
      m.price_cached_in != null
        ? `${m.name}: cached input ${perM(m.price_cached_in)} per million tokens. PM's list, checked on ${dayMonthYear(m.checked_on)}.`
        : `${m.name}. PM's list, checked on ${dayMonthYear(m.checked_on)}.`,
  }));
}

/** The picker's caption: what the last check can say about the list. */
export function pickerCaption(p: DirectProviderView): string {
  return p.key_check?.outcome === "ok"
    ? `Models your ${p.name} key can use.`
    : "PM couldn't check which of these your key can use.";
}

// ---- Key checks ------------------------------------------------------------------------------------

export type KeyVerdictTone = "ok" | "warn" | "refused";

export interface KeyVerdict {
  tone: KeyVerdictTone;
  text: string;
}

/** The raw word of an outcome, for the unknown ones. */
export function outcomeWord(outcome: KeyOutcome): string {
  if (typeof outcome === "string") return outcome;
  if (outcome && typeof outcome === "object" && "unexpected" in outcome) return "unexpected";
  return String(outcome);
}

/** What a key check found, in words. `listedCount` is how many catalogue models the key can see. An
 *  outcome this version has no sentence for is shown as itself and counted as not saved. */
export function keyVerdict(outcome: KeyOutcome, listedCount: number, name: string): KeyVerdict {
  if (typeof outcome === "object" && outcome !== null && "unexpected" in outcome) {
    return {
      tone: "refused",
      text: `Not saved. ${name} answered with an error (${outcome.unexpected}). Try again shortly.`,
    };
  }
  switch (outcome) {
    case "ok": {
      const ready =
        listedCount === 0
          ? `Key works, but none of the models PM knows are in its list. You can still pick one.`
          : listedCount === 1
            ? `Key works. 1 ${name} model is ready to use.`
            : `Key works. ${listedCount} ${name} models are ready to use.`;
      return {
        tone: "ok",
        text: `${ready} A key can pass this check and still be read-only, which can't chat.`,
      };
    }
    case "restricted":
      return {
        tone: "warn",
        text: "Saved. This key is restricted, so PM can't see which models it can use. If a chat fails, give the key access to the models and chat endpoints in your OpenAI project.",
      };
    case "rejected":
      return {
        tone: "refused",
        text: `Not saved. ${name} says this key isn't right. Check you copied the whole key from the right project. If it's an Admin key, make a normal project key instead.`,
      };
    case "region_blocked":
      return {
        tone: "refused",
        text: `Not saved. ${name} doesn't serve requests from where you are right now.`,
      };
    case "malformed":
      return {
        tone: "refused",
        text: "Not saved. That doesn't look like a key. Paste the whole key, with no spaces.",
      };
    case "unreachable":
      return {
        tone: "refused",
        text: `Not saved. PM couldn't reach ${name} to check it. Check your connection and try again.`,
      };
    default:
      return { tone: "refused", text: `Not saved. The key check ended with "${String(outcome)}".` };
  }
}

/** A stored verdict, shown only beside a key that is really there (a verdict carried in a backup is
 *  never drawn next to no key). */
export function storedVerdict(p: DirectProviderView, name: string): KeyVerdict | null {
  if (p.key !== "present" || !p.key_check) return null;
  return keyVerdict(p.key_check.outcome, p.key_check.listed.length, name);
}

/** A stored verdict that says the key was turned away. Only a check that was never kept could say
 *  this, so it guards a hand-edited row and nothing more. */
function turnedAway(c: StoredKeyCheck | null): boolean {
  if (!c) return false;
  return c.outcome !== "ok" && c.outcome !== "restricted";
}

// ---- Who can be chosen, and what is wrong with a choice --------------------------------------------

/** Opted in, key there, last check not a refusal: the providers a role may be moved to. */
export function isEligible(p: DirectProviderView): boolean {
  return p.opted_in && p.key === "present" && !turnedAway(p.key_check);
}

export function eligibleProviders(state: DirectProvidersState): DirectProviderView[] {
  return state.providers.filter(isEligible);
}

export function providerView(
  state: DirectProvidersState,
  id: CloudProviderId | string,
): DirectProviderView | undefined {
  return state.providers.find((p) => p.provider === id);
}

/** Why a role's stored provider can't answer, or null when nothing is wrong. */
export type RoleIssue = "turned_off" | "key_missing" | "key_unreadable";

export function roleIssue(state: DirectProvidersState, role: RoleName): RoleIssue | null {
  const r = state.roles[role];
  if (!isDirect(r.stored)) return null;
  if (r.stored !== r.effective) return "turned_off";
  const p = providerView(state, r.stored);
  if (!p) return "key_missing";
  if (p.key === "unreadable") return "key_unreadable";
  if (p.key === "absent") return "key_missing";
  return null;
}

/** The sentence for a role's issue (the "turned off" one also carries a button, drawn by the caller). */
export function roleIssueLine(issue: RoleIssue, role: RoleName, providerLabel: string): string {
  const who = roleName(role);
  switch (issue) {
    case "turned_off":
      return `${providerLabel} is turned off above.`;
    case "key_unreadable":
      return `${who} is set to ${providerLabel}, but PM can't read your ${providerLabel} key right now. ${who} won't answer until it can, or until you pick another provider.`;
    case "key_missing":
      return `Your ${providerLabel} key is missing on this device. Add it above, or move ${who} back to OpenRouter.`;
  }
}

/** The label of the stored provider's option when it can't be chosen. */
export function unavailableLabel(
  id: CloudProviderId | string,
  state: DirectProvidersState,
  role: RoleName,
): string {
  const name = providerName(id);
  const issue = roleIssue(state, role);
  if (issue === "turned_off") return `${name} (turned off)`;
  if (issue === "key_unreadable") return `${name} (key unreadable)`;
  if (issue === "key_missing") return `${name} (no key)`;
  return directInfo(id) ? name : `${name} (unavailable)`;
}

/** "OpenRouter (asks for zero data retention)" / "OpenAI, direct (no zero data retention)". */
export function providerOptionLabel(id: CloudProviderId | string): string {
  return id === "openrouter"
    ? "OpenRouter (asks for zero data retention)"
    : `${providerName(id)}, direct (no zero data retention)`;
}

/** The sentence under a card or a role when roles were moved back to OpenRouter. `orKey` says
 *  whether each role has an OpenRouter key to land on. */
export function movedLine(
  moves: RoleMove[],
  orKey: { chat: boolean; background: boolean },
): string | null {
  if (moves.length === 0) return null;
  const roles = moves.map((m) => m.role);
  const both = roles.includes("chat") && roles.includes("background");
  const lead = both
    ? "Chat and Background are back on OpenRouter"
    : `${roleName(roles[0])} is back on OpenRouter`;
  const lacking = roles.some((r) => !orKey[r]);
  return lacking
    ? `${lead}, but there's no OpenRouter key yet. Add one above, or choose another provider.`
    : `${lead}.`;
}

/** What a role's direct provider means for the role's routing, said at the control. `routing` is the
 *  Local AI preference ("cloud" | "local" | "local-then-cloud"); with Local AI off it is "cloud". */
export function routeNote(role: RoleName, name: string, routing: string): string {
  if (routing === "local") {
    return `This job runs on your local model. ${name} is only used if you switch it to a cloud routing.`;
  }
  if (routing === "local-then-cloud") {
    return role === "chat"
      ? `${name} answers if your local model can't. That isn't zero data retention.`
      : `${name} answers background work if your local model can't, and then sends parts of your documents. That isn't zero data retention.`;
  }
  return role === "chat"
    ? `Chat goes straight to ${name} with your key. It isn't zero data retention.`
    : `Background work goes straight to ${name} with your key and sends parts of your documents. It isn't zero data retention.`;
}

/** The status read aloud when a role's provider changes. */
export function roleChangedAnnouncement(
  role: RoleName,
  to: CloudProviderId | string,
  modelName: string | null,
): string {
  return to === "openrouter"
    ? `${roleName(role)} moved back to OpenRouter`
    : `${roleName(role)} now uses ${providerName(to)}.${modelName ? ` Model: ${modelName}` : ""}`;
}

/** The effective model of a role on a provider, from the state. */
export function roleModel(p: DirectProviderView, role: RoleName): DirectModelView | undefined {
  const id = p.roles[role].model;
  return p.models.find((m) => m.id === id);
}

/** Whether the first-run wizard may let the person finish with no OpenRouter key: a direct
 *  provider is opted in with a good key, at least one role is on it, and BOTH roles resolve to a
 *  provider that has a key. */
export function directReady(state: DirectProvidersState | null, openRouterKey: boolean): boolean {
  if (!state) return false;
  const roles: RoleName[] = ["chat", "background"];
  let anyDirect = false;
  for (const role of roles) {
    const eff = state.roles[role].effective;
    if (eff === "openrouter") {
      if (!openRouterKey) return false;
      continue;
    }
    const p = providerView(state, eff);
    if (!p || !isEligible(p)) return false;
    anyDirect = true;
  }
  return anyDirect;
}

/** The wizard's line once a direct key is good: which models answer. */
export function wizardModelsLine(
  p: DirectProviderView,
  chatUsesIt: boolean,
  backgroundUsesIt: boolean,
): string | null {
  const chat = roleModel(p, "chat")?.name;
  const background = roleModel(p, "background")?.name;
  if (chatUsesIt && backgroundUsesIt && chat && background) {
    return `PM will use ${chat} for chat and ${background} for background work. You can change that later in Settings > AI & Models.`;
  }
  if (chatUsesIt && chat) {
    return `PM will use ${chat} for chat. You can change that later in Settings > AI & Models.`;
  }
  if (backgroundUsesIt && background) {
    return `PM will use ${background} for background work. You can change that later in Settings > AI & Models.`;
  }
  return null;
}

// ---- The sidebar footer and the composer ---------------------------------------------------------

/** Whether the context meter's "use a larger model" may rewrite chat's model list. That list is
 *  OpenRouter's: a chat on a provider of the person's own never reads it, so writing it would change
 *  a list nothing uses and leave the meter claiming a switch that didn't happen. */
export function chatUpgradeApplies(s: Settings | null): boolean {
  return (s?.chat_provider ?? "openrouter") === "openrouter";
}

/** What the sidebar footer says for a role. The model is the one that really answers (the backend's
 *  `*_cloud_model`), not the head of OpenRouter's list, which a direct role never reads; a direct
 *  role has no auto-switch fallbacks, and says which route it takes. */
export function footerRow(
  s: Settings | null,
  role: RoleName,
): { id: string | null; fallbacks: number; via: string | null } {
  if (!s) return { id: null, fallbacks: 0, via: null };
  const list = role === "chat" ? s.chat_models : s.background_models;
  const auto = role === "chat" ? s.chat_auto_switch : s.background_auto_switch;
  const provider = (role === "chat" ? s.chat_provider : s.background_provider) ?? "openrouter";
  const cloud = role === "chat" ? s.chat_cloud_model : s.background_cloud_model;
  if (isDirect(provider)) {
    const info = directInfo(provider);
    return {
      id: cloud ?? null,
      fallbacks: 0,
      via: info ? info.chipLabel : `${providerName(provider)} direct`,
    };
  }
  return {
    id: cloud ?? list[0] ?? null,
    fallbacks: auto ? Math.max(0, list.length - 1) : 0,
    via: null,
  };
}
