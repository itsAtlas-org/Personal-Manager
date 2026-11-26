// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The daily briefing — a short "here's your picture today" synthesis (Step 7), rendered wherever the
// user has asked to see it. Lifted out of FocusView so the Focus card, the sidebar panel and the
// floating panel are literally the same component reading the same provider, rather than three
// copies that drift.
//
// The variants differ only in chrome (a Card vs. a bare block, heading size, how much vertical room
// the text may take). The body, the empty/generating states and the Refresh control are shared —
// which is the point: a change to how a briefing reads lands everywhere at once.

import { Button, Callout, Card, RefreshIcon } from "./ui";
import { Markdown } from "../lib/markdown";
import { formatDateTime } from "../lib/format";
import { useBriefing } from "../lib/briefing";

/** How much chrome the briefing wears. `card` is the Focus tab's full-width card; `panel` is the
 *  compact form used by the sidebar footer and the floating panel, where vertical room is scarce. */
export type BriefingVariant = "card" | "panel";

function RefreshButton({ busy, onRefresh }: { busy: boolean; onRefresh: () => void }) {
  return (
    <Button
      variant="tertiary"
      size="sm"
      onClick={onRefresh}
      disabled={busy}
      // The name is the visible word (WCAG 2.5.3 Label in Name: a speech user says "click Refresh"),
      // so there is no aria-label; what it does is the hover text, which a screen reader gets as the
      // description.
      title="Regenerate today's briefing from your current projects and calendar"
    >
      <RefreshIcon spinning={busy} />
      <span>{busy ? "Refreshing…" : "Refresh"}</span>
    </Button>
  );
}

/**
 * Today's briefing. Reads the shared provider, so every mounted instance shows the same text and one
 * Refresh updates all of them.
 *
 * Renders nothing at all when there is no briefing and none is being generated (e.g. an empty store)
 * — a zero-height empty state, so an enabled-but-empty surface never leaves a stray box behind.
 */
export function Briefing({
  variant = "card",
  className = "",
  fill = false,
}: {
  variant?: BriefingVariant;
  className?: string;
  /** Panel variant only: grow the text to the host's height instead of stopping at `max-h-48`.
   *  For hosts the USER can resize — the OS briefing window and the in-app floating panel — where a
   *  fixed cap left the window growing while the text stayed pinned, i.e. dead space below
   *  "Updated …". The sidebar must NOT pass it: there the cap is load-bearing, keeping a long
   *  briefing from pushing the nav out of reach. */
  fill?: boolean;
}) {
  const { briefing, busy, error, requestRefresh } = useBriefing();
  const text = briefing?.briefing.trim() ?? "";
  if (!text && !busy) return null;

  const onRefresh = () => void requestRefresh();
  const body = text ? (
    <div className="pm-inline-md text-sm leading-relaxed text-ink2">
      <Markdown>{text}</Markdown>
    </div>
  ) : (
    <p className="text-sm text-ink4">Putting together your briefing…</p>
  );
  // The time as well as the day: a Refresh that works on unchanged facts reads much like the old
  // briefing, and a date-only stamp could not tell the user it had run at all.
  const updated = text && briefing?.updated_at && (
    <p className="mt-2 text-xs text-ink4">Updated {formatDateTime(briefing.updated_at)}</p>
  );
  // Only a failure the user asked for (their Refresh click): the briefing itself stays as it was.
  // Kept OUT of the sidebar's `max-h-48` text box, so a long message neither scrolls away with the
  // text nor pushes the box taller; it gets its own small cap instead.
  const failure = error && (
    <Callout
      as="p"
      live={false}
      role="status"
      className={`mt-2 shrink-0 [overflow-wrap:anywhere] ${variant === "panel" && !fill ? "max-h-24 overflow-y-auto" : ""}`}
    >
      Couldn't refresh: {error}
    </Callout>
  );

  if (variant === "card") {
    return (
      <Card className={`mb-5 px-4 py-3 ${className}`} data-help="focus-briefing">
        <div className="mb-2 flex items-center justify-between">
          <h2 className="font-mono text-xs font-semibold uppercase tracking-wide text-ink3">
            Today
          </h2>
          <RefreshButton busy={busy} onRefresh={onRefresh} />
        </div>
        {body}
        {updated}
        {failure}
      </Card>
    );
  }

  // Panel: no Card chrome (the host supplies its own) and a smaller heading. The text scrolls inside
  // a bounded box in the sidebar, where growing would push the nav out of reach — but in a
  // user-resizable host (`fill`) it takes the height it is given, so the window and its content grow
  // together instead of leaving a black gap under "Updated …".
  return (
    <div
      className={`${fill ? "flex h-full min-h-0 flex-col" : ""} ${className}`}
      data-help="focus-briefing"
    >
      <div className="mb-1 flex shrink-0 items-center justify-between gap-2">
        <h2 className="font-mono text-[0.6875rem] font-semibold uppercase tracking-wide text-ink4">
          Today
        </h2>
        <RefreshButton busy={busy} onRefresh={onRefresh} />
      </div>
      <div className={fill ? "min-h-0 flex-1 overflow-y-auto" : "max-h-48 overflow-y-auto"}>
        {body}
      </div>
      {updated}
      {failure}
    </div>
  );
}
