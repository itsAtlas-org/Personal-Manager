// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the Connected apps section says and offers, apart from the component so the words and the
// defaults are one place a test and a reviewer can read (and the component file exports only a
// component, which Fast Refresh needs). Scope ids are API 1.0's permission ids; `calendar.details`
// is never offered, and nothing here lets an app change anything.

import type { McpScope } from "../../lib/modsTypes";

/** The longest an app's name may be (Rust's `NAME_MAX_CHARS`). */
export const NAME_MAX = 60;

/** What an app may be given, in the order the form lists it. `on` is whether it starts ticked. */
export const SCOPE_CHOICES: readonly {
  id: McpScope;
  label: string;
  detail: string;
  on: boolean;
}[] = [
  {
    id: "tasks.read",
    label: "Tasks",
    // Says what the task record really holds (its "why" note too), and that a task's project or linked
    // event is named only when those boxes are ticked.
    detail:
      "Your tasks: titles, notes, dates and status. A task's project or linked event is named only if you tick those too.",
    on: true,
  },
  {
    id: "projects.read",
    label: "Projects and milestones",
    detail: "Project names and milestones, and which project a task belongs to.",
    on: true,
  },
  {
    id: "calendar.read",
    label: "Calendar events (no attendees)",
    detail: "Event titles, times and locations. Never who is invited.",
    on: true,
  },
  {
    // Starts OFF, with its own plain line: it sends text from the person's documents.
    id: "documents.text",
    label: "Documents search",
    detail: "Sends text from your documents.",
    on: false,
  },
];

/** The scopes ticked when the form opens. */
export const DEFAULT_SCOPES: readonly McpScope[] = SCOPE_CHOICES.filter((s) => s.on).map(
  (s) => s.id,
);

/** A scope's name as the list shows it. */
export const SCOPE_LABEL: Record<string, string> = Object.fromEntries(
  SCOPE_CHOICES.map((s) => [s.id, s.label]),
);

export const WARN_PROVIDER =
  "The AI provider of any app you connect is outside PM's zero-data-retention promise: what PM returns to it leaves PM.";
export const WARN_CONFIG =
  "Any program running as you can read a connected app's saved configuration and use its token.";
export const NOTE_UNTRUSTED =
  "What PM returns is text from your own data; treat it as untrusted by the app.";

export const COPY_FAILED = "PM couldn't copy it. Select the text above and copy it yourself.";
