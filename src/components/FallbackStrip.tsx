// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ChatFallback, FallbackStep } from "../lib/types";
import { shortModel } from "../lib/format";
import { Callout, IconButton } from "./ui";

/**
 * Map the backend fallback slug (`ChatEvent::Fallback.reason`, e.g. `hard_failure:timeout` /
 * `cooldown`) to a short, plain-voiced "why" clause. Exported for unit testing and so the copy lives
 * in one place. An unknown slug degrades to a generic clause rather than leaking the raw token.
 */
export function fallbackCopy(reason: string): string {
  if (reason === "cooldown") return "your local model was resting after repeated errors";
  if (reason === "power_policy") return "PM switched to the cloud to save power";
  // Not a failure: the endpoint's address now resolves to a public host over http, so PM refused to
  // send the chat in the clear. Distinct copy so it doesn't read as "your server is broken".
  if (reason === "endpoint_refused")
    return "your local endpoint's address now points somewhere PM won't send chats in the clear";
  if (reason.startsWith("hard_failure:")) {
    switch (reason.slice("hard_failure:".length)) {
      case "timeout":
        return "your local model timed out";
      case "refused":
        return "your local model wasn't reachable";
      case "model_loading":
        return "your local model was still loading";
      case "malformed_stream":
      case "degenerate_stream":
        return "your local model's reply was unusable";
      case "reply_too_large":
        return "your local model's reply was too large";
      // Thinking that ran past its time, size or loop bound, or ended with no answer. Only an answer
      // token rules the cloud out, so a turn can think locally and still fall back.
      case "unfinished_thought":
        return "your local model thought without answering";
      default:
        return "your local model couldn't answer";
    }
  }
  return "your local model couldn't answer";
}

/** A row's part in the sentence. The first reads in full, a later one is shortened ("Gemini couldn't
 *  (busy)") so several join into one line. A row PM held back is "is paused where you are"; one it
 *  passed over for another reason says nothing was sent to it. The `why` is Rust's own wording. */
function stepCopy(step: FallbackStep, first: boolean): string {
  const label = shortModel(step.label);
  switch (step.how) {
    case "failed":
      return first ? `${label} couldn't answer (${step.why})` : `${label} couldn't (${step.why})`;
    case "paused":
      return `${label} is ${step.why}`;
    case "skipped":
      return `${label} was skipped (${step.why})`;
  }
}

/** "a", "a and b", "a, b and c". */
function joinParts(parts: string[]): string {
  if (parts.length <= 1) return parts.join("");
  return `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}`;
}

/**
 * The strip for a reply a LATER row of the person's own list answered: "Claude Opus 5.5 couldn't
 * answer (no credit left or at a spend limit), so Gemini 4 Carbon did. This reply went to Google,
 * not Anthropic." Several rows join ("A couldn't answer (x) and B couldn't (y), so C did."); a row
 * that was only held back reads "B is paused where you are, so C answered.". The company sentence is
 * left out when the same company answered, because nothing then went somewhere new. Exported so the
 * copy is pinned in one place.
 */
export function listFallbackCopy(fallback: ChatFallback): string {
  const { steps, answered } = fallback;
  if (!answered || steps.length === 0) return "A later model in your list answered this reply.";
  const said = joinParts(steps.map((s, i) => stepCopy(s, i === 0)));
  const verb = steps[steps.length - 1].how === "failed" ? "did" : "answered";
  const head = `${said}, so ${shortModel(answered.label)} ${verb}.`;
  // Every company the list passed over, not only the first row's: with [Claude, Gemini, OpenRouter]
  // and the last answering, the person is told the reply went to OpenRouter, not Anthropic or Google.
  const others = [...new Set(steps.map((s) => s.company))].filter((c) => c !== answered.via);
  const moved =
    others.length > 0
      ? ` This reply went to ${answered.via}, not ${others.length > 1 ? `${others.slice(0, -1).join(", ")} or ${others[others.length - 1]}` : others[0]}.`
      : "";
  return head.charAt(0).toUpperCase() + head.slice(1) + moved;
}

/**
 * The dismissible chat honesty strip (#297): shown when a turn fell back from the user's preferred
 * local endpoint to the cloud, or when a later row of the person's own list answered because the
 * ones above it could not. A fell-back reply is REAL (not an error), so this is a caution
 * (`--st-look`), not the red error banner. Sits beside the error banner in both chat hosts.
 */
export function FallbackStrip({
  fallback,
  onDismiss,
}: {
  fallback: ChatFallback;
  onDismiss: () => void;
}) {
  return (
    <Callout tone="warning" variant="strip" size="md" live className="flex items-center gap-2">
      <span className="min-w-0 flex-1">
        {fallback.kind === "list" ? (
          listFallbackCopy(fallback)
        ) : (
          <>
            This reply came from the cloud — {fallbackCopy(fallback.reason)}
            {fallback.to_model ? ` (via ${shortModel(fallback.to_model)})` : ""}.
          </>
        )}
      </span>
      <IconButton label="Dismiss" variant="subtle" onClick={onDismiss} className="shrink-0">
        ✕
      </IconButton>
    </Callout>
  );
}
