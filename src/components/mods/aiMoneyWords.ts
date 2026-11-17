// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Every user-facing sentence of the money prompt (the first-use question for `ai`), in one place, so
// the wording is a one-place change. The facts they are built from (the route, the Background
// models, whether it can cost money, the suggested limit) come from the backend
// (`mod_ai_prompt_info`), never from the mod. Every model name is the person's own setting and is
// drawn as plain text by the caller.
//
// The model sentence names the WHOLE Background list, local models included, rather than the one
// model the next call would try first, so it stays true if the person changes models later.

import type { AiPromptInfo } from "../../lib/modsTypes";
import { MAX_AI_CAP_USD } from "../../lib/modsTypes";

/** Where the person sets the Background models. */
const WHERE = "Settings → AI & Models";

/** "A", "A and B", "A, B and C". */
function list(items: string[]): string {
  if (items.length < 2) return items.join("");
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

export const MONEY_PROMPT_WORDS = {
  /** What the prompt says while it asks PM about the route. */
  checking: () => "Checking which AI model this would use…",

  /** The route could not be read; Allow stays off. */
  failed: () => "PM couldn't check which AI model this would use, so it can't ask about money.",

  intro: () => "This mod wants to ask an AI model to do things for it.",

  /** Which models the mod's request may go to: the whole Background list, by the facts. */
  models: (info: AiPromptInfo): string => {
    const cloud = info.cloudModels;
    const local = info.localModel;
    const names = [...cloud, ...(local ? [`your local model ${local}`] : [])];
    if (info.route === "none" || names.length === 0) {
      return `No Background model is set up yet, so its requests fail until you choose one in ${WHERE}.`;
    }
    const many = names.length > 1;
    const head = many
      ? `It uses your Background models (${WHERE})${local ? ", local ones included" : ""}: ${list(names)}.`
      : `It uses your Background model (${WHERE}): ${names[0]}.`;
    const change = many
      ? "If you change them, it uses the new ones."
      : "If you change it, it uses the new one.";
    // What applies now, when it differs from the list.
    const now =
      info.route === "local-then-cloud"
        ? " It tries your local model first."
        : info.route === "cloud-for-power"
          ? " You are on battery, so right now it uses only the cloud ones."
          : "";
    return `${head}${now} ${change}`;
  },

  /** Where the mod's text goes. */
  sends: () =>
    "PM sends the text the mod writes to that model and nothing else of yours, and sends it only to providers that keep nothing.",

  /** What it costs, by the facts. */
  cost: (info: AiPromptInfo): string => {
    if (info.unpriced)
      return "PM can't tell what this costs, so the mod's requests will be refused.";
    return info.paid ? "It can cost real money." : "A local model costs nothing.";
  },

  /** The standing line of `ai` for a mod that needs no first-use question (first party): the
   *  table lets it ask, and the money question comes when a request would cost money. */
  standingAuto: () =>
    "It can use a local model for free. Before it first costs real money, PM asks you for a monthly limit.",

  capLabel: () => "Monthly limit",
  capHelp: () => "You can change it any time in the Mods tab.",
  capInvalid: () =>
    `Type an amount from 0 to ${MAX_AI_CAP_USD.toLocaleString("en-US")}, like 1.00.`,
} as const;
