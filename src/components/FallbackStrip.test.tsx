// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The chat honesty strip (#297) + its copy map. Pins that every backend fallback slug maps to plain
// text (and an unknown slug degrades rather than leaking the raw token), that the cloud model is
// named, and that Dismiss fires.

import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { FallbackStrip, fallbackCopy, listFallbackCopy } from "./FallbackStrip";
import type { ChatFallback } from "../lib/types";

/** The local-to-cloud strip's shape: no steps, no answering row. */
const LOCAL: ChatFallback = {
  kind: "local",
  from_model: "",
  to_model: "",
  reason: "",
  steps: [],
  answered: null,
};

/** One row that failed and a later one that answered. */
const LIST_ONE: ChatFallback = {
  kind: "list",
  from_model: "anthropic/claude-opus-5-5",
  to_model: "gemini/gemini-4-carbon",
  reason: "list_no_credit",
  steps: [
    {
      label: "Claude Opus 5.5",
      company: "Anthropic",
      why: "no credit left or at a spend limit",
      how: "failed",
    },
  ],
  answered: { label: "Gemini 4 Carbon", via: "Google" },
};

describe("fallbackCopy", () => {
  it("maps the known slugs to friendly reasons", () => {
    expect(fallbackCopy("cooldown")).toContain("resting");
    expect(fallbackCopy("hard_failure:timeout")).toContain("timed out");
    expect(fallbackCopy("hard_failure:refused")).toContain("reachable");
    expect(fallbackCopy("hard_failure:model_loading")).toContain("loading");
    expect(fallbackCopy("hard_failure:reply_too_large")).toContain("too large");
    expect(fallbackCopy("hard_failure:degenerate_stream")).toContain("unusable");
    // `fail_kind_slug(&LocalFailKind::UnfinishedThought)` in llm_gateway.rs: a model that thought
    // and never answered must not read as the generic "couldn't answer".
    expect(fallbackCopy("hard_failure:unfinished_thought")).toBe(
      "your local model thought without answering",
    );
  });

  // The other half of `FallbackReason::local_why` (llm_gateway.rs): the seeded Local step of a list's
  // strip carries these exact words, so a Local row's step reads like the local strip, never "it
  // failed". Rust pins the same sentences in `a_local_heads_tail_is_walked_and_tagged_with_why_local_failed`.
  it("has the sentences the list strip's seeded Local step carries", () => {
    expect(fallbackCopy("cooldown")).toBe("your local model was resting after repeated errors");
    expect(fallbackCopy("hard_failure:timeout")).toBe("your local model timed out");
    expect(fallbackCopy("hard_failure:refused")).toBe("your local model wasn't reachable");
    expect(fallbackCopy("hard_failure:model_loading")).toBe("your local model was still loading");
    expect(fallbackCopy("hard_failure:server_error")).toBe("your local model couldn't answer");
  });

  // The banked half of a contract whose other half is compiler-enforced. `FallbackReason::PowerPolicy`
  // (llm_gateway.rs) has no producer yet and carries "Do not remove it or 'clean up' the unused
  // variant" — in Rust, exhaustiveness makes that stick. It does NOT survive the IPC string boundary:
  // delete the `power_policy` branch in FallbackStrip.tsx as apparently-dead code and the slug falls
  // through to the generic clause, reporting a DELIBERATE power-saving switch as a local model
  // FAILURE — with the whole suite still green, because the test below actively blesses that output
  // for unrecognised slugs. This is the guard that makes it fail instead.
  it("never reports a deliberate power-policy switch as a failure", () => {
    expect(fallbackCopy("power_policy")).toContain("save power");
    expect(fallbackCopy("power_policy")).not.toContain("couldn't answer");
  });

  // Same class of guard as the power-policy one above. A call-time posture refusal is a POLICY
  // decision about the address the endpoint now resolves to, not evidence the server failed — the
  // server is very likely running fine. Without this branch the slug falls through to "your local
  // model couldn't answer", sending the user to debug a healthy server instead of their DNS.
  it("never reports a call-time endpoint refusal as a model failure", () => {
    expect(fallbackCopy("endpoint_refused")).toContain("in the clear");
    expect(fallbackCopy("endpoint_refused")).not.toContain("couldn't answer");
  });

  it("degrades an unknown slug to a generic reason (never leaks the raw token)", () => {
    const copy = fallbackCopy("some_future_reason");
    expect(copy).toContain("couldn't answer");
    expect(copy).not.toContain("some_future_reason");
  });
});

describe("FallbackStrip", () => {
  it("renders the reason + the cloud model and fires onDismiss", () => {
    const onDismiss = vi.fn();
    const { container, getByTitle } = render(
      <FallbackStrip
        fallback={{
          ...LOCAL,
          from_model: "llama3",
          to_model: "openai/gpt-x",
          reason: "hard_failure:timeout",
        }}
        onDismiss={onDismiss}
      />,
    );
    expect(container.textContent).toContain("timed out");
    expect(container.textContent).toContain("gpt-x");
    fireEvent.click(getByTitle("Dismiss"));
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it("degrades gracefully when the model names are empty", () => {
    const { container } = render(
      <FallbackStrip
        fallback={{ ...LOCAL, from_model: "", to_model: "", reason: "cooldown" }}
        onDismiss={() => {}}
      />,
    );
    expect(container.textContent).toContain("cloud");
    expect(container.textContent).not.toContain("(via");
  });

  // The list's own notice is a different strip: it names the row that did not answer, why, and who
  // got the reply instead. The local-to-cloud words never appear for it.
  it("says, for a list, which row did not answer, why, and where the reply went", () => {
    const { container } = render(<FallbackStrip fallback={LIST_ONE} onDismiss={() => {}} />);
    // The strip's text is the sentence, then the Dismiss button's glyph.
    expect(container.textContent).toContain(
      "Claude Opus 5.5 couldn't answer (no credit left or at a spend limit), so Gemini 4 Carbon did. This reply went to Google, not Anthropic.",
    );
    expect(container.textContent).not.toContain("came from the cloud");
  });
});

describe("listFallbackCopy", () => {
  it("joins several rows into one sentence, the later ones shortened", () => {
    expect(
      listFallbackCopy({
        ...LIST_ONE,
        steps: [
          { label: "Claude Opus 5.5", company: "Anthropic", why: "no credit left", how: "failed" },
          { label: "Gemini 4 Carbon", company: "Google", why: "busy", how: "failed" },
        ],
        answered: { label: "vendor/some-model", via: "OpenRouter" },
      }),
    ).toBe(
      "Claude Opus 5.5 couldn't answer (no credit left) and Gemini 4 Carbon couldn't (busy), so some-model did. This reply went to OpenRouter, not Anthropic or Google.",
    );
  });

  it("names a Local AI server on another machine as what it is, not 'this computer'", () => {
    expect(
      listFallbackCopy({
        ...LIST_ONE,
        steps: [{ label: "Claude Opus 5.5", company: "Anthropic", why: "busy", how: "failed" }],
        answered: { label: "llama3", via: "your Local AI server" },
      }),
    ).toBe(
      "Claude Opus 5.5 couldn't answer (busy), so llama3 did. This reply went to your Local AI server, not Anthropic.",
    );
  });

  it("reads a row that was only held back as paused, and says 'answered'", () => {
    expect(
      listFallbackCopy({
        ...LIST_ONE,
        steps: [
          {
            label: "Gemini 4 Carbon",
            company: "Google",
            why: "paused where you are",
            how: "paused",
          },
        ],
        answered: { label: "Claude Opus 5.5", via: "Anthropic" },
      }),
    ).toBe(
      "Gemini 4 Carbon is paused where you are, so Claude Opus 5.5 answered. This reply went to Anthropic, not Google.",
    );
  });

  it("says a skipped row was passed over, and leaves out the company line when it did not change", () => {
    const copy = listFallbackCopy({
      ...LIST_ONE,
      steps: [
        { label: "Claude Opus 5.5", company: "Anthropic", why: "no key saved", how: "skipped" },
      ],
      answered: { label: "Claude Sonnet 5.5", via: "Anthropic" },
    });
    expect(copy).toBe("Claude Opus 5.5 was skipped (no key saved), so Claude Sonnet 5.5 answered.");
    expect(copy).not.toContain("This reply went to");
  });

  it("has a plain sentence when the event carries no steps", () => {
    expect(listFallbackCopy({ ...LIST_ONE, steps: [], answered: null })).toBe(
      "A later model in your list answered this reply.",
    );
  });
});
