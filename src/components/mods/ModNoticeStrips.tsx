// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The strips a mod's `notify` puts at the top of PM (Mods spec §1.4, §1.8): the only way a mod can say
// something once its tab is closed, since PM sends no OS notifications. They are drawn in two places, as
// the safety list's strips are (ModsStrips): across the top of the app while Settings is closed, and
// inside Settings, whose scrim covers the app's copy (App draws its own only while Settings is closed).
//
// A MOD'S WORDS ARE UNTRUSTED, so the strip is built to be unable to pose as PM:
//   - it is a band that names the mod, quoted and clipped (ModName), and wears the mod's tier as PM's
//     badge, so nobody has to guess who is speaking; the words are on their own line under it;
//   - the words are TEXT: one plain line (`modText`), drawn by React as text, so a link, a button or
//     markup in them is just characters. Nothing in a strip is clickable except PM's own "Dismiss";
//   - no tone borrows PM's error look: `warn` is the caution tint, `info` and `good` the neutral one,
//     and nothing a mod sends makes a danger strip;
//   - at most MAX_NOTICES_PER_MOD per mod (the newest replaces the oldest) and at most MAX_SHOWN in all;
//     the rest wait behind a line that says so, so a mod can't fill the screen or bury another's strip.
//
// They are not stored anywhere: a restart or a lock clears them (lib/modSurface).

import { dismissAllNotices, dismissNotice, useModNotices } from "../../lib/modSurface";
import { Button, Callout } from "../ui";
import { ModName } from "./ModName";
import { modText } from "./modText";
import type { NoticeTone } from "./modPageHooks";
import { TierBadge } from "./TierBadge";

/** The most strips drawn at once, whatever the mods send. */
export const MAX_SHOWN = 5;

const TONE: Record<NoticeTone, "info" | "warning"> = {
  info: "info",
  good: "info",
  warn: "warning",
};

export function ModNoticeStrips() {
  const notices = useModNotices();
  if (notices.length === 0) return null;
  // Newest last, as they arrived; when there are too many the oldest wait.
  const shown = notices.slice(-MAX_SHOWN);
  const waiting = notices.length - shown.length;
  return (
    <div data-help="mods-notice" data-testid="mod-notices">
      {waiting > 0 && (
        <Callout tone="info" size="md" live={false} variant="strip">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <p>
              {waiting === 1
                ? "1 older notice from a mod is waiting."
                : `${waiting} older notices from mods are waiting.`}{" "}
              Dismiss one to see the next.
            </p>
            <Button variant="tertiary" size="sm" onClick={dismissAllNotices}>
              Dismiss all
            </Button>
          </div>
        </Callout>
      )}
      {shown.map(({ id, from, notice }) => (
        <Callout key={id} tone={TONE[notice.tone]} size="md" live variant="strip">
          <div className="flex items-start gap-3">
            <div className="min-w-0 flex-1 space-y-1">
              <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs">
                <span>A notice from</span>
                <span className="font-medium">
                  <ModName name={from.name} />
                </span>
                <TierBadge tier={from.tier} />
              </p>
              <p className="break-words">{modText(notice.text)}</p>
            </div>
            <Button
              variant="tertiary"
              size="sm"
              className="shrink-0"
              aria-label={`Dismiss the notice from ${modText(from.name)}`}
              onClick={() => dismissNotice(id)}
            >
              Dismiss
            </Button>
          </div>
        </Callout>
      ))}
    </div>
  );
}
