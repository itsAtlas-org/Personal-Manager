// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Mods disclaimer (spec §1.13): mods are made by people, what PM checks, and what it can't. One
// text, drawn by PM, for the Mods tab's first open and for an auto-installed mod's first-open screen
// (D17), so the two can't say different things. It is static: nothing a mod supplies goes in it.

export function ModsDisclaimer() {
  return (
    <div className="space-y-2 text-sm leading-relaxed text-ink2">
      <p>
        Mods are small add-ons made by people, not by PM. Some are made by the Atlas team, some by
        people the Atlas team has looked over, and some by anyone.
      </p>
      <p>
        <span className="font-medium text-ink">What PM checks.</span> A mod from the catalogue is
        signed by the Atlas team, and PM checks the signature before the mod runs. PM keeps a safety
        list of known problems and can pause or remove a mod that is on it. Each mod runs in a
        sealed box and can only do what you allow.
      </p>
      <p>
        <span className="font-medium text-ink">What PM can&apos;t check.</span> Whether a mod is any
        good, or what it does with something you let it read. A mod you allow to read your documents
        can read them. A mod from a file that nobody has signed has not been checked by anyone.
      </p>
      <p>You can turn any mod off, or remove it, from the Mods tab.</p>
    </div>
  );
}
