// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "About this mod": the help a mod's maker wrote (`contributions.help`), drawn by PM over the mod's
// tab (Mods spec §1.17: help mode can't see inside a frame, so PM shows the mod's static entries
// itself). It is the maker's text in PM's panel, so it is held to the same rules as every other
// word of a mod's:
//
// - a title and the description are plain lines (`modText`), drawn as React text;
// - a body is Markdown source and goes through `markdown.tsx`, PM's one sanitizing boundary, and
//   nothing else: never `dangerouslySetInnerHTML`, never an HTML string, never the mod's own markup.
//   Links in it are drawn as their text (`links={false}`): a link here would be a clickable thing PM
//   seems to vouch for, and images already come out as their alt text;
// - the panel says whose words these are, so a body can't pass for PM's own voice.
//
// The frame stays mounted underneath (ModFrame keeps the session), so closing this loses nothing.

import { useEffect, useRef } from "react";
import type { ModHelpEntry } from "../../lib/modsTypes";
import { Markdown } from "../../lib/markdown";
import { isKeyForADialog } from "../../lib/useDialogLayer";
import { Button } from "../ui";
import { modBody, modText } from "./modText";

export interface ModAboutProps {
  /** The mod's own description (the manifest's), as a plain line. */
  description: string;
  help: readonly ModHelpEntry[];
  /** Who made it, already in `makerCredit`'s form ("Atlas · credited in the mod to Bobby Yu"); empty to say nothing. */
  credit?: string;
  onClose: () => void;
}

export function ModAbout({ description, help, credit, onClose }: ModAboutProps) {
  const closeRef = useRef<HTMLButtonElement>(null);
  const about = modText(description);
  // An entry with no body has nothing to say; one with no title still reads, under a plain heading.
  const entries = help
    .map((h) => ({ title: modText(h.title) || "Help", body: modBody(h.body) }))
    .filter((h) => h.body !== "");

  useEffect(() => {
    closeRef.current?.focus();
  }, []);

  // Escape closes it, unless a dialog owns the key: the permission question PM asks over the frame
  // is a modal dialog, and one Escape must answer it ("Not now"), not also close this panel.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape" && !e.defaultPrevented && !isKeyForADialog(e.target)) onClose();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="min-h-0 flex-1 overflow-y-auto" data-help="mod-about">
      <div className="mx-auto max-w-xl space-y-4 px-6 py-6">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h2 className="font-head text-lg font-semibold text-ink">About this mod</h2>
          <Button ref={closeRef} variant="secondary" size="sm" onClick={onClose}>
            Close
          </Button>
        </div>
        <p className="text-xs text-ink3">
          Written by the mod&apos;s maker, not by PM. PM shows it as plain text.
        </p>
        {credit && <p className="break-words text-xs text-ink3">Made by {credit}</p>}
        {about && <p className="break-words text-sm text-ink2">{about}</p>}
        {entries.length === 0 && (
          <p className="text-sm text-ink3">This mod&apos;s maker hasn&apos;t written any help.</p>
        )}
        {entries.map((h, i) => (
          <section key={i} className="space-y-1">
            <h3 className="break-words text-sm font-semibold text-ink">{h.title}</h3>
            <div className="pm-mod-help">
              <Markdown links={false}>{h.body}</Markdown>
            </div>
          </section>
        ))}
      </div>
    </div>
  );
}
