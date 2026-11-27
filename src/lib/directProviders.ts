// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Everything the UI says or decides about a provider the person reaches with their own key, kept
// pure so it is tested rather than seen. Anything provider-specific in the COPY lives in the one
// descriptor row below, so Gemini and Anthropic add a row (and a catalogue section in Rust), not a
// component. What a request does, where it goes and what a move is stays in Rust: this file only
// words what the backend's own answers say (`direct_providers_state`, the key check, a `RoleMove`).

import { pausedLine } from "./dailyLimit";
import type {
  CloudProviderId,
  DirectModelView,
  DirectProvidersState,
  DirectProviderView,
  KeyCheckResult,
  KeyOutcome,
  KeyTier,
  LimitView,
  ModelInfo,
  RegionRefusal,
  RoleMove,
  RoleName,
  RouteTerms,
  Settings,
  StoredKeyCheck,
} from "./types";

/** The providers that have a card in this version. */
export type DirectProviderId = "openai" | "gemini" | "anthropic";

/** One state's unfolded warning: a headline and short points. `danger` is for terms where Google may
 *  train on what is sent; `warning` is for the rest. */
export interface TermsCopy {
  tone: "warning" | "danger";
  headline: string;
  points: string[];
}

/** What a provider with a free tier and a paid one says, by verdict. Absent for a provider with no
 *  tiers (OpenAI), which is how the UI knows to draw no badge and no verdict-shaped warning. */
export interface TierCopy {
  paid: TermsCopy;
  free: TermsCopy;
  /** The key checked out but PM couldn't tell the tier. Reads as free. */
  unknown: TermsCopy;
  /** A key is here and there is no usable verdict for it (never checked, or a verdict carried from
   *  another key). Reads as free, and says nothing was asked. */
  unchecked: TermsCopy;
  /** The short name used in "Gemini is paused". */
  shortName: string;
  /** The chip and the footer label, by the terms of the route. */
  chip: { paid: string; free: string; blocked: string };
}

export interface DirectProviderInfo {
  id: DirectProviderId;
  name: string;
  /** Who answers, and who to blame: "OpenAI", "Google". */
  company: string;
  /** What its models are called in a sentence: "OpenAI", "Gemini". */
  modelsLabel: string;
  toggleLabel: string;
  /** The line under the toggle while it is off, and when it is off with a key kept. */
  offLine: string;
  offWithKeyLine: string;
  keyLabel: string;
  keyHint: string;
  keyDescription: string;
  keyUrl: string;
  keyLinkText: string;
  /** The `data-help` id on the key field. */
  helpId: string;
  /** The unfolded warning before there is a verdict to word, and the whole warning for a provider
   *  with no tiers. */
  headline: string;
  sharedHeadline?: string;
  points: string[];
  /** The folded SectionInfo, paragraph by paragraph. */
  detailsTitle: string;
  details: string[];
  /** The tick-box a provider with a data-sharing programme has. Absent for Gemini: its tier is
   *  detected, never asserted. */
  sharesDataOption?: { label: string; note: string };
  /** Only a provider with tiers has this. */
  tiers?: TierCopy;
  /** What a refused key's sentence adds after "says this key isn't right.". */
  rejectedHint: string;
  /** What a key that can list models but not chat needs, finishing "If a chat fails, ...". */
  restrictedHint: string;
  /** What a key with no access at all must be given, in "Give it access to ___ in your X project":
   *  OpenAI and Gemini keep their "the models and chat endpoints" wording from before Anthropic. */
  accessWhat: string;
  /** Whether the effective model's note is a warning worth a standing line, a link from the picker
   *  and a spoken announcement (Anthropic's Fable retention). Gemini's notes are catalogue facts
   *  (price tier, preview status), shown only inside the open picker as before. */
  warnsOnModel: boolean;
  /** What this provider calls the thing a key belongs to: "project" (OpenAI), "workspace"
   *  (Anthropic). Only the restricted-key sentences use it. */
  workspaceWord: string;
  /** Where to add credit or raise a spend limit, for the shared no-credit sentence. */
  billingPlace: string;
  /** A key that is valid but whose balance is empty: kept, never turned away. */
  noCredit: {
    saved: string;
    role: (who: string) => string;
    /** Whether such a key can still be picked for a role. OpenAI's can (the role's own line says it
     *  won't answer yet, #1149); a prepaid Gemini key can't, since every call would 402. */
    offered: boolean;
  };
  /** What the composer chip and the sidebar row say. */
  chipLabel: string;
  chipSharedLabel?: string;
  /** What the On battery question says Google or OpenAI does with a request (3.4c). */
  retention: { paid: string; free: string };
}

/** The no-credit sentences OpenAI and Anthropic share (#1149's wording). Spend limits arrive as
 *  no credit from both, so one sentence covers both; only the name and the billing place differ.
 *  Gemini's prepaid balance reads differently and keeps its own. */
function sharedNoCredit(name: string, billingPlace: string): DirectProviderInfo["noCredit"] {
  return {
    saved: `Saved. Your ${name} account is out of credit or at a spend limit, so chats won't work until you add credit or raise the limit at ${billingPlace}. Then press Check again.`,
    role: () =>
      `Your ${name} account is out of credit or at a spend limit, so this won't answer until you add credit or raise the limit.`,
    offered: true,
  };
}

const GEMINI_REGION_POINT =
  "In the UK, the EU, Iceland, Liechtenstein, Norway and Switzerland, PM only uses paid keys. PM works out where you are from your computer's time zone.";

export const DIRECT_PROVIDERS: Record<DirectProviderId, DirectProviderInfo> = {
  openai: {
    id: "openai",
    name: "OpenAI",
    company: "OpenAI",
    modelsLabel: "OpenAI",
    toggleLabel: "Use my own OpenAI key",
    offLine: "Off. PM uses OpenRouter, which asks for zero data retention on every request.",
    offWithKeyLine: "Off. Your key is saved but PM isn't using it.",
    keyLabel: "OpenAI API key",
    keyHint: "sk-...",
    keyDescription:
      "Paste a key from your OpenAI project. PM keeps it in your computer's keychain.",
    keyUrl: "https://platform.openai.com/api-keys",
    keyLinkText: "Get a key at platform.openai.com/api-keys →",
    helpId: "settings-openai-key",
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
      'Checking a key sends OpenAI one tiny test message ("Reply with OK."), which costs a tiny fraction of a cent and isn\'t listed in Usage.',
      "These are OpenAI's terms when this version of PM was built. They can change. OpenAI's data page is the one to trust.",
    ],
    sharesDataOption: {
      label: "My OpenAI project shares data to get free tokens",
      note: "PM can't see this setting, so tell it. If you tick it, PM warns you that OpenAI may train on what you send.",
    },
    rejectedHint:
      "Check you copied the whole key from the right project. If it's an Admin key, make a normal project key instead.",
    restrictedHint: "give the key access to the models and chat endpoints in your OpenAI project",
    accessWhat: "the models and chat endpoints",
    warnsOnModel: false,
    workspaceWord: "project",
    billingPlace: "platform.openai.com",
    noCredit: sharedNoCredit("OpenAI", "platform.openai.com"),
    chipLabel: "OpenAI direct",
    chipSharedLabel: "OpenAI direct, shared",
    retention: {
      paid: "OpenAI keeps requests for up to 30 days.",
      free: "OpenAI keeps requests for up to 30 days.",
    },
  },
  gemini: {
    id: "gemini",
    name: "Google Gemini",
    company: "Google",
    modelsLabel: "Gemini",
    toggleLabel: "Use my own Google Gemini key",
    offLine: "Off. PM uses OpenRouter, which asks for zero data retention on every request.",
    offWithKeyLine: "Off. Your key is saved but PM isn't using it.",
    keyLabel: "Gemini API key",
    keyHint: "Your Gemini API key",
    keyDescription: "Paste a key from Google AI Studio. PM keeps it in your computer's keychain.",
    keyUrl: "https://aistudio.google.com/api-keys",
    keyLinkText: "Get a key at aistudio.google.com/api-keys →",
    helpId: "settings-gemini-key",
    headline: "This isn't zero data retention.",
    points: [
      "A free Gemini key lets Google train on what you send, and lets people at Google read it.",
      "A paid key doesn't, but Google still logs what you send for a limited time to look for abuse.",
      GEMINI_REGION_POINT,
    ],
    detailsTitle: "What Google keeps",
    details: [
      "Google treats the Gemini API as a paid service when your key's project has billing linked and prepay credit. Anything else is unpaid.",
      "On the free tier, Google uses what you send, and the replies, to provide, improve and develop its products. Human reviewers may read, annotate and process it. Google says not to send sensitive, confidential or personal information on it.",
      "On a paid key, Google doesn't use your prompts or replies to improve its products. It logs them for a limited time, only to find and stop abuse and to meet legal requests. It doesn't say how long.",
      "Google's terms only allow paid Gemini services for apps used in the European Economic Area, Switzerland and the United Kingdom. Google's servers don't check this, so PM does: it reads your computer's time zone and doesn't use a free key there. It is a guard against mistakes, not a lock.",
      "Google may cache the start of a long prompt that repeats. PM can't turn that off. PM never uses Google's grounding, files or batch tools.",
      "Checking a key sends Google one tiny test message to a model only paid keys can use. On a paid key that costs a tiny fraction of a cent, and it isn't listed in Usage.",
    ],
    tiers: {
      paid: {
        tone: "warning",
        headline: "This isn't zero data retention.",
        points: [
          "Your key is on Google's paid terms, so Google doesn't use what you send to improve its products.",
          "Google still logs your requests and replies for a limited time, only to look for abuse. It doesn't say how long.",
          "Only Google's Vertex AI can promise zero data retention, and PM doesn't use it.",
        ],
      },
      free: {
        tone: "danger",
        headline: "Google may train on what you send, and people at Google may read it.",
        points: [
          "This key is on Google's free tier. Google uses what you send, and what comes back, to improve its products. Human reviewers may read it.",
          "PM sends parts of your documents and your chats. Don't use a free key for anything private.",
          "Free keys also have small daily limits, so background work may stop for the day.",
          "To get Google's paid terms, link billing and add prepay credit to the key's project in AI Studio, then press Check again.",
        ],
      },
      unknown: {
        tone: "danger",
        headline: "Google may train on what you send, and people at Google may read it.",
        points: [
          "PM couldn't tell whether this key is free or paid, so it treats it as free. If it is free, Google uses what you send to improve its products and people at Google may read it.",
          "PM sends parts of your documents and your chats. Don't use a free key for anything private.",
          "Free keys also have small daily limits, so background work may stop for the day.",
          "To get Google's paid terms, link billing and add prepay credit to the key's project in AI Studio, then press Check again.",
        ],
      },
      unchecked: {
        tone: "danger",
        headline: "PM hasn't checked this key, so it treats it as free.",
        points: [
          "A free Gemini key lets Google train on what you send, and lets people at Google read it. PM can't tell which this is until it checks.",
          "Press Check again. If the key is paid, this warning goes away.",
          GEMINI_REGION_POINT,
        ],
      },
      shortName: "Gemini",
      chip: { paid: "Gemini direct, paid", free: "Gemini direct, free", blocked: "Gemini paused" },
    },
    rejectedHint:
      "Check you copied the whole key from AI Studio. Google also blocks leaked keys, old keys with no restrictions, and keys limited to other Google services. A fresh key from AI Studio fixes all three.",
    restrictedHint:
      "give the key access to the models and chat endpoints in your Google Gemini project",
    accessWhat: "the models and chat endpoints",
    warnsOnModel: false,
    workspaceWord: "project",
    billingPlace: "aistudio.google.com/billing",
    noCredit: {
      saved:
        "Saved. This key is paid, but its prepay balance is empty, so Google has stopped it. Add credit at aistudio.google.com/billing, then press Check again.",
      role: (who) =>
        `${who} is set to Google Gemini, but its prepay balance is empty. ${who} won't answer until you add credit in AI Studio, or you pick another provider.`,
      offered: false,
    },
    chipLabel: "Gemini direct",
    retention: {
      paid: "Google Gemini logs requests for a limited time to look for abuse, and doesn't use them to improve its products.",
      free: "Google Gemini's free tier may train on requests, and people at Google may read them.",
    },
  },
  anthropic: {
    id: "anthropic",
    name: "Anthropic",
    company: "Anthropic",
    modelsLabel: "Anthropic",
    toggleLabel: "Use my own Anthropic key",
    offLine: "Off. PM uses OpenRouter, which asks for zero data retention on every request.",
    offWithKeyLine: "Off. Your key is saved but PM isn't using it.",
    keyLabel: "Anthropic API key",
    keyHint: "sk-ant-...",
    keyDescription:
      "Paste an API key from your Anthropic Console workspace. PM keeps it in your computer's keychain.",
    keyUrl: "https://platform.claude.com/settings/keys",
    keyLinkText: "Get a key at platform.claude.com →",
    helpId: "settings-anthropic-key",
    headline: "This isn't zero data retention.",
    points: [
      "Anthropic doesn't train on what you send through its API without your permission.",
      "Anthropic can keep what you send for up to 30 days, and for up to 2 years if it flags a request for breaking its usage policy. Zero retention only comes from Anthropic, by agreement with them.",
      "Claude Fable always needs 30-day retention: Anthropic won't run it on a workspace set to zero retention.",
    ],
    detailsTitle: "What Anthropic keeps",
    details: [
      "Anthropic's Commercial Terms say it may not train its models on what you send through the API, and its API documentation says retained data is never used for training without your express permission.",
      "Anthropic deletes the inputs and outputs on its systems within 30 days. If a request is flagged for breaking Anthropic's usage policy, it can keep that content for up to 2 years. You can't change either yourself.",
      "Zero data retention only exists by agreement with Anthropic. Ask Anthropic if you need it.",
      "Claude Fable 5.2 is a model Anthropic only runs with 30-day retention switched on. If your workspace is set to zero retention, Fable fails and the other models carry on working. In the Anthropic Console, put your key in a workspace with Standard data retention.",
      "PM keeps the part of a request that doesn't change in Anthropic's prompt cache for five minutes, so repeat requests cost less. Anthropic says it holds cached prompts in memory for that time.",
      'Checking a key sends Anthropic one tiny test message ("Reply with OK."), which costs a tiny fraction of a cent and isn\'t listed in Usage.',
      "These are Anthropic's terms when this version of PM was built. They can change. Anthropic's privacy centre is the one to trust.",
    ],
    rejectedHint:
      "Check you copied the whole key from your Anthropic Console workspace (Anthropic keys start with sk-ant-). If it's an Admin key, make a normal API key instead.",
    restrictedHint: "give the key access to the models in your Anthropic workspace",
    accessWhat: "the models and chat",
    warnsOnModel: true,
    workspaceWord: "workspace",
    billingPlace: "platform.claude.com",
    noCredit: sharedNoCredit("Anthropic", "platform.claude.com"),
    chipLabel: "Anthropic direct",
    retention: {
      paid: "Anthropic keeps requests for up to 30 days, or up to 2 years if it flags one for breaking its usage policy.",
      free: "Anthropic keeps requests for up to 30 days, or up to 2 years if it flags one for breaking its usage policy.",
    },
  },
};

/** "OpenAI", "OpenAI or Anthropic", "OpenAI, Anthropic or Google Gemini". */
export function orList(names: string[]): string {
  if (names.length <= 1) return names[0] ?? "";
  return `${names.slice(0, -1).join(", ")} or ${names[names.length - 1]}`;
}

/** The provider names in card order (the order Rust lists them), for sentences that name them all. */
export function directNames(): string[] {
  return Object.values(DIRECT_PROVIDERS).map((i) => i.name);
}

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
  // The model's own default is a real level (Gemini's and Anthropic's), so say which one the person
  // gets by leaving it alone. OpenAI's chat option keeps the wording it has always had.
  const modelDefault =
    model.effort_default && !model.id.startsWith("openai/")
      ? `Model default (${effortLabel(model.effort_default)})`
      : "Model default";
  return role === "chat" ? [{ value: "", label: modelDefault }, ...levels] : levels;
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

/** Whether the last check saw the key's model list: it did for a working key, a key with no credit
 *  (it lists fine, it just can't chat yet) and a read-only one. */
function listingKnown(p: DirectProviderView): boolean {
  const c = p.key_check;
  if (!c) return false;
  if (c.outcome === "ok") return true;
  // A listing refused outright (a restricted key tried through to a no-credit answer) shows none.
  return (c.outcome === "no_credit" || c.note === "read_only") && c.listed_total > 0;
}

/** A provider with a free tier and a paid one (Gemini) draws a tier badge and words its warning by
 *  the verdict. OpenAI has neither. */
export function hasTiers(info: DirectProviderInfo | null | undefined): boolean {
  return !!info?.tiers;
}

/** Whether a tiered provider's key reads as free: free, unknown, and no verdict at all, which is
 *  Unknown (Bobby, 2026-10-22). The UI never decides what that blocks; Rust read the region. */
export function readsFree(p: DirectProviderView): boolean {
  return hasTiers(directInfo(p.provider)) && p.tier !== "paid";
}

const MYTHOS = /mythos/i;

/** The catalogue as `ModelInfo` rows. Prices are per MILLION on the wire and per TOKEN in
 *  `ModelInfo`, which is what the picker multiplies back. */
export function directPickerRows(p: DirectProviderView): DirectPickerRow[] {
  const listed = listingKnown(p) ? (p.key_check?.listed ?? null) : null;
  const free = readsFree(p);
  return (
    p.models
      // Mythos is never offered (Bobby, 2026-11-26). Rust refuses it in three layers and the bundled
      // catalogue has no row for it; this is the last wall, so even a row that got through is not drawn.
      .filter((m) => !MYTHOS.test(`${m.id} ${m.name}`))
      .map((m) => {
        // A model only a paid key can use, shown against a key that reads as free. Still selectable: the
        // verdict may be stale, and Google's own answer says it plainly.
        const needsPaid = free && m.free_tier === false;
        const note = [m.note, needsPaid ? "Needs a paid key" : null].filter(Boolean).join(". ");
        const cached =
          m.price_cached_in != null
            ? `${m.name}: cached input ${perM(m.price_cached_in)} per million tokens.`
            : `${m.name}.`;
        const next = m.next_price
          ? ` Rises to ${perM(m.next_price.price_in)} in and ${perM(m.next_price.price_out)} out per million tokens on ${dayMonthYear(m.next_price.from)}.`
          : "";
        return {
          id: m.id,
          name: m.name,
          description: "",
          context_length: m.context,
          prompt_price: m.price_in / 1_000_000,
          completion_price: m.price_out / 1_000_000,
          input_modalities: m.input_modalities,
          note: note || null,
          unlisted: listed !== null && !listed.includes(m.id),
          retires: m.retires_on ? dayMonthYear(m.retires_on) : null,
          tip: `${cached}${next} PM's list, checked on ${dayMonthYear(m.checked_on)}.`,
        };
      })
  );
}

/** The picker's caption: what the last check can say about the list. */
export function pickerCaption(p: DirectProviderView): string {
  return listingKnown(p)
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

/** The sentence a test chat that couldn't be tried adds to the listing's verdict. */
const CHAT_UNTRIED = "PM couldn't try a chat just now.";

/** Whether a verdict is one the person can act on and press Check again for: no credit yet, a key
 *  that can't chat, or a check that couldn't try a chat. A plain "works" has nothing to recheck. */
export function offersRecheck(
  outcome: KeyOutcome | undefined,
  note?: KeyCheckResult["note"],
): boolean {
  return outcome === "no_credit" || outcome === "restricted" || note === "chat_untried";
}

/** Whether a SAVED key shows Check again. Beyond the verdicts above, a key on a provider with tiers
 *  offers it unless it is known paid: a free or unchecked key may have been upgraded since, and a
 *  paused one is only unpaused by asking again. `tier` and `blocked` are the card's current view of
 *  the key (this session's result or the state's). */
export function offersRecheckFor(
  info: DirectProviderInfo,
  current: { outcome?: KeyOutcome; note?: KeyCheckResult["note"] } | null,
  tier: KeyTier | null | undefined,
  blocked: RegionRefusal | null | undefined,
): boolean {
  if (offersRecheck(current?.outcome, current?.note)) return true;
  if (current?.outcome === "needs_verification") return true;
  return hasTiers(info) && (tier !== "paid" || !!blocked);
}

/** The answers `recheck_direct_key` stores as the key's verdict (the backend's "says something"
 *  list). Anything else (unreachable, an unexpected status) leaves the stored verdict standing, so
 *  the card must keep drawing that one. */
export function recheckStores(outcome: KeyOutcome): boolean {
  return (
    outcome === "ok" ||
    outcome === "restricted" ||
    outcome === "no_credit" ||
    outcome === "rejected" ||
    outcome === "needs_verification" ||
    outcome === "region_blocked"
  );
}

// ---- The words about where PM thinks this computer is --------------------------------------------

/** "PM thinks you're in the United Kingdom", or, for a zone it can't place, what it can't do. */
function whereClause(r: RegionRefusal): string {
  return r.kind === "unplaceable"
    ? "PM can't tell where this computer is"
    : `PM thinks you're in ${r.label}`;
}

/** "(it read your computer's time zone as Europe/London)", or the unplaceable form. */
function zoneClause(r: RegionRefusal): string {
  return r.kind === "unplaceable"
    ? `(its time zone reads as ${r.zone || "unknown"})`
    : `(it read your computer's time zone as ${r.zone})`;
}

/** The first clause of a paused sentence: what PM holds against the key. A key that was never
 *  checked and one the check couldn't place are different things to say. */
function keyClause(tier: KeyTier | null | undefined, r: RegionRefusal): string {
  if (r.basis === "free" && tier !== "unknown") return "Your key reads as free";
  return tier === "unknown"
    ? "PM couldn't tell whether your key is free or paid, so it treats it as free"
    : "PM hasn't checked your key, so it treats it as free";
}

/** The paused callout on a card (3.4). `flipped` is `flipped_free_at`, set when Google itself said so. */
export function pausedText(
  info: DirectProviderInfo,
  r: RegionRefusal,
  tier: KeyTier | null | undefined,
  flipped?: string | null,
): string {
  const short = info.tiers?.shortName ?? info.name;
  const lead = flipped
    ? `Google told PM on ${dayMonthYear(flipped)} that this project is on the free tier. `
    : "";
  const head = `${short} is paused. ${keyClause(tier, r)}, and ${whereClause(r)} ${zoneClause(r)}`;
  return r.kind === "unplaceable"
    ? `${lead}${head}, so it only uses paid keys. Set your computer's time zone, or make the project paid in AI Studio, then press Check again.`
    : `${lead}${head}. Google's terms only allow paid keys there, so PM won't use it. Make the project paid in AI Studio, then press Check again.`;
}

/** The refusal callout's body for a key that was typed and not kept (3.3). */
export function refusalText(r: RegionRefusal): string {
  const lead =
    r.basis === "unknown"
      ? "PM couldn't tell whether it's free or paid, so it treats it as free"
      : "It's a free key";
  if (r.kind === "unplaceable") {
    return `${lead}, and PM can't tell where this computer is (its time zone reads as ${r.zone || "unknown"}), so it only uses paid Gemini keys. Set your computer's time zone to where you are, or make the project paid in AI Studio, then press Check again.`;
  }
  return `${lead}, and PM thinks you're in ${r.label}: it read your computer's time zone as ${r.zone}. Google's terms only allow paid Gemini services there, so PM doesn't use a free key. To make it paid, link billing to the key's project in AI Studio and add prepay credit. It can take a few minutes to show. Then press Check again.`;
}

/** The extra line under the refusal, for a computer PM did place. */
export function notThereLine(r: RegionRefusal): string | null {
  return r.kind === "unplaceable"
    ? null
    : `Not in ${r.label}? Set your computer's time zone to where you really are, then press Check again.`;
}

/** The card's lead line when Google said, after a check, that the project is now free and PM is
 *  outside the places that would pause it. */
export function flippedLine(flipped: string): string {
  return `Google told PM on ${dayMonthYear(flipped)} that this project is now on the free tier, so Google may use what you send to improve its products. Make the project paid in AI Studio, then press Check again.`;
}

// ---- The unfolded warning, and the badge ----------------------------------------------------------

/** The warning a card shows, a pure function of the verdict. A provider with no tiers always shows
 *  its one warning; one with tiers words the verdict it holds for THIS key. */
export function termsFor(info: DirectProviderInfo, view: DirectProviderView): TermsCopy {
  const plain: TermsCopy = {
    tone: "warning",
    headline: view.shares_data && info.sharedHeadline ? info.sharedHeadline : info.headline,
    points: info.points,
  };
  const t = info.tiers;
  if (!t || view.key !== "present") return plain;
  if (view.tier === "paid") return t.paid;
  if (view.tier === "free") return t.free;
  if (view.tier === "unknown") return t.unknown;
  return t.unchecked;
}

export type BadgeTone = "ok" | "due";

/** The tier badge: a glyph (decorative) and real words. null when there is nothing to say: no tiers,
 *  or no key here. A paused or empty key says so in the same words. */
export function tierBadge(
  info: DirectProviderInfo,
  view: DirectProviderView,
): { glyph: string; text: string; tone: BadgeTone } | null {
  if (!info.tiers || view.key !== "present") return null;
  const base =
    view.tier === "paid"
      ? { glyph: "✓", text: "Paid key", tone: "ok" as const }
      : view.tier === "free"
        ? { glyph: "⚠", text: "Free key", tone: "due" as const }
        : view.tier === "unknown"
          ? { glyph: "⚠", text: "Free key (PM couldn't tell)", tone: "due" as const }
          : { glyph: "⚠", text: "Not checked yet (treated as free)", tone: "due" as const };
  const extra = view.blocked
    ? " (paused)"
    : view.key_check?.outcome === "no_credit"
      ? " (out of credit)"
      : "";
  return { ...base, text: `${base.text}${extra}` };
}

/** What a card knows about the key beside a verdict: which provider (for its own sentences) and the
 *  tier and refusal that came with it. All optional, so a caller with only a name still gets the
 *  words it always did. */
export interface VerdictContext {
  info?: DirectProviderInfo | null;
  tier?: KeyTier | null;
  /** The current refusal of a SAVED key (a check of it that still reads as paused). */
  refusal?: RegionRefusal | null;
}

/** The card for a provider name, when the caller didn't hand the descriptor over. */
function infoFor(name: string, ctx?: VerdictContext): DirectProviderInfo | null {
  if (ctx?.info) return ctx.info;
  return Object.values(DIRECT_PROVIDERS).find((i) => i.name === name) ?? null;
}

/** "N Gemini models are ready to use." with the singular and the empty list. */
function readyLine(n: number, label: string): string {
  return n === 1 ? `1 ${label} model is ready to use.` : `${n} ${label} models are ready to use.`;
}

/** What a check found for a key on a provider with tiers (Gemini), in words. */
function tieredOk(
  n: number,
  info: DirectProviderInfo,
  tier: KeyTier | null | undefined,
  untried: string,
): KeyVerdict {
  const label = info.modelsLabel;
  const empty = n === 0;
  if (tier === "paid") {
    const ready = empty
      ? "Key works, but none of the models PM knows are in its list. You can still pick one."
      : `Key works. It's on Google's paid terms. ${readyLine(n, label)}`;
    return { tone: untried ? "warn" : "ok", text: `${ready}${untried}` };
  }
  if (tier === "free") {
    const ready = empty
      ? "None of the models PM knows are in its list. You can still pick one."
      : `${n === 1 ? `1 ${label} model is` : `${n} ${label} models are`} ready, but models marked "Needs a paid key" won't work on it.`;
    return {
      tone: "warn",
      text: `Saved. This key is on Google's free tier, so Google may train on what you send. ${ready}${untried}`,
    };
  }
  return {
    tone: "warn",
    text: `Saved. PM couldn't tell whether this key is free or paid, so it treats it as free: Google may train on what you send.${untried}`,
  };
}

/** What a key check found, in words, for a key that was JUST typed (so a refusal is "Not saved").
 *  `listedCount` is how many catalogue models the key can see. An outcome this version has no
 *  sentence for is shown as itself and counted as not saved. */
export function keyVerdict(
  outcome: KeyOutcome,
  listedCount: number,
  name: string,
  note?: KeyCheckResult["note"],
  ctx?: VerdictContext,
): KeyVerdict {
  const info = infoFor(name, ctx);
  const company = info?.company ?? name;
  if (typeof outcome === "object" && outcome !== null && "unexpected" in outcome) {
    return {
      tone: "refused",
      text: `Not saved. ${company} answered with an error (${outcome.unexpected}). Try again shortly.`,
    };
  }
  const untried = note === "chat_untried" ? ` ${CHAT_UNTRIED}` : "";
  // "project" or "workspace", and what to ask for: each provider's own word, never another's.
  const where = info?.workspaceWord ?? "project";
  const hint =
    info?.restrictedHint ??
    `give the key access to the models and chat endpoints in your ${name} ${where}`;
  const accessWhat = info?.accessWhat ?? "the models and chat endpoints";
  switch (outcome) {
    case "ok": {
      if (info && hasTiers(info)) return tieredOk(listedCount, info, ctx?.tier, untried);
      const ready =
        listedCount === 0
          ? `Key works, but none of the models PM knows are in its list. You can still pick one.`
          : listedCount === 1
            ? `Key works. 1 ${name} model is ready to use.`
            : `Key works. ${listedCount} ${name} models are ready to use.`;
      // A caveat is not a tick: "couldn't try a chat" is the very thing Check again is for.
      return { tone: untried ? "warn" : "ok", text: `${ready}${untried}` };
    }
    case "needs_verification":
      return {
        tone: "refused",
        text: `Not saved. ${company} wants this project verified in AI Studio before the key can be used. Verify it, then press Save and check again.`,
      };
    case "busy":
      return {
        tone: "refused",
        text: `${company} was busy or limiting requests, so PM couldn't check the key. Nothing was decided. Wait a minute and press Save and check again.`,
      };
    case "free_key_refused":
      return {
        tone: "refused",
        text: "Not saved. It's a free key and PM only uses paid keys where you are.",
      };
    case "restricted":
      return {
        tone: "warn",
        text:
          note === "read_only"
            ? `Saved. This key can list models but can't chat. Give it access to chat in your ${name} ${where}, then press Check again.`
            : note === "no_access"
              ? `Saved. This key can't list models or chat. Give it access to ${accessWhat} in your ${name} ${where}, then press Check again.`
              : `Saved. This key is restricted, so PM can't see which models it can use. If a chat fails, ${hint}.${untried}`,
      };
    case "no_credit":
      // Valid and kept: only chatting fails, and only until credit is added.
      return {
        tone: "warn",
        text:
          info?.noCredit.saved ??
          `Saved. Your ${name} account is out of credit or at a spend limit, so chats won't work until you add credit or raise the limit. Then press Check again.`,
      };
    case "rejected":
      return {
        tone: "refused",
        text: `Not saved. ${company} says this key isn't right. ${info?.rejectedHint ?? "Check you copied the whole key."}`,
      };
    case "region_blocked":
      return {
        tone: "refused",
        text: hasTiers(info)
          ? "Not saved. Google doesn't offer free Gemini keys where you are. Link billing to the key's project in AI Studio to make it paid, then press Save and check again."
          : `Not saved. ${name} doesn't serve requests from where you are right now.`,
      };
    case "malformed":
      return {
        tone: "refused",
        text: "Not saved. That doesn't look like a key. Paste the whole key, with no spaces.",
      };
    case "unreachable":
      return {
        tone: "refused",
        text: `Not saved. PM couldn't reach ${company} to check it. Check your connection and try again.`,
      };
    default:
      return { tone: "refused", text: `Not saved. The key check ended with "${String(outcome)}".` };
  }
}

/** What a check of a key that is ALREADY saved found. The same words for a key that works, is
 *  restricted or has no credit; for a refusal the key stays saved, so it never says "Not saved". */
export function recheckVerdict(
  outcome: KeyOutcome,
  listedCount: number,
  name: string,
  note?: KeyCheckResult["note"],
  ctx?: VerdictContext,
): KeyVerdict {
  const info = infoFor(name, ctx);
  const company = info?.company ?? name;
  if (typeof outcome === "object" && outcome !== null && "unexpected" in outcome) {
    return {
      tone: "refused",
      text: `${company} answered with an error (${outcome.unexpected}). Try again shortly.`,
    };
  }
  // The key is saved and still reads as free where PM only uses paid ones: the paused callout says
  // the rest, so this line is neutral and "Saved." never sits beside "paused" (S7).
  if (ctx?.refusal && (outcome === "ok" || outcome === "needs_verification")) {
    return { tone: "warn", text: "Checked: this key still reads as free." };
  }
  switch (outcome) {
    case "rejected":
      return {
        tone: "refused",
        text: `${company} now says this key isn't right. Replace it below, or remove it.`,
      };
    case "region_blocked":
      return {
        tone: "refused",
        text: hasTiers(info)
          ? "Google doesn't offer free Gemini keys where you are. Link billing to the key's project in AI Studio to make it paid, then press Check again."
          : `${name} doesn't serve requests from where you are right now.`,
      };
    case "needs_verification":
      return {
        tone: "refused",
        text: `${company} wants this project verified in AI Studio before the key can be used. Verify it, then press Check again.`,
      };
    case "busy":
      return {
        tone: "refused",
        text: `${company} was busy or limiting requests, so PM couldn't check the key. Nothing was decided. Wait a minute and press Check again.`,
      };
    case "unreachable":
      return {
        tone: "refused",
        text: `PM couldn't reach ${company} to check it. Check your connection and try again.`,
      };
    default: {
      const v = keyVerdict(outcome, listedCount, name, note, ctx);
      // The key was saved some time ago (or a moment ago by a first check, which uses keyVerdict):
      // "Saved." would be news only the first time.
      return { ...v, text: v.text.replace(/^Saved\. /, "") };
    }
  }
}

/** A stored verdict, shown only beside a key that is really there (a verdict carried in a backup is
 *  never drawn next to no key). On a provider with tiers it is also shown only when it is about THIS
 *  key (`tier` is null for a verdict carried from another one: the card then says PM hasn't checked
 *  it, and a "Key works" line about some other key would be false). */
export function storedVerdict(p: DirectProviderView, name: string): KeyVerdict | null {
  if (p.key !== "present" || !p.key_check) return null;
  const info = infoFor(name);
  if (hasTiers(info) && p.tier == null) return null;
  const c = p.key_check;
  return recheckVerdict(c.outcome, c.listed.length, name, c.note, {
    info,
    tier: p.tier,
    refusal: p.blocked,
  });
}

/** A stored verdict that says the key was turned away. Only a check that was never kept could say
 *  this, so it guards a hand-edited row and nothing more. A key with no credit is NOT turned away:
 *  it is valid, and the role's own line says it won't answer yet. */
function turnedAway(c: StoredKeyCheck | null): boolean {
  if (!c) return false;
  return c.outcome !== "ok" && c.outcome !== "restricted" && c.outcome !== "no_credit";
}

// ---- Who can be chosen, and what is wrong with a choice --------------------------------------------

/** Opted in, key there, last check not a refusal, not paused, and (for a provider whose empty key
 *  would 402 every call) not out of credit: the providers a role may be moved to. */
export function isEligible(p: DirectProviderView): boolean {
  if (!(p.opted_in && p.key === "present") || turnedAway(p.key_check) || p.blocked) return false;
  const emptyKeyRefused =
    p.key_check?.outcome === "no_credit" && directInfo(p.provider)?.noCredit.offered === false;
  return !emptyKeyRefused;
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
export type RoleIssue =
  "turned_off" | "key_missing" | "key_unreadable" | "blocked" | "limit_reached" | "no_credit";

export function roleIssue(state: DirectProvidersState, role: RoleName): RoleIssue | null {
  const r = state.roles[role];
  if (!isDirect(r.stored)) return null;
  if (r.stored !== r.effective) return "turned_off";
  const p = providerView(state, r.stored);
  if (!p) return "key_missing";
  if (p.key === "unreadable") return "key_unreadable";
  if (p.key === "absent") return "key_missing";
  // Paused beats an empty balance: the pause is the thing the person can fix first, and Rust only
  // sets `blocked` for a key that is here and a provider that is on.
  if (p.blocked) return "blocked";
  // The person's own daily limit: a pause too, and one that ends by itself at midnight.
  if (p.limit.reached) return "limit_reached";
  if (p.key_check?.outcome === "no_credit") return "no_credit";
  return null;
}

/** What a role's issue line may need beyond the issue itself: which provider (its own sentences)
 *  and, for a paused one, where PM thinks this computer is and what it holds against the key. */
export interface IssueContext {
  id?: CloudProviderId | string;
  blocked?: RegionRefusal | null;
  tier?: KeyTier | null;
  /** The provider's daily limit, for a role paused by it. */
  limit?: LimitView;
}

/** The sentence for a role's issue (the "turned off" one also carries a button, drawn by the caller). */
export function roleIssueLine(
  issue: RoleIssue,
  role: RoleName,
  providerLabel: string,
  ctx?: IssueContext,
): string {
  const who = roleName(role);
  const info = ctx?.id ? directInfo(ctx.id) : null;
  switch (issue) {
    case "turned_off":
      return `${providerLabel} is turned off above.`;
    case "key_unreadable":
      return `${who} is set to ${providerLabel}, but PM can't read your ${providerLabel} key right now. ${who} won't answer until it can, or until you pick another provider.`;
    case "key_missing":
      return `Your ${providerLabel} key is missing on this device. Add it above, or move ${who} back to OpenRouter.`;
    case "blocked": {
      const r = ctx?.blocked;
      const short = info?.tiers?.shortName ?? providerLabel;
      // "(your key reads as free and PM thinks you're in the United Kingdom)": mid-sentence, so the
      // clause's own capital goes.
      const why = r
        ? `${keyClause(ctx?.tier, r).replace(/^Your /, "your ")} and ${whereClause(r)}`
        : "your key reads as free";
      return `${who} is set to ${providerLabel}, but ${short} is paused (${why}). ${who} won't answer until the project is paid, or until you pick another provider.`;
    }
    case "limit_reached":
      return `${who} is set to ${providerLabel}. ${
        ctx?.limit
          ? pausedLine(ctx.limit, providerLabel)
          : "Paused for today by your daily limit. It starts again at 00:00 (your day), or sooner if you raise the limit."
      } PM won't send this to another company on its own.`;
    case "no_credit":
      return info
        ? info.noCredit.role(who)
        : `Your ${providerLabel} account is out of credit or at a spend limit, so this won't answer until you add credit or raise the limit.`;
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
  if (issue === "blocked") return `${name} (paused)`;
  if (issue === "limit_reached") {
    // An unreadable limit does not lift at midnight, so "for today" would promise too much.
    return providerView(state, id)?.limit.unreadable
      ? `${name} (paused)`
      : `${name} (paused for today)`;
  }
  if (issue === "no_credit" && directInfo(id)?.noCredit.offered === false) {
    return `${name} (out of credit)`;
  }
  return directInfo(id) ? name : `${name} (unavailable)`;
}

/** "OpenRouter (asks for zero data retention)" / "OpenAI, direct (no zero data retention)". */
export function providerOptionLabel(id: CloudProviderId | string): string {
  return id === "openrouter"
    ? "OpenRouter (asks for zero data retention)"
    : `${providerName(id)}, direct (no zero data retention)`;
}

/** What the wizard adopts a freshly checked key for. A paid key (or one with no tiers) takes both
 *  roles when there is no OpenRouter key; a key that reads as free takes CHAT only, so documents are
 *  never sent to be trained on without a choice (S8). */
export function adoptRoles(view: DirectProviderView | undefined): RoleName[] {
  if (!view || !isEligible(view)) return [];
  return readsFree(view) ? ["chat"] : ["chat", "background"];
}

/** The wizard's line beside the role Select when a free key took chat only. */
export const BACKGROUND_NOT_SET_UP =
  "Background work isn't set up yet. Pick where it should go, or add an OpenRouter key.";

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

/** The effective model's WARNING, for a standing line, a link from the picker and a spoken
 *  announcement. Only a provider whose notes are warnings has one (Anthropic's Fable): the notes of
 *  the other cards are catalogue facts that show inside the open picker, as they always did. */
export function modelWarning(
  provider: string,
  model: Pick<DirectModelView, "note"> | undefined,
): string | null {
  return directInfo(provider)?.warnsOnModel ? (model?.note ?? null) : null;
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
): {
  id: string | null;
  fallbacks: number;
  via: string | null;
  /** What the route says when hovered, for a provider whose terms differ by key (Gemini). null: the
   *  footer builds its own. */
  viaTitle: string | null;
  /** The route is one to look at (a free key, or paused), so it wears the due tint. */
  viaDue: boolean;
} {
  if (!s) return { id: null, fallbacks: 0, via: null, viaTitle: null, viaDue: false };
  const list = role === "chat" ? s.chat_models : s.background_models;
  const auto = role === "chat" ? s.chat_auto_switch : s.background_auto_switch;
  const provider = (role === "chat" ? s.chat_provider : s.background_provider) ?? "openrouter";
  const cloud = role === "chat" ? s.chat_cloud_model : s.background_cloud_model;
  if (isDirect(provider)) {
    const info = directInfo(provider);
    const terms = role === "chat" ? s.chat_route_terms : s.background_route_terms;
    // Rust answers `null` for a provider with tiers whose key isn't there: nothing is known about the
    // terms, so the footer says only where the route goes, never "free". (`undefined` is a Settings
    // that predates the field, which keeps reading free.)
    if (info && hasTiers(info) && terms === null) {
      return {
        id: cloud ?? null,
        fallbacks: 0,
        via: info.chipLabel,
        viaTitle: null,
        viaDue: false,
      };
    }
    return {
      id: cloud ?? null,
      fallbacks: 0,
      via: info ? routeLabel(info, terms) : `${providerName(provider)} direct`,
      viaTitle: info && hasTiers(info) ? routeTitle(info, role, terms) : null,
      viaDue: !!info && hasTiers(info) && routeTermsOf(terms) !== "paid",
    };
  }
  return {
    id: cloud ?? list[0] ?? null,
    fallbacks: auto ? Math.max(0, list.length - 1) : 0,
    via: null,
    viaTitle: null,
    viaDue: false,
  };
}

/** The terms a route is on, with a missing word read as free: Unknown reads as free everywhere. */
export function routeTermsOf(terms: RouteTerms | null | undefined): RouteTerms {
  return terms === "paid" || terms === "blocked" ? terms : "free";
}

/** The terms of a provider's route from its own view, for the chip when `Settings` has none yet. */
export function viewTerms(view: DirectProviderView): RouteTerms {
  return view.blocked ? "blocked" : view.tier === "paid" ? "paid" : "free";
}

/** The chip and footer label for a provider's route. A provider with tiers says its terms; one
 *  without says its one label (and "shared" when the person said their project shares data). */
export function routeLabel(
  info: DirectProviderInfo,
  terms: RouteTerms | null | undefined,
  shared = false,
): string {
  if (info.tiers) return info.tiers.chip[routeTermsOf(terms)];
  return shared && info.chipSharedLabel ? info.chipSharedLabel : info.chipLabel;
}

/** The hover text of a tiered route, for the footer and the chip (3.4). `where` is the paused
 *  refusal's label when the caller has it (the chip does; the footer only has Settings). */
export function routeTitle(
  info: DirectProviderInfo,
  role: RoleName,
  terms: RouteTerms | null | undefined,
  where?: RegionRefusal | null,
): string {
  const who = roleName(role);
  const name = info.name;
  switch (routeTermsOf(terms)) {
    case "paid":
      return `${who} goes straight to ${name} with your own key, on Google's paid terms. It isn't zero data retention. Change it in Settings > AI & Models.`;
    case "free":
      return `${who} goes straight to ${name} with a key PM treats as free. If it is free, Google may train on what you send and people at Google may read it. Change it in Settings > AI & Models.`;
    case "blocked":
      return `${who} is set to ${name}, but your key reads as free and ${
        where ? whereClause(where) : "PM only uses paid keys where you are"
      }. ${who} can't answer until you fix this. Open Settings > AI & Models.`;
  }
}
