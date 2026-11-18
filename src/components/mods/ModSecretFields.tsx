// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// PM's own field for the keys a mod's manifest declares (Mods spec §1.7, §1.15). ONE component for
// every place a key is typed: the Installed row's Details, Settings → Developer → Test keys, and the
// dialog a mod's `secrets.set` opens. A key is entered here, in PM's chrome, and goes from the field to
// PM's backend (`modSecretSave`) and nowhere else: the mod never sees it, and PM never sends it back
// (the backend answers only whether a key is stored).
//
// What the field owes the person:
//   - it is masked, and says where the key will be sent ("only to <host>, in the <header> header"): the
//     host and header are the INSTALLED manifest's, copied by the backend when the key is saved, so
//     what is shown is where it goes;
//   - Replace and Remove are separate, and an empty field can't be saved (the backend treats an empty
//     key as "delete it", so a stray click on Save must never do that: Remove is its own button);
//   - the typed key leaves the page the moment it is sent, whether the save works or not. It is held in
//     this component's state only, never in a store that outlives it, and a failure's sentence is the
//     backend's fixed words, which never quote the key.
//
// The slot's name, host and header are the mod's own words (they come from its manifest): each is drawn
// as one plain line (`modText`).

import { useState } from "react";
import { modSecretClear, modSecretSave } from "../../lib/ipc";
import { clearModError, runModAction, useModActions } from "../../lib/modActions";
import { modsErrorOf, type ModSecretSlot } from "../../lib/modsTypes";
import { Button, Field, Input } from "../ui";
import { modText } from "./modText";

/** The busy/error key of one slot. A `:` can't be in a mod id, so it never meets a mod's own key. */
const slotKey = (modId: string, name: string) => `secret:${modId}:${name}`;

/** Run `call` so that only a refusal PM wrote reaches the person. Any other failure (an error from
 *  the IPC layer, whatever it says) becomes PM's own fallback sentence: a key was in that request, and
 *  nothing but PM's fixed words may be drawn beside the field it came from. */
async function pmWordsOnly<T>(call: () => Promise<T>): Promise<T> {
  try {
    return await call();
  } catch (e) {
    // An empty message makes `failureText` use the caller's fallback.
    throw modsErrorOf(e) ? e : new Error("");
  }
}

export function ModSecretFields({
  modId,
  slots,
  disabled = false,
  onChanged,
}: {
  modId: string;
  /** The mod's slots as the installed list has them (name, host, header, and whether a key is stored). */
  slots: readonly ModSecretSlot[];
  /** Something else is changing this mod now. */
  disabled?: boolean;
  /** A key was saved or removed: read the rows again. */
  onChanged?: () => void;
}) {
  // The backend's last answer to a save or a remove is newer than the rows until they are read again;
  // a change to the rows themselves wins over it.
  const sig = JSON.stringify(slots);
  const [seen, setSeen] = useState(sig);
  const [latest, setLatest] = useState<readonly ModSecretSlot[] | null>(null);
  if (seen !== sig) {
    setSeen(sig);
    setLatest(null);
  }
  const live = (slot: ModSecretSlot): boolean =>
    latest?.find((l) => l.name === slot.name)?.set ?? slot.set;

  return (
    <div className="space-y-3" data-help="mods-secret-fields">
      {slots.map((slot) => (
        <SlotField
          key={slot.name}
          modId={modId}
          slot={slot}
          isSet={live(slot)}
          disabled={disabled}
          onDone={(next) => {
            setLatest(next);
            onChanged?.();
          }}
        />
      ))}
    </div>
  );
}

function SlotField({
  modId,
  slot,
  isSet,
  disabled,
  onDone,
}: {
  modId: string;
  slot: ModSecretSlot;
  isSet: boolean;
  disabled: boolean;
  onDone: (slots: readonly ModSecretSlot[]) => void;
}) {
  const key = slotKey(modId, slot.name);
  const { busy, errors } = useModActions();
  const working = busy.get(key) ?? null;
  const error = errors.get(key) ?? null;
  const [typed, setTyped] = useState("");
  const name = modText(slot.name);
  const host = modText(slot.host);
  const header = modText(slot.header);
  const frozen = disabled || working !== null;

  function save() {
    // The key leaves the page now, win or lose: only this local copy goes to the backend.
    const value = typed;
    setTyped("");
    clearModError(key);
    void runModAction(key, "Saving", "PM couldn't save that key.", () =>
      pmWordsOnly(() => modSecretSave(modId, slot.name, value)),
    ).then((r) => {
      if (r.ok) onDone(r.value);
    });
  }

  function remove() {
    setTyped("");
    clearModError(key);
    void runModAction(key, "Removing", "PM couldn't remove that key.", () =>
      pmWordsOnly(() => modSecretClear(modId, slot.name)),
    ).then((r) => {
      // A remove answers nothing; the slot reads as empty until the rows are read again.
      if (r.ok) onDone([{ ...slot, set: false }]);
    });
  }

  return (
    <div className="space-y-1">
      <Field
        label={`Key: ${name}`}
        description={`PM sends it only to ${host}, in the ${header} header. The mod never sees it.`}
      >
        {(controlProps) => (
          <div className="flex flex-wrap items-center gap-2">
            <Input
              {...controlProps}
              type="password"
              autoComplete="off"
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              className="max-w-xs"
              placeholder={isSet ? "Type a new key to replace it" : "Paste the key"}
              value={typed}
              disabled={frozen}
              onChange={(e) => setTyped(e.target.value)}
            />
            <Button
              variant="secondary"
              size="sm"
              disabled={frozen || typed.trim() === ""}
              aria-label={`${isSet ? "Replace" : "Save"} the key ${name}`}
              onClick={save}
            >
              {working === "Saving" ? "Saving…" : isSet ? "Replace key" : "Save key"}
            </Button>
            {isSet && (
              <Button
                variant="secondary"
                size="sm"
                disabled={frozen}
                aria-label={`Remove the key ${name}`}
                onClick={remove}
              >
                {working === "Removing" ? "Removing…" : "Remove key"}
              </Button>
            )}
          </div>
        )}
      </Field>
      <p className="text-xs text-ink3" role="status">
        {isSet ? "A key is saved." : "No key is saved."}
      </p>
      {error && (
        <p role="alert" className="break-words text-xs text-st-due">
          {error}
        </p>
      )}
    </div>
  );
}
