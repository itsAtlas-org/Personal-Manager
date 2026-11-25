// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The page and the manifest agree. The manifest is what the person is shown and what PM enforces: a call
// whose permission it lacks is refused at run time, in the user's vault, long after every test here went
// green; a key it does not declare is `content` whatever the page meant; a `projectRefs` path that misses
// a place the board stores a project name leaves a renamed project behind in the board. So this reads the
// committed manifest and the Rust dispatch table (as text) and holds the page to both.
//
// THE COVERAGE RULE (PR C plan K7): every Host method and event the page's source names, and every
// method it makes in a run through every call it has, has its row's permission declared, and its
// second-READ permissions too (a write that names a target also needs the matching reads: the table is
// `second_read.rs`'s `WRITE_RULES`, read from the Rust source here, not copied), and no permission is
// declared that nothing uses. The two lists (what the source names, what a run makes) must also be the
// same list, so a call added to the source with no test that makes it fails here.

import { afterEach, describe, expect, it } from "vitest";
import noteCallsRs from "../../../src-tauri/src/mods/api/note_calls.rs?raw";
import secondReadRs from "../../../src-tauri/src/mods/api/second_read.rs?raw";
import tableRs from "../../../src-tauri/src/mods/api/table.rs?raw";
import taskCallsRs from "../../../src-tauri/src/mods/api/task_calls.rs?raw";
import taskListTs from "../../../src/kit/tasks/TaskList.tsx?raw";
import manifest from "../manifest.json";
import { frameTaskData } from "./frame/taskData";
import { frameThemeStore } from "./frame/theme";
import { connectPm } from "./lib/pm";
import * as pm from "./lib/pm";
import { PREFS_KEY } from "./lib/pinboard/prefs";
import { PINBOARD_PREF_KEY } from "./lib/pinboard/types";
import typesTs from "./lib/pinboard/types.ts?raw";
import { memoryPm } from "./testing";

// ---- what Rust says -------------------------------------------------------------------------------

/** The dispatch table's rows (the part of the file before its tests): name to permission, or null for a
 *  row any session has, for calls and events apart. Whichever way rustfmt wrapped them, and whichever of
 *  the row constructors makes them. */
const tableBody = tableRs.slice(0, tableRs.indexOf("#[cfg(test)]"));
const callRows = new Map<string, string>(
  Array.from(
    tableBody.matchAll(/\b(?:call|family_call|experimental_call)\(\s*"([\w.]+)",\s*"([\w.]+)"/g),
  ).map((m) => [m[1], m[2]]),
);
/** Each call row's class (`Read`, `Write`, `Local`, `Egress` ...), which is what makes it a write. */
const callClass = new Map<string, string>(
  Array.from(
    tableBody.matchAll(
      /\b(?:call|family_call|experimental_call)\(\s*"([\w.]+)",\s*"[\w.]+",\s*Class::(\w+)/g,
    ),
  ).map((m) => [m[1], m[2]]),
);
const eventRows = new Map<string, string | null>(
  Array.from(tableBody.matchAll(/\bevent\(\s*"([\w.]+)",\s*(?:Some\("([\w.]+)"\)|None)/g)).map(
    (m) => [m[1], m[2] ?? null],
  ),
);

/** `second_read.rs`'s rules: a write call's reads, and the reads it needs only when it names its
 *  optional target. */
const writeRules = new Map<string, { reads: string[]; ifNamed: string[] }>(
  Array.from(secondReadRs.matchAll(/\brule\(\s*"([\w.]+)",\s*&\[([^\]]*)\],\s*&\[([^\]]*)\]/g)).map(
    (m) => {
      const list = (s: string) => Array.from(s.matchAll(/"([\w.]+)"/g)).map((x) => x[1]);
      return [m[1], { reads: list(m[2]), ifNamed: list(m[3]) }];
    },
  ),
);

const declared = new Set(manifest.permissions.map((p) => p.id));

// ---- what the page's source names ------------------------------------------------------------------

const RAW = import.meta.glob("./**/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

/** A source without its comment lines: prose that names a call or a field is not a use of it. */
const codeOf = (text: string): string =>
  text
    .split("\n")
    .filter((l) => !/^\s*(\/\/|\*|\/\*)/.test(l))
    .join("\n");

/** The page's own sources: no test, and no stand-in for PM. */
const CODE = Object.entries(RAW)
  .filter(([path]) => !/\.test\.tsx?$/.test(path) && path !== "./testing.ts")
  .map(([path, text]) => [path, codeOf(text)]) as ReadonlyArray<readonly [string, string]>;

const named = (names: Iterable<string>): string[] =>
  [...names]
    .filter((n) =>
      CODE.some(([, text]) => new RegExp(`["'\`]${n.replace(/\./g, "\\.")}["'\`]`).test(text)),
    )
    .sort();

const namedCalls = named(callRows.keys());
const namedEvents = named(eventRows.keys());

/** The calls the page makes, by name: a run through every one of them against a fake PM. */
async function tour(): Promise<string[]> {
  const everything = await memoryPm({
    answers: {
      "theme.get": {
        vars: {},
        mode: "dark",
        system: "slate",
        accent: "mono",
        colorblind: false,
        depth: "standard",
      },
      "projects.list": { projects: [], truncated: false },
      "milestones.list": { milestones: [], next: null },
      "milestones.add": { id: 1 },
      "milestones.update": { id: 1 },
      "milestones.setStatus": { id: 1, status: "done" },
      "milestones.delete": { deleted: true },
      "tasks.list": { tasks: [], stacks: [], next: null },
      "tasks.add": { id: 1, status: "todo" },
      "tasks.setStatus": { id: 1, status: "done" },
      "tasks.reorder": { reordered: 0 },
      "tasks.importChecklist": { created: [], skipped: 0 },
      "ui.openTaskEditor": { opened: true },
      "documents.saveNote": { saved: true },
      "documents.statusOf": {},
      "shell.openLink": { opened: true },
      "calendar.overlay.set": { set: 0 },
    },
  });
  const { host } = everything;
  await frameThemeStore(host).get();
  await pm.getPref(PINBOARD_PREF_KEY);
  await pm.setPref(PINBOARD_PREF_KEY, "{}");
  await pm.listProjects();
  await pm.listMilestones("Atlas");
  await pm.addMilestone("Atlas", "x", null);
  await pm.updateMilestone(1, "x", null);
  await pm.setMilestoneStatus(1, "done");
  await pm.deleteMilestone(1);
  const data = frameTaskData(host, window);
  await data.list("Atlas");
  await data.add({ title: "t", project: "Atlas", deadline: null });
  await data.setStatus(1, "done");
  await data.reorder([1]);
  data.edit({ defaults: { title: "t" } }, () => {});
  await pm.importChecklistTasks("Atlas", []);
  await pm.ingestNote("w", "t", "b");
  await pm.noteStatuses(["w"]);
  await pm.openUrl("https://example.org");
  await pm.setOverlay([]);
  await everything.settle();
  return everything.methods().sort();
}

afterEach(() => connectPm(null));

// ---- the tests -------------------------------------------------------------------------------------

describe("the dispatch table the page is held to", () => {
  it("is read: the calls and events this page needs are all rows, and the second-read rules are found", () => {
    expect(callRows.get("tasks.list")).toBe("tasks.read");
    expect(callRows.get("tasks.add")).toBe("tasks.write");
    expect(callRows.get("shell.openLink")).toBe("links");
    expect(callRows.get("storage.get")).toBe("storage");
    expect(eventRows.get("tasks.changed")).toBe("tasks.read");
    expect(eventRows.get("lifecycle.suspend")).toBeNull();
    expect(writeRules.get("milestones.add")).toEqual({ reads: ["projects.read"], ifNamed: [] });
    expect(writeRules.get("tasks.add")).toEqual({
      reads: ["tasks.read"],
      ifNamed: ["projects.read"],
    });
  });
});

/** Whether the code (comments aside) reads the fields only a holder of calendar.read is answered. */
function usesCalendarFields(): boolean {
  const lib = codeOf(RAW["./lib/pm.ts"]);
  return (
    /\.event_title\b/.test(codeOf(taskListTs)) &&
    /\braw\.event_missing\b/.test(lib) &&
    /\braw\.event_uid\b/.test(lib)
  );
}

describe("what the page does is covered by what the manifest asks for", () => {
  it("the calls the source names are the calls a run makes", async () => {
    expect(namedCalls).toEqual(await tour());
  });

  it("the calls are the nineteen the Pinboard uses, and nothing else", () => {
    expect(namedCalls).toEqual([
      "calendar.overlay.set",
      "documents.saveNote",
      "documents.statusOf",
      "milestones.add",
      "milestones.delete",
      "milestones.list",
      "milestones.setStatus",
      "milestones.update",
      "projects.list",
      "shell.openLink",
      "storage.get",
      "storage.set",
      "tasks.add",
      "tasks.importChecklist",
      "tasks.list",
      "tasks.reorder",
      "tasks.setStatus",
      "theme.get",
      "ui.openTaskEditor",
    ]);
  });

  it("the events the source names are PM's", () => {
    expect(namedEvents).toEqual(["tasks.changed", "theme.changed"]);
  });

  /** What each call needs: its row's permission, the reads a write names a target for, and the reads a
   *  call needs only because the page passes the optional target (a project name). */
  function needs(method: string): string[] {
    const need = new Set<string>();
    const row = callRows.get(method);
    expect(row, `${method} is not a call of the dispatch table`).toBeDefined();
    need.add(row as string);
    const rule = writeRules.get(method);
    for (const read of rule?.reads ?? []) need.add(read);
    // The page names a project in these (a task's, a list's filter, a checklist's).
    if (["tasks.add", "tasks.importChecklist"].includes(method)) {
      for (const read of rule?.ifNamed ?? []) need.add(read);
    }
    // Naming a project in `tasks.list` is reading about projects (task_calls.rs).
    if (method === "tasks.list") need.add("projects.read");
    return [...need];
  }

  it("every call has its permission declared, and its second-read permissions too", () => {
    for (const method of namedCalls) {
      for (const permission of needs(method)) {
        expect(declared.has(permission), `${method} needs ${permission}`).toBe(true);
      }
    }
  });

  it("every write call that names a target has a second-read rule (so its reads are counted)", () => {
    const writes = namedCalls.filter((m) => callClass.get(m) === "Write");
    expect(writes).toContain("tasks.add");
    expect(writes).toContain("milestones.delete");
    for (const method of writes) {
      // `documents.saveNote` names nothing that exists (its own namespace), so it has no rule.
      if (method === "documents.saveNote") continue;
      expect(writeRules.has(method), `${method} has no second-read rule`).toBe(true);
    }
  });

  it("every event it hears has its permission declared (a session's own word needs none)", () => {
    for (const name of namedEvents) {
      const permission = eventRows.get(name);
      expect(permission, `${name} is not an event of the dispatch table`).not.toBeUndefined();
      if (permission !== null) expect(declared.has(permission as string), name).toBe(true);
    }
  });

  it("no permission is declared that nothing uses", () => {
    const used = new Set<string>(["ui.tab"]); // the tab itself, not a call
    for (const method of namedCalls) for (const p of needs(method)) used.add(p);
    for (const name of namedEvents) {
      const p = eventRows.get(name);
      if (p) used.add(p);
    }
    // `calendar.read` gates no call: it adds fields to answers the page already asks for. It counts as
    // used only while the code (not a comment) reads those fields, and the view tests draw them.
    if (usesCalendarFields()) used.add("calendar.read");
    expect([...declared].sort()).toEqual([...used].sort());
  });

  it("calendar.read is used: the page shows the event a task is linked to, which only it grants", () => {
    // The kit's task list draws `event_title`, and the page reads `event_uid` and `event_missing` of a
    // milestone; PM's answers carry them only to a mod holding calendar.read (Bobby, 2026-11-18, Q3).
    // That they are DRAWN is held by PinboardView.bridge.test.tsx ("the calendar events a card names").
    expect(usesCalendarFields()).toBe(true);
    for (const field of ["event_title", "event_uid"]) {
      expect(taskCallsRs).toMatch(new RegExp(`Needs::Calendar,\\s*"${field}"`));
    }
    expect(manifest.permissions.find((p) => p.id === "calendar.read")?.why).toMatch(/event/i);
  });

  it("each permission says why in the person's words, once", () => {
    expect(new Set(manifest.permissions.map((p) => p.id)).size).toBe(manifest.permissions.length);
    for (const p of manifest.permissions) expect(p.why.length, p.id).toBeGreaterThan(10);
  });

  it("a note's saved key is what statusOf reads back: both name the caller's own notes", () => {
    // documents.statusOf and saveNote are the one namespace (note_calls.rs): the legacy mapping for the
    // first-party Pinboard. The page uses the widget id as the key for both.
    expect(noteCallsRs).toContain('PINBOARD_ID: &str = "atlas.pinboard"');
  });
});

describe("the data the manifest declares", () => {
  const keys = Object.fromEntries(manifest.data.keys.map((k) => [k.key, k.class]));

  it("is the board and the preferences, and the board's copy from before the move, all content", () => {
    expect(keys).toEqual({
      [PINBOARD_PREF_KEY]: "content",
      [PREFS_KEY]: "content",
      "board-before-move": "content",
    });
  });

  it("every key is one PM's grammar accepts (a lowercase letter, then letters, digits, _ . -)", () => {
    for (const key of Object.keys(keys)) expect(key).toMatch(/^[a-z][a-z0-9_.-]{0,63}$/);
  });

  it("the page reads and writes only the keys it declares", () => {
    const used = new Set<string>([PINBOARD_PREF_KEY, PREFS_KEY]);
    for (const [path, text] of CODE) {
      for (const m of text.matchAll(/\b(?:getPref|setPref)\(\s*["'`]([^"'`]+)["'`]/g))
        used.add(m[1]);
      expect(text, path).not.toMatch(/storage\.doc\./);
    }
    for (const key of used) expect(keys[key], key).toBe("content");
  });
});

describe("projectRefs", () => {
  const refs = manifest.data.projectRefs;

  /** The strings a path reaches in `value`: the grammar of `refs.rs` (`$`, `.key`, `[*]`, and `..key`,
   *  a member of the current value or of anything under it, down to eight levels, an array a level). */
  function reach(value: unknown, path: string): string[] {
    const steps = path.slice(1).match(/\.\.[A-Za-z0-9_-]+|\.[A-Za-z0-9_-]+|\[\*\]/g) ?? [];
    const member = (v: unknown, key: string): unknown[] =>
      typeof v === "object" && v !== null && !Array.isArray(v) && key in v
        ? [(v as Record<string, unknown>)[key]]
        : [];
    const under = (v: unknown, key: string, level: number): unknown[] => {
      if (level > 8 || typeof v !== "object" || v === null) return [];
      const kids = Array.isArray(v) ? v : Object.values(v);
      return [...member(v, key), ...kids.flatMap((k) => under(k, key, level + 1))];
    };
    let level: unknown[] = [value];
    for (const step of steps) {
      level = level.flatMap((v) => {
        if (step === "[*]") return Array.isArray(v) ? v : [];
        if (step.startsWith("..")) return under(v, step.slice(2), 0);
        return member(v, step.slice(1));
      });
    }
    return level.filter((v): v is string => typeof v === "string");
  }

  it("names the board, and reaches the project of a card on the board and of a card inside a folder, however deep (core's rename and unbind went through every level)", () => {
    expect(refs.every((r) => r.key === PINBOARD_PREF_KEY)).toBe(true);
    const board = {
      version: 1,
      widgets: [
        { id: "t", kind: "timeline", project: "Atlas" },
        { id: "l", kind: "tasklist", project: "Home" },
        { id: "n", kind: "note" },
        {
          id: "f",
          kind: "folder",
          children: [
            { id: "c", kind: "timeline", project: "Deep" },
            // A folder in a folder is never drawn, but it is kept, and its cards' names follow a rename.
            {
              id: "g",
              kind: "folder",
              children: [{ id: "d", kind: "timeline", project: "Deeper" }],
            },
          ],
        },
      ],
    };
    const found = refs.flatMap((r) => reach(board, r.path)).sort();
    expect(found).toEqual(["Atlas", "Deep", "Deeper", "Home"]);
  });

  it("covers every field of a card that holds a project name (the board's own type says which)", () => {
    const widget = typesTs.slice(
      typesTs.indexOf("export interface Widget"),
      typesTs.indexOf("export interface Board"),
    );
    const fields = Array.from(widget.matchAll(/^ {2}(\w+)\??:/gm)).map((m) => m[1]);
    expect(fields.filter((f) => /project/i.test(f))).toEqual(["project"]);
  });
});

describe("the contributions", () => {
  it("are a tab, a calendar overlay and help, and nothing else", () => {
    expect(Object.keys(manifest.contributions).sort()).toEqual([
      "calendar.overlay",
      "help",
      "nav.tab",
    ]);
    expect(manifest.contributions["nav.tab"].label.length).toBeLessThanOrEqual(24);
    expect(manifest.contributions["calendar.overlay"].label.length).toBeLessThanOrEqual(40);
    expect(manifest.description.length).toBeLessThanOrEqual(200);
  });

  it("asks for no internet host and no secret", () => {
    expect("internetHosts" in manifest).toBe(false);
    expect("secrets" in manifest).toBe(false);
  });
});

describe("the help entries", () => {
  const help = manifest.contributions.help;
  const used = new Set(
    CODE.flatMap(([, text]) =>
      Array.from(text.matchAll(/["'`](pinboard-[a-z-]+)["'`]/g)).map((m) => m[1]),
    ),
  );

  it("are the pinboard-* entries core's help had: each one the board's own sections point at, and no other", () => {
    expect(help.map((h) => h.id).sort()).toEqual([...used].sort());
    expect(help).toHaveLength(19);
  });

  it("follow package.rs's rules for a help entry", () => {
    expect(help.length).toBeLessThanOrEqual(64);
    expect(new Set(help.map((h) => h.id)).size).toBe(help.length);
    for (const h of help) {
      expect(h.id, h.id).toMatch(/^[a-z][a-z0-9-]{0,47}$/);
      for (const [what, text, max, newlines] of [
        ["title", h.title, 80, false],
        ["body", h.body, 2000, true],
      ] as const) {
        expect(text.length, `${h.id} ${what}`).toBeGreaterThan(0);
        expect(Array.from(text).length, `${h.id} ${what}`).toBeLessThanOrEqual(max);
        expect(text.trim(), `${h.id} ${what}`).toBe(text);
        const controls = (c: number) =>
          c < 32 || (c >= 127 && c <= 159) || c === 0x2028 || c === 0x2029;
        const invisible = (c: number) =>
          (c >= 0x200b && c <= 0x200f) ||
          (c >= 0x202a && c <= 0x202e) ||
          (c >= 0x2060 && c <= 0x206f) ||
          c === 0xfeff;
        for (const ch of text) {
          const c = ch.codePointAt(0) as number;
          expect(
            controls(c) && !(newlines && c === 10),
            `${h.id} ${what} has a control character`,
          ).toBe(false);
          expect(invisible(c), `${h.id} ${what} has an invisible character`).toBe(false);
        }
      }
    }
  });

  it("say what is so in the mod: a new project name asks first, and nothing sends the person to PM's Settings for a Pinboard control", () => {
    const all = help.map((h) => `${h.title}\n${h.body}`).join("\n");
    const project = help.find((h) => h.id === "pinboard-timeline-project")?.body ?? "";
    expect(project).toMatch(/PM asks you first before it creates a new project/);
    // New copy has no em dash.
    expect(project).not.toContain("\u2014");
    expect(all).not.toMatch(/becomes a real project when you add its first milestone/);
    expect(all).not.toMatch(/\bin Settings\b/);
  });
});
