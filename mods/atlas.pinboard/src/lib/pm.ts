// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Pinboard's door to PM: the calls the core Pinboard made through `lib/ipc`, kept under the same
// names and the same shapes where they can be, each made as a Host API call (Mods spec 1.5) over the
// bridge. The copied view and hook import from here where core imported `ipc`, so the diff between the
// two stays the import line. A frame has no Tauri and no `localStorage`; this file and the page's
// start-up (frame/) are the only places that know that.
//
// What PM answers is read defensively: a reply that is not the shape the call documents is the page
// being handed something it cannot use, and says so (`PmAnswerError`) instead of drawing from it.
// What PM refuses reaches the view as a `PmError` whose text is PM's own sentence and nothing else
// (core's errors were plain strings; the views show `String(e)`).

import { ModCallError, type ModHost } from "../../../../src/kit/host";
import { tidyLabel, tidyLine } from "./pinboard/calendarEntries";
import type { Milestone, MilestoneStatus, PinboardEntry } from "./types";

/** PM refused or failed a call. Its text is PM's own sentence (never the call's arguments), and
 *  `String(error)` is exactly that, as core's string errors were. */
export class PmError extends Error {
  readonly code: string | null;
  /** The key's current revision, when PM refused a stale write and said so. */
  readonly revision: number | null;

  constructor(message: string, code: string | null = null, revision: number | null = null) {
    super(message);
    this.name = "PmError";
    this.code = code;
    this.revision = revision;
  }

  override toString(): string {
    return this.message;
  }
}

/** PM answered, but not in the shape the call documents. */
export class PmAnswerError extends PmError {
  constructor(call: string) {
    super(`PM's answer to ${call} wasn't what the Pinboard expected.`);
    this.name = "PmAnswerError";
  }
}

/** The codes the page acts on (the Rust codes of the same names; hostApi tests pin them). */
export const CODE = {
  staleRevision: "MOD-4001",
  declined: "MOD-2004",
  tooLarge: "MOD-3002",
  quotaExceeded: "MOD-3003",
} as const;

let current: ModHost | null = null;

/** Connect the page to PM (the frame's start-up) or, with null, let go. */
export function connectPm(host: ModHost | null): void {
  current = host;
  revisions.clear();
}

/** The connected host; a call before the page connected is a bug, and says so. */
export function pmHost(): ModHost {
  if (!current) throw new PmError("The Pinboard isn't connected to PM.");
  return current;
}

async function call(method: string, args?: unknown): Promise<unknown> {
  try {
    return await pmHost().call(method, args);
  } catch (error) {
    if (error instanceof ModCallError) {
      throw new PmError(error.message, error.code, error.revision ?? null);
    }
    if (error instanceof PmError) throw error;
    throw new PmError(error instanceof Error ? error.message : "PM didn't answer.");
  }
}

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const isWhole = (v: unknown): v is number =>
  typeof v === "number" && Number.isSafeInteger(v) && v >= 0;

// --- storage: the board and the preferences -------------------------------------------------------

/** The revision each key was last read or written at: a write names it (`expectedRevision`), and PM
 *  refuses one made over a value that changed since. A key never read has none, and is never written. */
const revisions = new Map<string, number>();

/** What the key holds, as the JSON text core's `getPref` answered, or null for nothing stored. */
export async function getPref(key: string): Promise<string | null> {
  const reply = await call("storage.get", { key });
  if (!isRecord(reply) || !isWhole(reply.revision) || !("value" in reply)) {
    throw new PmAnswerError("storage.get");
  }
  revisions.set(key, reply.revision);
  return reply.value === null ? null : JSON.stringify(reply.value);
}

/** Store `json` (a JSON text) under `key`, over the revision it was read at. A stale write is refused
 *  (`PmError.code` is `MOD-4001`): nothing is overwritten. */
export async function setPref(key: string, json: string): Promise<void> {
  const expectedRevision = revisions.get(key);
  if (expectedRevision === undefined)
    throw new PmError("That was never read, so it won't be saved over.");
  const reply = await call("storage.set", {
    key,
    value: JSON.parse(json) as unknown,
    expectedRevision,
  });
  if (!isRecord(reply) || !isWhole(reply.revision)) throw new PmAnswerError("storage.set");
  revisions.set(key, reply.revision);
}

// --- projects and milestones ----------------------------------------------------------------------

/** Every project's name. */
export async function listProjects(): Promise<string[]> {
  const reply = await call("projects.list");
  if (!isRecord(reply) || !Array.isArray(reply.projects)) throw new PmAnswerError("projects.list");
  return reply.projects.filter((p): p is string => typeof p === "string");
}

/** The most pages of milestones read: PM pages by id under a byte budget, and a library has far fewer. */
const MAX_PAGES = 200;

function readMilestone(raw: unknown): Milestone | null {
  if (!isRecord(raw) || typeof raw.id !== "number" || typeof raw.project_name !== "string")
    return null;
  if (typeof raw.label !== "string") return null;
  return {
    id: raw.id,
    project_name: raw.project_name,
    label: raw.label,
    due_date: typeof raw.due_date === "string" ? raw.due_date : null,
    event_uid: typeof raw.event_uid === "string" ? raw.event_uid : null,
    calendar_linked: raw.calendar_linked === true,
    event_missing: raw.event_missing === true,
    state: raw.state === "met" || raw.state === "unmet" ? raw.state : null,
    status:
      raw.status === "not_started" ||
      raw.status === "in_progress" ||
      raw.status === "almost_done" ||
      raw.status === "done"
        ? raw.status
        : null,
    source_type: null,
    external_id: null,
    sort_order: typeof raw.sort_order === "number" ? raw.sort_order : 0,
  };
}

/** One project's milestones, resolved (a linked one's date follows its event). PM lists every
 *  milestone, in pages, and the project is picked out here: the call takes no filter. */
export async function listMilestones(project: string): Promise<Milestone[]> {
  const wanted = project.trim().toLowerCase();
  const out: Milestone[] = [];
  let after = 0;
  for (let page = 0; page < MAX_PAGES; page += 1) {
    const reply = await call("milestones.list", after === 0 ? {} : { after });
    if (!isRecord(reply) || !Array.isArray(reply.milestones))
      throw new PmAnswerError("milestones.list");
    for (const raw of reply.milestones) {
      const m = readMilestone(raw);
      if (m && m.project_name.trim().toLowerCase() === wanted) out.push(m);
    }
    const next = reply.next;
    if (next === null || next === undefined) return out;
    if (!isWhole(next) || next <= after) throw new PmAnswerError("milestones.list");
    after = next;
  }
  throw new PmAnswerError("milestones.list");
}

/** The most characters of a milestone's label and of a new project's name PM takes (`MOD_LABEL_MAX`,
 *  `NEW_PROJECT_MAX`: both 200). */
const LINE_MAX = 200;

/** A milestone's label as PM takes it: one plain line of the person's words (a control or hidden
 *  character, a tab or a line break would be refused, and an emoji with its joiner is fine), at most
 *  200 characters. A label that comes to nothing is "deadline", as the page always called it. */
export function milestoneLabel(label: string): string {
  return tidyLine(label, LINE_MAX) || "deadline";
}

/** A project name as PM takes a new one: one plain line of the person's words, at most 200
 *  characters. A card is bound to this string, so it and PM's project are the same name. */
export function tidyProjectName(name: string): string {
  return tidyLine(name, LINE_MAX);
}

/** Add a milestone to a project (a name PM does not know asks the person first: PM's own question).
 *  Resolves to its id. A milestone from the Pinboard is never linked to an event, so there is no
 *  `eventUid` here, unlike core's. The label and a new project's name go through the same line rule
 *  PM holds them to, so what the person typed is never refused over a stray character. */
export async function addMilestone(
  project: string,
  label: string,
  dueDate: string | null,
): Promise<number> {
  const reply = await call("milestones.add", {
    project: tidyProjectName(project),
    label: milestoneLabel(label),
    dueDate,
  });
  if (!isRecord(reply) || !isWhole(reply.id)) throw new PmAnswerError("milestones.add");
  return reply.id;
}

/** Edit a milestone's label and (for a PM-native milestone) its date. */
export async function updateMilestone(
  id: number,
  label: string,
  dueDate: string | null,
): Promise<void> {
  await call("milestones.update", { id, label: milestoneLabel(label), dueDate });
}

export async function setMilestoneStatus(id: number, status: MilestoneStatus): Promise<void> {
  await call("milestones.setStatus", { id, status });
}

/** Delete a milestone. One the Pinboard did not make asks the person first (PM's own question). */
export async function deleteMilestone(id: number): Promise<void> {
  await call("milestones.delete", { id });
}

// --- tasks: the list card's calls (frame/taskData.ts), and checklists ------------------------------

/** One page of `tasks.list`: the tasks (each as PM answers it: `fromModTask` reads one), the stacks (the
 *  first page only) and the cursor of the next page. */
export interface TaskPage {
  tasks: unknown[];
  stacks: unknown[];
  next: string | null;
}

/** A page of one project's tasks (every task for null), after `after` (a cursor a page gave). */
export async function listTasksPage(
  project: string | null,
  after: string | null,
): Promise<TaskPage> {
  const reply = await call("tasks.list", {
    ...(project === null ? {} : { project }),
    ...(after === null ? {} : { after }),
  });
  if (
    !isRecord(reply) ||
    !Array.isArray(reply.tasks) ||
    !Array.isArray(reply.stacks) ||
    !(reply.next === null || typeof reply.next === "string")
  ) {
    throw new PmAnswerError("tasks.list");
  }
  return { tasks: reply.tasks, stacks: reply.stacks, next: reply.next };
}

/** Add a task (the quick-add row's title, project and date). */
export async function addTask(input: {
  title: string;
  project?: string | null;
  deadline?: string | null;
}): Promise<number | null> {
  const made = await call("tasks.add", {
    title: input.title,
    ...(input.project ? { project: input.project } : {}),
    ...(input.deadline ? { deadline: input.deadline } : {}),
  });
  // The new task's id (PM answers it), so the frame can tell its own add from an editor's save.
  const id = isRecord(made) ? made.id : null;
  return typeof id === "number" && Number.isInteger(id) ? id : null;
}

export async function setTaskStatus(id: number, status: "todo" | "doing" | "done"): Promise<void> {
  await call("tasks.setStatus", { id, status });
}

/** Put these tasks in this order, within their own places in the global order. */
export async function reorderTasks(ids: number[]): Promise<void> {
  await call("tasks.reorder", { ids });
}

/** Ask PM to open ITS task editor (it answers at once; the editor stays open on its own, and the
 *  list hears `tasks.changed` when a task is saved). A new task's prefill is plain text. */
export async function openTaskEditor(
  target: { id: number } | { title?: string; project?: string; deadline?: string },
): Promise<void> {
  await call("ui.openTaskEditor", "id" in target ? target : { new: target });
}

/** What a checklist import did: the ids it created and how many lines it skipped. */
export interface ImportResult {
  created: number[];
  skipped: number;
}

/** Turn checklist lines into tasks, skipping blanks and ones already in that project. */
export async function importChecklistTasks(
  project: string | null,
  items: { title: string; done: boolean }[],
): Promise<ImportResult> {
  const reply = await call("tasks.importChecklist", {
    ...(project === null ? {} : { project }),
    items,
  });
  if (
    !isRecord(reply) ||
    !Array.isArray(reply.created) ||
    !reply.created.every(isWhole) ||
    !isWhole(reply.skipped)
  ) {
    throw new PmAnswerError("tasks.importChecklist");
  }
  return { created: reply.created, skipped: reply.skipped };
}

// --- notes ----------------------------------------------------------------------------------------

/** The title core gave an untitled note: its first non-blank line, cut to 80 characters with an
 *  ellipsis, else "Untitled note" (`derive_title` in commands/archivist.rs). A line that is only
 *  hidden or control characters is blank here, as it is to the eye. */
export function deriveTitle(body: string): string {
  const line =
    body
      .split("\n")
      .map((l) => tidyLabel(l))
      .find((l) => l !== "") ?? "";
  if (line === "") return "Untitled note";
  const chars = Array.from(line);
  return chars.length > 80 ? `${chars.slice(0, 80).join("")}…` : line;
}

/** Save a note's text as a document in the person's vault, keyed on the widget id (idempotent: an
 *  edited note updates the same document, keeping its filing). PM wants a title of one plain line, so
 *  the note's own title (else the one derived from its text) goes through the shared line rule before
 *  it is sent: a title that comes to nothing falls back to the text's first line, then to "Untitled
 *  note", because PM refuses a blank one. */
export async function ingestNote(widgetId: string, title: string, text: string): Promise<void> {
  const body = text.trim();
  // One plain line: PM refuses a title with a line break, a control or hidden character, a tab, or
  // `---` (it lands in the vault file's front matter), and the person never typed any of that on purpose.
  // Tidied BEFORE the runs of hyphens are folded: a hidden character between two hyphens ("a--<zero
  // width space>-b") is only a run of three once the tidy has taken it out.
  const plain = (t: string) =>
    tidyLine(tidyLine(t, NOTE_TITLE_MAX).replace(/-{3,}/g, "-"), NOTE_TITLE_MAX);
  const reply = await call("documents.saveNote", {
    key: widgetId,
    title: plain(title) || plain(deriveTitle(body)) || "Untitled note",
    body,
  });
  if (!isRecord(reply) || reply.saved !== true) throw new PmAnswerError("documents.saveNote");
}

/** The most characters of a note's title PM takes (`NOTE_TITLE_MAX_CHARS`). */
const NOTE_TITLE_MAX = 200;

/** What a saved note's document is now: filed under a project, or waiting in Review. */
export interface NoteStatus {
  reviewed: boolean;
  project: string;
}

/** The document status of each note key that has one (a key with no document is left out, so a note
 *  whose document was deleted reads as "not ingested"). At most 500 keys go in one call. */
export async function noteStatuses(keys: string[]): Promise<Map<string, NoteStatus>> {
  const out = new Map<string, NoteStatus>();
  for (let i = 0; i < keys.length; i += 500) {
    const chunk = keys.slice(i, i + 500);
    const reply = await call("documents.statusOf", { keys: chunk });
    if (!isRecord(reply)) throw new PmAnswerError("documents.statusOf");
    for (const key of chunk) {
      const row = reply[key];
      if (!isRecord(row)) continue;
      out.set(key, {
        reviewed: row.reviewed === true,
        project: typeof row.project === "string" ? row.project : "",
      });
    }
  }
  return out;
}

// --- links and the Calendar -----------------------------------------------------------------------

/** Open an https address in the person's browser (PM refuses anything else, and says why). */
export async function openUrl(url: string): Promise<void> {
  await call("shell.openLink", { url });
}

/** Replace the Pinboard's dated entries on the Calendar with `entries` (the overlay, PM draws it). */
export async function setOverlay(entries: PinboardEntry[]): Promise<void> {
  await call("calendar.overlay.set", { entries });
}
