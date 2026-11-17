// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Mods tab's first open (Mods spec §1.13): the disclaimer, before any mod can be used. "I
// understand" is `accept_mods_disclaimer`, and the backend remembers it per device AND per vault, so a
// new machine or a vault the person joins asks again. What it says is `ModsDisclaimer`, the one text
// the first-open screen of an auto-installed mod shows too (D17: that screen's "Turn on" also counts as
// accepting, so the next read here finds it accepted without a press).
//
// The gate answers only what the backend says: while it hasn't answered, or can't, nothing the tab
// offers is shown, because "nobody has told me" is not "they accepted". It reads again on every
// `mods://changed` (a "Turn on" elsewhere writes the same record) and after its own button.

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { acceptModsDisclaimer, modsDisclaimerState, onModsChanged } from "../../lib/ipc";
import { runModAction, clearModError, useModActions } from "../../lib/modActions";
import { failureText, type ModsDisclaimerState } from "../../lib/modsTypes";
import { subscribeUntilCleanup } from "../../lib/subscribe";
import { useDisclaimerShowing } from "../../lib/useDisclaimerShowing";
import { Button, Callout, Card } from "../ui";
import { ModsDisclaimer } from "./ModsDisclaimer";

const KEY = "disclaimer";

export function ModsDisclaimerGate({ children }: { children: ReactNode }) {
  const [state, setState] = useState<ModsDisclaimerState | null>(null);
  const [readError, setReadError] = useState<string | null>(null);
  const asked = useRef(0);
  const alive = useRef(true);
  const { busy, errors } = useModActions();

  const read = useCallback(async () => {
    const mine = ++asked.current;
    try {
      const next = await modsDisclaimerState();
      if (alive.current && mine === asked.current) {
        setState(next);
        setReadError(null);
      }
    } catch (e) {
      if (alive.current && mine === asked.current) {
        setReadError(failureText(e, "PM couldn't check whether you've read this."));
      }
    }
  }, []);

  useEffect(() => {
    alive.current = true;
    void read();
    const stop = subscribeUntilCleanup(() => onModsChanged(() => void read()));
    return () => {
      alive.current = false;
      stop();
    };
  }, [read]);

  // The full text is on screen from the moment the backend says it isn't accepted yet, until it is.
  // Each such stretch is one showing for the count behind a mod's "Don't show this again" (that box
  // is never offered here: this screen is always in full).
  useDisclaimerShowing(state !== null && !state.accepted, false);

  // A read that failed after one that said "accepted" keeps the acceptance: nothing changed.
  if (state?.accepted) return <>{children}</>;

  if (state === null) {
    return readError ? (
      <div className="space-y-3">
        <Callout size="md">{readError}</Callout>
        <Button variant="secondary" onClick={() => void read()}>
          Try again
        </Button>
      </div>
    ) : (
      <p className="text-sm text-ink3">Loading…</p>
    );
  }

  const working = busy.has(KEY);
  const error = errors.get(KEY) ?? null;
  return (
    <section aria-labelledby="mods-disclaimer-title" data-help="mods-disclaimer">
      <Card className="space-y-4 p-5">
        <h2 id="mods-disclaimer-title" className="font-head text-base font-semibold text-ink">
          Before you use mods
        </h2>
        {state.problem && (
          <Callout tone="warning" size="md" live={false}>
            PM couldn&apos;t tell whether you&apos;ve already read this, so it is asking again:{" "}
            {state.problem}
          </Callout>
        )}
        <ModsDisclaimer />
        {error && <Callout size="md">{error}</Callout>}
        <Button
          variant="primary"
          disabled={working}
          onClick={() => {
            clearModError(KEY);
            void runModAction(KEY, "Saving", "PM couldn't save that.", acceptModsDisclaimer).then(
              () => read(),
            );
          }}
        >
          {working ? "Saving…" : "I understand"}
        </Button>
      </Card>
    </section>
  );
}
