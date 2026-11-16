// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A mod's icon beside its name. The bytes come from the backend (`mod_icon`: a PNG or WebP out of the
// mod's own verified files, size-capped and checked against what its extension claims) and become a
// `blob:` URL, because the main window's CSP has no `pmmod:` in `img-src`. The image is the mod's, so
// it is drawn small, with no alt text of its own (the name is beside it) and in a box that clips it.
// A mod without an icon, or one whose icon can't be read, gets its first letter on a plain square:
// the name is the mod's own text and drawn as text.

import { useEffect, useState } from "react";
import { modIcon } from "../../lib/ipc";
import { iconBlob } from "../../lib/modsTypes";

export function ModIconView({
  modId,
  version,
  name,
  hasIcon,
}: {
  modId: string;
  /** A new version may carry a new icon: reading it again is keyed on it. */
  version: string;
  name: string;
  hasIcon: boolean;
}) {
  const [url, setUrl] = useState<string | null>(null);

  useEffect(() => {
    if (!hasIcon) return;
    let cancelled = false;
    let made: string | null = null;
    void modIcon(modId)
      .then((icon) => {
        if (cancelled) return;
        made = URL.createObjectURL(iconBlob(icon));
        setUrl(made);
      })
      .catch(() => {
        // No icon is the same as the placeholder: the row is complete without one.
        if (!cancelled) setUrl(null);
      });
    return () => {
      cancelled = true;
      if (made) URL.revokeObjectURL(made);
      setUrl(null);
    };
  }, [modId, version, hasIcon]);

  const letter = Array.from(name.trim())[0]?.toUpperCase() ?? "?";
  return (
    <span
      aria-hidden
      className="flex h-10 w-10 flex-none items-center justify-center overflow-hidden rounded-[var(--radius-sm)] border border-border bg-surface font-head text-sm font-semibold text-ink3"
    >
      {url ? <img src={url} alt="" className="h-full w-full object-contain" /> : letter}
    </span>
  );
}
