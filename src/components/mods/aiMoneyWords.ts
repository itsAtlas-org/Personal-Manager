// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Every user-facing sentence of the money prompt (the first-use question for `ai`, plan Q8a), in one
// place, because the final wording is Bobby's and is a one-place change.
//
// WORDING PENDING: BOBBY. The sentences below are placeholders: the draft he proposed in the plan
// (Q8a), split by the facts the prompt knows. While `WORDING_PENDING` is true `say()` puts a visible
// "[wording pending: Bobby]" in front of each, so nothing built on them can pass for finished
// copy. To finish: replace the texts, set `WORDING_PENDING` to false, and update `aiMoneyWords.test.ts`
// (its marker test names this flag). No other file holds a sentence of the prompt; the facts it is
// built from (the route, the model, whether it can cost money, the suggested limit) come from the
// backend (`mod_ai_prompt_info`), never from the mod.

import type { AiPromptInfo } from "../../lib/modsTypes";
import { MAX_AI_CAP_USD } from "../../lib/modsTypes";

export const WORDING_PENDING = true;

export const PENDING_MARK = "[wording pending: Bobby]";

/** A sentence of the prompt, marked while the wording is pending. */
export function say(text: string): string {
  return WORDING_PENDING ? `${PENDING_MARK} ${text}` : text;
}

/** The model's name as the person set it, or nothing when there is none: plain text, never markup. */
function modelNote(model: string | null): string {
  return model ? ` (${model})` : "";
}

export const MONEY_PROMPT_WORDS = {
  /** What the prompt says while it asks PM about the route. */
  checking: () => say("Checking which AI model this would use…"),

  /** The route could not be read; Allow stays off. */
  failed: () =>
    say("PM couldn't check which AI model this would use, so it can't ask about money."),

  intro: () => say("This mod wants to ask an AI model to do things for it."),

  /** Which model the mod's request would go to. */
  route: (info: AiPromptInfo): string => {
    switch (info.route) {
      case "cloud":
        return say(`This uses your OpenRouter model${modelNote(info.model)}.`);
      case "local":
        return say(`This uses your local model${modelNote(info.model)}.`);
      case "local-then-cloud":
        return say(
          `This uses your local model${modelNote(info.model)}, and falls back to the cloud if it can't answer.`,
        );
      case "cloud-for-power":
        return say(`On battery, this uses the cloud${modelNote(info.model)}.`);
      default:
        return say("No AI model is set up, so its requests will fail until you pick one.");
    }
  },

  /** Where the mod's text goes. */
  sends: () =>
    say(
      "PM sends the text the mod writes to that model and nothing else of yours, and sends it only to providers that keep nothing.",
    ),

  /** What it costs, by the facts. */
  cost: (info: AiPromptInfo): string => {
    if (info.unpriced)
      return say("PM can't tell what this costs, so the mod's requests will be refused.");
    return info.paid ? say("It can cost real money.") : say("A local model costs nothing.");
  },

  /** The standing line of `ai` for a mod that needs no first-use question (first party): the
   *  table lets it ask, and the money question comes when a request would cost money. */
  standingAuto: () =>
    say(
      "Free on a model on your computer. PM asks for a monthly limit before it first costs money.",
    ),

  capLabel: () => say("Monthly limit (US dollars)"),
  capHelp: () =>
    say("You can change it any time in the Mods tab. 0 allows the AI but no spending."),
  capInvalid: () =>
    say(`Type an amount from 0 to ${MAX_AI_CAP_USD.toLocaleString("en-US")}, like 1.00.`),
} as const;
