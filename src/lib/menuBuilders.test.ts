// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The pure item builders behind R3's right-click menus (documents, milestones, projects,
// conversations, review rows, chat answers): which items each state offers, in what order, and that
// an item the surface doesn't offer is left out rather than greyed. Each item's handler is the
// surface's own, so a builder never does the work itself; what the menus do is tested per surface.

import { afterEach, describe, expect, it, vi } from "vitest";
import type { MenuItem } from "../kit/lib/contextMenu";
import { chatMessageMenuItems } from "./chatMessageMenu";
import { conversationMenuItems } from "./conversationMenu";
import { documentMenuItems, documentMenuLabel } from "./documentMenu";
import { milestoneMenuItems } from "./milestoneMenu";
import { projectMenuItems } from "./projectMenu";
import { reviewRowMenuItems } from "./reviewMenu";
import type { Document } from "./types";

const copy = vi.hoisted(() => vi.fn());
vi.mock("../kit/lib/contextMenu", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  copyViaHost: copy,
}));
vi.mock("./ipc", () => ({ openSource: vi.fn(async () => {}) }));

afterEach(() => vi.clearAllMocks());

/** The labels, with a separator as "—". */
const names = (items: MenuItem[]) =>
  items.map((i) => (i.kind === "separator" ? "—" : i.kind === "swatches" ? i.label : i.label));
const item = (items: MenuItem[], label: string) =>
  items.find(
    (i) => i.kind !== "separator" && i.kind !== "swatches" && i.label === label,
  ) as Extract<MenuItem, { label: string; onSelect: () => void }>;

function doc(over: Partial<Document> = {}): Document {
  return {
    id: 1,
    title: "Pitch deck",
    project: "Atlas",
    linked_projects: [],
    tags: [],
    importance: null,
    source_type: "vault",
    source_state: "ok",
    external_ref: null,
    source_id: null,
    chunk_count: 3,
    ...over,
  } as unknown as Document;
}

describe("documentMenuItems", () => {
  const h = { open: vi.fn(), askDelete: vi.fn() };

  it("a vault file offers Open, Copy title and Delete…, with Delete… last and no source items", () => {
    const items = documentMenuItems(doc(), h);
    expect(names(items).filter((n) => n !== "—")).toEqual(["Open", "Copy title", "Delete…"]);
    const last = items[items.length - 1] as { label: string; danger?: boolean };
    expect(last).toMatchObject({ label: "Delete…", danger: true });
  });

  it("Delete… asks and does nothing else", () => {
    const askDelete = vi.fn();
    item(documentMenuItems(doc(), { ...h, askDelete }), "Delete…").onSelect();
    expect(askDelete).toHaveBeenCalledOnce();
  });

  it("an indexed page offers Open source and Copy link; a local file says Reveal in file manager", () => {
    const web = doc({
      source_type: "index_only",
      external_ref: "https://x.test/p",
      source_id: "gdrive:1",
    });
    expect(names(documentMenuItems(web, h))).toEqual(
      expect.arrayContaining(["Open source", "Copy link"]),
    );
    item(documentMenuItems(web, h), "Copy link").onSelect();
    expect(copy).toHaveBeenCalledWith("https://x.test/p", "Link copied");

    const local = doc({
      source_type: "index_only",
      external_ref: "/home/a/b.pdf",
      source_id: "local:9",
    });
    const labels = names(documentMenuItems(local, h));
    expect(labels).toContain("Reveal in file manager");
    expect(labels).not.toContain("Open source");
    expect(labels).not.toContain("Copy link"); // a path is not a link
  });

  it("leaves Open source out for a source that is gone", () => {
    const gone = doc({
      source_type: "index_only",
      external_ref: "https://x.test",
      source_state: "source_missing",
    });
    expect(names(documentMenuItems(gone, h))).not.toContain("Open source");
  });

  it("offers Open project only with a handler and a real project", () => {
    const openProject = vi.fn();
    expect(names(documentMenuItems(doc(), { ...h, openProject }))).toContain("Open project Atlas");
    item(documentMenuItems(doc(), { ...h, openProject }), "Open project Atlas").onSelect();
    expect(openProject).toHaveBeenCalledWith("Atlas");
    expect(names(documentMenuItems(doc(), h))).not.toContain("Open project Atlas");
    expect(
      names(documentMenuItems(doc({ project: "Unsorted" }), { ...h, openProject })),
    ).not.toContain("Open project Unsorted");
  });

  it("offers the editor and Import fully only where the row does", () => {
    const sheet = doc({
      source_type: "index_only",
      external_ref: "https://docs.google.com/spreadsheets/d/1",
    });
    const edit = vi.fn();
    const importFully = vi.fn();
    const full = names(documentMenuItems(sheet, { ...h, edit, importFully }));
    expect(full).toEqual(expect.arrayContaining(["Change project or importance", "Import fully"]));
    const bare = names(documentMenuItems(sheet, h));
    expect(bare).not.toContain("Change project or importance");
    expect(bare).not.toContain("Import fully");
    // A plain page is not a spreadsheet.
    expect(
      names(
        documentMenuItems(doc({ source_type: "index_only", external_ref: "https://x.test" }), {
          ...h,
          importFully,
        }),
      ),
    ).not.toContain("Import fully");
  });

  it("names a chat as a chat", () => {
    expect(documentMenuLabel(doc({ source_type: "chat", title: "Q3" }))).toBe("Chat Q3");
    expect(documentMenuLabel(doc())).toBe("Document Pitch deck");
    expect(names(documentMenuItems(doc({ source_type: "chat" }), h))[0]).toBe("Open chat");
  });
});

describe("milestoneMenuItems", () => {
  const h = {
    setStatus: vi.fn(),
    onMove: vi.fn(),
    isFirst: false,
    isLast: false,
    askUnlink: vi.fn(),
    askRemove: vi.fn(),
  };

  it("ticks the current progress and does nothing for choosing it again", () => {
    const items = milestoneMenuItems({ calendar_linked: false }, "in_progress", h);
    expect(items.filter((i) => i.kind !== "separator" && "checked" in i && i.checked)).toHaveLength(
      1,
    );
    item(items, "In progress").onSelect();
    expect(h.setStatus).not.toHaveBeenCalled();
    item(items, "Done").onSelect();
    expect(h.setStatus).toHaveBeenCalledWith("done");
  });

  it("offers Unlink from calendar… only for a linked milestone, and Remove… always, last", () => {
    const linked = milestoneMenuItems({ calendar_linked: true }, "not_started", h);
    expect(names(linked)).toContain("Unlink from calendar…");
    expect(names(linked)[names(linked).length - 1]).toBe("Remove…");
    expect(names(milestoneMenuItems({ calendar_linked: false }, "not_started", h))).not.toContain(
      "Unlink from calendar…",
    );
  });

  it("Remove… and Unlink… only ask", () => {
    const items = milestoneMenuItems({ calendar_linked: true }, "not_started", h);
    item(items, "Remove…").onSelect();
    item(items, "Unlink from calendar…").onSelect();
    expect(h.askRemove).toHaveBeenCalledOnce();
    expect(h.askUnlink).toHaveBeenCalledOnce();
  });

  it("shows the arrows only in manual order and disables them at the ends", () => {
    const first = milestoneMenuItems({ calendar_linked: false }, "done", { ...h, isFirst: true });
    expect(item(first, "Move up").disabled).toBe(true);
    expect(item(first, "Move down").disabled).toBeFalsy();
    const sorted = milestoneMenuItems({ calendar_linked: false }, "done", {
      ...h,
      onMove: undefined,
    });
    expect(names(sorted)).not.toContain("Move up");
  });
});

describe("projectMenuItems", () => {
  const all = {
    open: vi.fn(),
    triage: vi.fn(),
    archive: vi.fn(),
    unarchive: vi.fn(),
    merge: vi.fn(),
    askDelete: vi.fn(),
  };

  it("a Focus card: Open, Triage, Archive, Merge into…, then Delete… last", () => {
    expect(names(projectMenuItems(all)).filter((n) => n !== "—")).toEqual([
      "Open",
      "Triage",
      "Archive",
      "Merge into…",
      "Delete…",
    ]);
    expect(names(projectMenuItems(all, { triageOpen: true }))).toContain("Close triage");
  });

  it("leaves Merge into… out when there is nothing to merge into", () => {
    expect(names(projectMenuItems({ ...all, merge: undefined }))).not.toContain("Merge into…");
  });

  it("an archived row: Open and Unarchive only", () => {
    expect(
      names(
        projectMenuItems({ open: all.open, unarchive: all.unarchive }, { archived: true }),
      ).filter((n) => n !== "—"),
    ).toEqual(["Open", "Unarchive"]);
  });

  it("a sidebar entry: Open and Archive", () => {
    expect(
      names(projectMenuItems({ open: all.open, archive: all.archive })).filter((n) => n !== "—"),
    ).toEqual(["Open", "Archive"]);
  });

  it("Delete… and Merge into… only ask", () => {
    const items = projectMenuItems(all);
    item(items, "Delete…").onSelect();
    item(items, "Merge into…").onSelect();
    expect(all.askDelete).toHaveBeenCalledOnce();
    expect(all.merge).toHaveBeenCalledOnce();
  });
});

describe("conversationMenuItems", () => {
  const h = { open: vi.fn(), askMove: vi.fn(), askDelete: vi.fn() };

  it("a global chat can be renamed", () => {
    const items = conversationMenuItems({ ...h, rename: vi.fn() });
    expect(names(items).filter((n) => n !== "—")).toEqual([
      "Open",
      "Rename",
      "Move to project…",
      "Delete…",
    ]);
  });

  it("a project chat has no Rename", () => {
    expect(names(conversationMenuItems(h))).not.toContain("Rename");
  });

  it("Delete… is last, and only asks", () => {
    const items = conversationMenuItems(h);
    expect(items[items.length - 1]).toMatchObject({ label: "Delete…", danger: true });
    item(items, "Delete…").onSelect();
    expect(h.askDelete).toHaveBeenCalledOnce();
  });
});

describe("reviewRowMenuItems", () => {
  it("offers Approve and the folder action only where the row does", () => {
    const open = vi.fn();
    const full = reviewRowMenuItems("A.pdf", {
      open,
      approve: vi.fn(),
      applyFolder: vi.fn(),
      applyCount: 4,
    });
    expect(names(full).filter((n) => n !== "—")).toEqual([
      "Open",
      "Approve",
      "Apply this filing to 4 other files…",
      "Copy title",
    ]);
    const held = reviewRowMenuItems("A.pdf", { open, applyCount: 4 });
    expect(names(held).filter((n) => n !== "—")).toEqual(["Open", "Copy title"]);
    expect(names(reviewRowMenuItems("A", { open, applyFolder: vi.fn(), applyCount: 1 }))).toContain(
      "Apply this filing to 1 other file…",
    );
  });

  it("has no Delete, and Copy title speaks", () => {
    const items = reviewRowMenuItems("A.pdf", { open: vi.fn(), approve: vi.fn(), applyCount: 0 });
    expect(names(items).some((n) => /delete/i.test(n))).toBe(false);
    item(items, "Copy title").onSelect();
    expect(copy).toHaveBeenCalledWith("A.pdf", "Title copied");
  });
});

describe("chatMessageMenuItems", () => {
  it("an answer with sources: Copy answer, Save as note and the rating, ticked", () => {
    const save = vi.fn();
    const choose = vi.fn();
    const items = chatMessageMenuItems("the answer", { save, rate: { current: "up", choose } });
    expect(names(items).filter((n) => n !== "—")).toEqual([
      "Copy answer",
      "Save as note",
      "This answer was helpful",
      "This answer missed",
    ]);
    expect(item(items, "This answer was helpful").checked).toBe(true);
    expect(item(items, "This answer missed").checked).toBe(false);
    item(items, "Copy answer").onSelect();
    expect(copy).toHaveBeenCalledWith("the answer", "Answer copied");
    item(items, "This answer missed").onSelect();
    expect(choose).toHaveBeenCalledWith("down");
    item(items, "Save as note").onSelect();
    expect(save).toHaveBeenCalledOnce();
  });

  it("leaves Save as note and the rating out where the answer shows neither", () => {
    expect(names(chatMessageMenuItems("x", {}))).toEqual(["Copy answer"]);
  });
});
