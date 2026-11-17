// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The safety list's strips at the top of the Mods tab (Mods spec §1.10): a mod paused or removed
// while a problem is fixed, one back on, an update waiting, and the state of the list itself ("not
// checked since …" after 30 days, a damaged or unreadable record on this machine).
//
// WHERE THEY LIVE. PM sends no OS notifications, so these are how a person learns a mod was paused or
// removed, and that can't depend on opening the Mods tab: a mod with no tab of its own has nowhere
// else to say it. So they are drawn in two places (`variant`):
//
//   "page"  at the top of the Mods tab, where they point at "its row below";
//   "app"   as full-width strips at the top of the app on every other tab (ModsAppStrips), and again
//           inside Settings. Settings is a Modal whose scrim covers App's column, so the app's copy is
//           invisible and unclickable while it is open and Settings draws its own (the rule the
//           sign-in strip follows). The two are never drawn at once: App draws its copy only while
//           Settings is closed and the Mods tab isn't the view.
//
// The app copy leaves out what is not worth interrupting every tab for: the "never checked yet" line
// and a failed read of the notices.
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
import { modText, quoteName } from "./modText";

const RESET_KEY = "safety-reset";

/** A reason from the safety list, ready to follow a colon. */
const reasonOf = (reason: string) => modText(reason).replace(/[.\s]+$/, "");

type Variant = "page" | "app";

function noticeText(notice: Exclude<ModNotice, { kind: "recordFault" }>, variant: Variant): string {
  // The mod's name is its own words: made one plain line and quoted, so it can't reorder this
  // sentence or pass for part of it.
  const n = { ...notice, name: quoteName(notice.name) };
  // Where its row is: below on the Mods tab, in the Mods tab from anywhere else.
  const row = variant === "page" ? "its row below" : "its row in the Mods tab";
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
        `them: use Uninstall on ${row}.`
      );
    }
    case "updateWaiting":
      return `${n.name} has an update, version ${n.version}, that asks for more than you allowed. Review it in ${row}.`;
    case "updateNeedsNewerPm":
      return `${n.name} has a newer version, ${n.version}, that needs a newer PM. Update PM to get it.`;
  }
}

/** The strips for every view but the Mods tab (which draws its own): App draws this while Settings
 *  is closed. A button takes the person to the Mods tab, where the rows are. */
export function ModsAppStrips({ view, onOpenMods }: { view: string; onOpenMods: () => void }) {
  if (view === "mods") return null;
  return <ModsStrips variant="app" onOpenMods={onOpenMods} />;
}

export function ModsStrips({
  variant = "page",
  onOpenMods,
}: {
  variant?: Variant;
  /** The app copy's way to the Mods tab (Settings' copy has none: it is a dialog over the app). */
  onOpenMods?: () => void;
}) {
  const { status, problem, refresh } = useModsSafety();
  const app = variant === "app";
  // Page strips are boxes in a column; the app's are full-bleed bands that separate themselves with
  // a rule, like every other strip across the top of the app.
  const shape = app ? ({ variant: "strip" } as const) : ({} as const);
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
    return problem && !app ? (
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
  const never = status.checked.kind === "never" && !app ? status.checked.text : null;
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
    <div className={app ? undefined : "space-y-2"} data-help="mods-safety">
      {fault && (
        <Callout tone="warning" size="md" live {...shape}>
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
        <Callout tone="warning" size="md" live {...shape}>
          {stale}
        </Callout>
      )}
      {notices.map((n) => (
        <Strip
          key={`${n.kind}:${n.id}`}
          notice={n}
          refresh={refresh}
          variant={variant}
          onOpenMods={onOpenMods}
        />
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
          PM keeps a note on this computer of the newest safety list it has seen and of any maker
          keys that list has switched off.{" "}
          {fault?.damaged === false
            ? "Yours can't be read right now. If that's a passing problem PM reads it again by itself, so reset it only if it stays this way."
            : "Yours is damaged and can't be used."}
        </p>
        <p className="pt-2">
          Resetting clears that note. PM forgets which list was newest and which maker keys were
          switched off until it downloads the list again, which it does straight away. Your mods and
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
  variant,
  onOpenMods,
}: {
  notice: Exclude<ModNotice, { kind: "recordFault" }>;
  refresh: () => Promise<void>;
  variant: Variant;
  onOpenMods?: () => void;
}) {
  const { busy, errors } = useModActions();
  const key = `strip:${notice.kind}:${notice.id}`;
  const warn = notice.kind !== "resumed";
  return (
    <Callout
      tone={warn ? "warning" : "info"}
      size="md"
      live
      variant={variant === "app" ? "strip" : "box"}
    >
      <p className="break-words">{noticeText(notice, variant)}</p>
      {errors.get(key) && <p className="break-words pt-1">{errors.get(key)}</p>}
      {variant === "app" && onOpenMods && warn && (
        <div className="pt-2">
          <Button variant="secondary" size="sm" onClick={onOpenMods}>
            Open the Mods tab
          </Button>
        </div>
      )}
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
