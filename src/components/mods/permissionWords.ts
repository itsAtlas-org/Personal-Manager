// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What each permission means, in plain words, for the screens PM draws about a mod (Mods spec §1.5's
// table). The backend sends a permission's id, its class and the maker's own reason; the words for the
// id are PM's, so a maker can't dress "reads all your documents" up as something else, and a screen
// never shows the user a dotted name. The maker's reason is shown beside these, as the maker's.
// `permissionWords.test.ts` pins the list to the backend's table.

import type { ModPermission, PermissionClass } from "../../lib/modsTypes";
import { modText } from "./modText";

/** What a screen says about a permission that has no checkbox of its own (one asked at install gets
 *  a checkbox instead). */
export function standingLine(p: Pick<ModPermission, "when" | "offBecause">): string {
  switch (p.when) {
    case "auto":
      return "Always on.";
    case "first-use":
    case "install-then-first-use":
      return "PM asks you the first time the mod needs this.";
    default:
      // "ask" (a mod nobody has checked, one permission at a time) and anything PM doesn't know:
      // off until it is turned on from the Mods tab.
      return p.offBecause ?? "Off until you turn it on in the Mods tab.";
  }
}

/** A widening an update asks for, in words (`permission:<id>`, `host:<site>`, `secret:<name>`,
 *  `contribution:<what>`, as the backend lists them). One PM doesn't know is shown as it is. */
export function wideningWords(w: string): string {
  const at = w.indexOf(":");
  const kind = at < 0 ? "" : w.slice(0, at);
  const rest = at < 0 ? w : w.slice(at + 1);
  switch (kind) {
    case "permission":
      return permissionWords(rest);
    // The site, key name and contribution are the mod's own words: one plain line each.
    case "host":
      return `Reach ${modText(rest)} on the internet`;
    case "secret":
      return `Keep a saved key called “${modText(rest)}”`;
    case "contribution":
      return `Add something of its own to PM: ${modText(rest)}`;
    default:
      return modText(w);
  }
}

const WORDS: Record<string, string> = {
  storage: "Keep its own saved data inside PM",
  "ui.tab": "Have a tab of its own in the sidebar",
  notify: "Show you notices inside PM",
  "projects.read": "Read your projects, milestones and tags",
  "tasks.read": "Read your tasks",
  "documents.meta": "See your documents' titles, projects, tags, dates and types",
  "documents.vectors": "Read the numbers PM uses to tell how alike your documents are",
  "documents.text": "Read the text of your documents and search them",
  "calendar.read": "Read the time, title and place of your calendar events",
  "calendar.details": "Read your calendar events' guests, organiser, description and links",
  "tasks.write": "Add tasks, and change your tasks' status",
  "milestones.write": "Add milestones, and change your milestones",
  "documents.write": "Save notes into your documents",
  ai: "Ask an AI model to do things for it, which can cost money",
  internet: "Send and fetch things from sites on the internet",
  links: "Open web links in your browser",
};

/** The permission ids this file has words for (the test compares them to the backend's). */
export const PERMISSION_IDS: readonly string[] = Object.keys(WORDS);

const CLASS_WORDS: Record<PermissionClass, string> = {
  local: "Stays inside PM",
  read: "Reads your data",
  write: "Changes your data",
  money: "Can cost money",
  egress: "Reaches outside PM",
};

/** What a permission lets a mod do. An id PM has no words for is shown as it is, never hidden. */
export function permissionWords(id: string): string {
  return Object.prototype.hasOwnProperty.call(WORDS, id) ? WORDS[id] : `Use "${id}"`;
}

/** The kind of thing a permission touches. A class PM doesn't know says nothing rather than
 *  guessing. */
export function classWords(cls: string): string | null {
  return Object.prototype.hasOwnProperty.call(CLASS_WORDS, cls)
    ? CLASS_WORDS[cls as PermissionClass]
    : null;
}
