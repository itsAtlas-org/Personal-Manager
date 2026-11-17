// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A mod's name inside a heading or a dialog title: quoted (`quoteName`) and clipped to its own box, so
// whatever the name is made of can't paint over the badges, buttons and sentences beside it. Plain
// React text, never markup.

import { quoteName } from "./modText";

export function ModName({ name }: { name: string }) {
  return (
    <span className="inline-block max-w-full overflow-hidden break-words align-bottom">
      {quoteName(name)}
    </span>
  );
}
