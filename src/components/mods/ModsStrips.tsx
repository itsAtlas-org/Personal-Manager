// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The safety list's strips at the top of the Mods tab (Mods spec §1.10): a mod paused or removed
// while a problem is fixed, one back on, an update waiting, and the state of the list itself ("not
// checked since …" after 30 days, a damaged or unreadable record on this machine).
//
// WHERE THEY LIVE. Settings is a Modal whose scrim covers App's column, so an app-wide strip is
// invisible and unclickable while Settings is open. These are not app-wide: nothing a strip offers
// starts in Settings, and the Mods tab is a view, drawn whenever it is the tab, so the strips are
// drawn here and Settings needs none of them.
//
// WHAT THEY SAY comes from the backend's status (lib/useModsSafety), which derives it from the rows,
// its notes and the cached index and is read on mount (the event can fire before this view exists)
// and on every cue. The names and reasons are text, never markup. The one destructive thing here,
// "Reset the safety list", asks first and says what it forgets.

import { useEffect, useState } from "react";
import { dismissModResumed, resetSafetyRecord } from "../../lib/ipc";
import { addModNote, clearModError, runModAction, useModActions } from "../../lib/modActions";
import type { ModNotice } from "../../lib/modsTypes";
import { useModsSafety } from "../../lib/useModsSafety";
import { Button, Callout, ConfirmDialog } from "../ui";
import { modText } from "./modText";

const RESET_KEY = "safety-reset";

/** A reason from the safety list, ready to follow a colon. */
const reasonOf = (reason: string) => modText(reason).replace(/[.\s]+$/, "");

function noticeText(notice: Exclude<ModNotice, { kind: "recordFault" }>): string {
  // The mod's name is its own words (made one plain line, so it can't reorder this sentence).
  const n = { ...notice, name: modText(notice.name) };
  switch (n.kind) {
    case "paused": {
      const why = reasonOf(n.reason);
      return (
        `${n.name} is paused while we fix a problem${why ? `: ${why}` : ""}. Your data is safe. ` +
        (n.fixedIn
          ? `It will turn back on by itself when version ${n.fixedIn} arrives.`
          : "It will turn back on by itself when the fix arrives.")
      );
    }
    case "resumed":
      return `${n.name} is back on, fixed in version ${n.version}.`;
    case "removed": {
      const why = reasonOf(n.reason);
      return (
        `${n.name} was removed by PM's safety list${why ? `: ${why}` : ""}. It is switched off ` +
        "and its files are gone. What it saved and what it made are still here until you remove " +
        "them: use Uninstall on its row below."
      );
    }
    case "updateWaiting":
      return `${n.name} has an update, version ${n.version}, that asks for more than you allowed. Review it in its row below.`;
    case "updateNeedsNewerPm":
      return `${n.name} has a newer version, ${n.version}, that needs a newer PM. Update PM to get it.`;
  }
}

export function ModsStrips() {
  const { status, problem, refresh } = useModsSafety();
  const { busy, errors } = useModActions();
  const [confirmReset, setConfirmReset] = useState(false);

  // A fault that mends itself while the confirm is open takes the confirm with it. Without this the
  // flag would stay true and the next fault, days later, would open the dialog by itself.
  const hasFault =
    !!status && (status.record !== null || status.notices.some((n) => n.kind === "recordFault"));
  useEffect(() => {
    if (!hasFault) setConfirmReset(false);
  }, [hasFault]);

  if (!status) {
    return problem ? (
      <Callout tone="warning" size="md" live={false} data-help="mods-safety">
        <p>PM couldn&apos;t read the safety list&apos;s notices: {problem}</p>
        <div className="pt-1">
          <Button variant="secondary" size="sm" onClick={() => void refresh()}>
            Try again
          </Button>
        </div>
      </Callout>
    ) : null;
  }

  // The record's fault, from the status or, failing that, a fault notice the list carried.
  const faultNotice = status.notices.find(
    (n): n is Extract<ModNotice, { kind: "recordFault" }> => n.kind === "recordFault",
  );
  const fault = status.record
    ? { damaged: status.record.kind === "damaged", text: status.record.text }
    : faultNotice
      ? { damaged: !faultNotice.unreadable, text: faultNotice.text }
      : null;
  const notices = status.notices.filter(
    (n): n is Exclude<ModNotice, { kind: "recordFault" }> => n.kind !== "recordFault",
  );
  const stale = status.checked.kind === "stale" ? status.checked.text : null;
  const never = status.checked.kind === "never" ? status.checked.text : null;
  const resetting = busy.has(RESET_KEY);

  function reset() {
    void runModAction(
      RESET_KEY,
      "Resetting",
      "PM couldn't reset the safety list.",
      resetSafetyRecord,
      (r) => {
        if (!r.reset) {
          addModNote("info", "PM's record of the safety list was fine, so it was left as it was.");
        } else if (r.problem) {
          addModNote(
            "warning",
            `PM cleared its record of the safety list but couldn't download the list yet: ${r.problem} It will try again.`,
          );
        } else if (r.fetched) {
          addModNote(
            "info",
            "PM cleared its record of the safety list and downloaded the list again.",
          );
        } else {
          addModNote(
            "info",
            "PM cleared its record of the safety list. It will download the list again soon.",
          );
        }
      },
    ).then((r) => {
      void refresh();
      if (r.ok) setConfirmReset(false);
    });
  }

  if (!fault && !stale && !never && notices.length === 0) return null;
  return (
    <div className="space-y-2" data-help="mods-safety">
      {fault && (
        <Callout tone="warning" size="md" live>
          <p className="break-words">{fault.text}</p>
          {/* An unreadable record may be a passing failure (a busy disk), so it says first that PM
              tries again; but one that stays unreadable would leave the safety list off for good,
              so the reset is offered for it too, behind the same confirm that says what it forgets. */}
          {!fault.damaged && (
            <p className="pt-1 text-xs">
              PM will try to read it again by itself. If this keeps showing, you can start over.
            </p>
          )}
          <div className="pt-2">
            <Button
              variant="secondary"
              onClick={() => {
                clearModError(RESET_KEY);
                setConfirmReset(true);
              }}
            >
              Reset the safety list
            </Button>
          </div>
        </Callout>
      )}
      {stale && (
        <Callout tone="warning" size="md" live>
          {stale}
        </Callout>
      )}
      {notices.map((n) => (
        <Strip key={`${n.kind}:${n.id}`} notice={n} refresh={refresh} />
      ))}
      {never && <p className="text-xs text-ink3">{never}</p>}

      <ConfirmDialog
        open={confirmReset}
        title="Reset the safety list?"
        confirmLabel="Reset the safety list"
        danger
        busy={resetting}
        initialFocus="cancel"
        helpId="mods-reset-safety"
        onClose={() => setConfirmReset(false)}
        onConfirm={reset}
      >
        <p>
          PM keeps a record on this computer of the newest safety list it has accepted and of any
          signing keys that list cancelled.{" "}
          {fault?.damaged === false
            ? "Yours can't be read right now. If that's a passing problem PM reads it again by itself, so reset it only if it stays this way."
            : "Yours is damaged and can't be used."}
        </p>
        <p className="pt-2">
          Resetting clears that record. PM forgets which list was newest and which keys were
          cancelled until it downloads the list again, which it does straight away. Your mods and
          their data are not touched.
        </p>
        {errors.get(RESET_KEY) && (
          <Callout size="md" className="mt-2">
            {errors.get(RESET_KEY)}
          </Callout>
        )}
      </ConfirmDialog>
    </div>
  );
}

function Strip({
  notice,
  refresh,
}: {
  notice: Exclude<ModNotice, { kind: "recordFault" }>;
  refresh: () => Promise<void>;
}) {
  const { busy, errors } = useModActions();
  const key = `strip:${notice.kind}:${notice.id}`;
  const warn = notice.kind !== "resumed";
  return (
    <Callout tone={warn ? "warning" : "info"} size="md" live>
      <p className="break-words">{noticeText(notice)}</p>
      {errors.get(key) && <p className="break-words pt-1">{errors.get(key)}</p>}
      {notice.kind === "resumed" && (
        <div className="pt-2">
          <Button
            variant="secondary"
            size="sm"
            disabled={busy.has(key)}
            onClick={() => {
              clearModError(key);
              void runModAction(key, "Dismissing", "PM couldn't dismiss that.", () =>
                dismissModResumed(notice.id),
              ).then(() => refresh());
            }}
          >
            Dismiss
          </Button>
        </div>
      )}
    </Callout>
  );
}
