// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The dialog a mod's `secrets.set` opens (Mods spec §1.7): PM's own window with PM's own key field
// (ModSecretFields), so the mod can't draw it, press anything in it or read what is typed. The mod
// has only asked "I need the key named X"; the backend checked the name is one its manifest declares
// and counted the ask, and this draws it.
//
// WHAT IS SHOWN comes from the installed list, not from the ask: the slot's host and header are read
// from the mod's row when the dialog is drawn (and again after every change to the rows), so a
// manifest that moved between the ask and the typing shows where the key will go NOW. A mod that is
// gone, or that no longer declares the slot, has no dialog (the store drops it, and nothing is drawn
// meanwhile). One dialog per mod, one mod's dialog at a time.
//
// Drawn only while Settings is closed: Settings is a Modal and a second dialog over it would fight for
// focus. The ask waits, and is shown when Settings closes. The mod's name is quoted and clipped
// (ModName) and its tier worn as PM's badge.

import { closeKeyAsk, useModKeyAsks } from "../../lib/modSurface";
import { useModRows } from "../../lib/modRows";
import { Button, Dialog } from "../ui";
import { ModName } from "./ModName";
import { ModSecretFields } from "./ModSecretFields";
import { TierBadge } from "./TierBadge";

export function ModKeyAsks() {
  const asks = useModKeyAsks();
  const { rows, refresh } = useModRows();

  for (const ask of asks) {
    const mod = rows.find((r) => r.id === ask.from.modId);
    const slot = mod?.secrets.find((s) => s.name === ask.slot);
    if (!mod || !slot) continue;
    const close = () => closeKeyAsk(mod.id);
    return (
      <Dialog
        key={mod.id}
        open
        title={
          <>
            A key for <ModName name={mod.name} />
          </>
        }
        onClose={close}
        widthClassName="max-w-md"
        helpId="mods-key-ask"
        footer={
          <Button variant="primary" onClick={close} data-autofocus>
            Done
          </Button>
        }
      >
        <div className="mt-2 space-y-3 text-sm text-ink2">
          <p className="flex flex-wrap items-center gap-2 text-xs text-ink3">
            <TierBadge tier={mod.tier} />
          </p>
          <p>
            This mod needs a key to use a service. Type it here: it goes to PM and is kept on this
            computer, and the mod is never shown it.
          </p>
          <ModSecretFields modId={mod.id} slots={[slot]} onChanged={() => void refresh()} />
        </div>
      </Dialog>
    );
  }
  return null;
}
