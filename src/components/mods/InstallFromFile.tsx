// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Install from file section of the Mods tab (Mods spec §1.13): the ONLY home of installing a mod
// from a `.pmmod` file. Choosing a file reads and checks it and installs nothing; what comes back is
// the consent screen (ConsentDialog). A file signed by Atlas gets the tiered screen. A file nobody
// signed is taken only with developer mode on, because nobody has checked it: with it off the
// section says so and where to switch it on; with it on the backend returns the stark screen
// (UnsignedConsentDialog). Which of the two a file gets is the backend's call (lib/modInstall).
//
// The read runs in lib/modInstall under one key, so leaving the Mods tab while PM is reading the file
// loses nothing: coming back shows the screen, or the backend's refusal.

import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { useModActions } from "../../lib/modActions";
import { FILE_KEY, startFileInstall } from "../../lib/modInstall";
import { Button, Callout } from "../ui";

export function InstallFromFile() {
  const { busy, errors } = useModActions();
  const [pickError, setPickError] = useState<string | null>(null);
  const working = busy.get(FILE_KEY) ?? null;
  const error = pickError ?? errors.get(FILE_KEY) ?? null;

  async function choose() {
    setPickError(null);
    let picked: string | string[] | null;
    try {
      picked = await open({
        multiple: false,
        filters: [{ name: "PM mod", extensions: ["pmmod"] }],
      });
    } catch {
      setPickError("PM couldn't open the file picker.");
      return;
    }
    if (typeof picked === "string") void startFileInstall(picked);
  }

  return (
    <div className="space-y-3">
      <p className="text-sm text-ink2">
        Install a mod from a <code className="font-mono">.pmmod</code> file. This is the only place
        to do that. PM checks the file first, then shows what it asks to do, and nothing is
        installed until you say so.
      </p>
      <p className="text-xs text-ink3">
        A file signed by Atlas installs the way a mod from the catalogue does. A file nobody has
        signed can&apos;t be checked by anyone, so PM only takes one when developer mode is on
        (Settings, then Developer), and then asks you to type the mod&apos;s id to confirm.
      </p>
      <div>
        <Button variant="secondary" onClick={() => void choose()} disabled={working !== null}>
          {working ? `${working}…` : "Choose a mod file…"}
        </Button>
      </div>
      {error && <Callout size="md">{error}</Callout>}
    </div>
  );
}
