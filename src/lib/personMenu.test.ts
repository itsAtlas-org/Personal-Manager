// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The People row's right-click menu: which items each state offers, in what order, and that each one
// only calls the tab's own handler.

import { describe, expect, it, vi } from "vitest";
import type { MenuItem } from "../kit/lib/contextMenu";
import { personMenuItems, personMenuLabel, type PersonMenuState } from "./personMenu";

const names = (items: MenuItem[]) =>
  items.map((i) => (i.kind === "separator" ? "—" : i.kind === "swatches" ? i.label : i.label));
const pick = (items: MenuItem[], label: string) =>
  items.find(
    (i) => i.kind !== "separator" && i.kind !== "swatches" && i.label === label,
  ) as Extract<MenuItem, { label: string; onSelect: () => void }>;

function handlers() {
  return {
    open: vi.fn(),
    edit: vi.fn(),
    star: vi.fn(),
    toggleLabel: vi.fn(),
    newLabel: vi.fn(),
    trash: vi.fn(),
    restore: vi.fn(),
    askDeleteNow: vi.fn(),
  };
}

const LABELS = [
  { id: 1, name: "Family" },
  { id: 2, name: "Climbing" },
];

function state(over: Partial<PersonMenuState> = {}): PersonMenuState {
  return {
    count: 1,
    trashed: false,
    allStarred: false,
    labels: LABELS,
    carriedByAll: new Set([2]),
    ...over,
  };
}

describe("personMenuItems", () => {
  it("offers Open, Edit…, Star, one radio per label, New label… and Move to Trash last", () => {
    const items = personMenuItems(state(), handlers());
    expect(names(items)).toEqual([
      "Open",
      "Edit…",
      "Star",
      "—",
      "Label: Family",
      "Label: Climbing",
      "New label…",
      "—",
      "Move to Trash",
    ]);
    expect(items[items.length - 1]).toMatchObject({ label: "Move to Trash", danger: true });
  });

  it("ticks the labels everyone carries", () => {
    const items = personMenuItems(state(), handlers());
    expect(pick(items, "Label: Family").checked).toBe(false);
    expect(pick(items, "Label: Climbing").checked).toBe(true);
  });

  it("each item calls the tab's handler and does nothing itself", () => {
    const h = handlers();
    const items = personMenuItems(state(), h);
    pick(items, "Open").onSelect();
    pick(items, "Edit…").onSelect();
    pick(items, "Star").onSelect();
    pick(items, "Label: Family").onSelect();
    pick(items, "Label: Climbing").onSelect();
    pick(items, "New label…").onSelect();
    pick(items, "Move to Trash").onSelect();
    expect(h.open).toHaveBeenCalledOnce();
    expect(h.edit).toHaveBeenCalledOnce();
    expect(h.star).toHaveBeenCalledWith(true);
    expect(h.toggleLabel).toHaveBeenNthCalledWith(1, 1, true);
    expect(h.toggleLabel).toHaveBeenNthCalledWith(2, 2, false);
    expect(h.newLabel).toHaveBeenCalledOnce();
    expect(h.trash).toHaveBeenCalledOnce();
    expect(h.restore).not.toHaveBeenCalled();
    expect(h.askDeleteNow).not.toHaveBeenCalled();
  });

  it("says Unstar for someone already starred", () => {
    const h = handlers();
    const items = personMenuItems(state({ allStarred: true }), h);
    expect(names(items)).toContain("Unstar");
    expect(names(items)).not.toContain("Star");
    pick(items, "Unstar").onSelect();
    expect(h.star).toHaveBeenCalledWith(false);
  });

  it("leaves Open out for a selection of several and acts on them all", () => {
    const items = personMenuItems(state({ count: 3 }), handlers());
    expect(names(items)).not.toContain("Open");
    expect(names(items)).not.toContain("Edit…");
    expect(names(items)[0]).toBe("Star");
  });

  it("offers only Restore and Delete now… in Trash, Delete now… last", () => {
    const h = handlers();
    const items = personMenuItems(state({ trashed: true }), h);
    expect(names(items)).toEqual(["Open", "Restore", "—", "Delete now…"]);
    expect(items[items.length - 1]).toMatchObject({ danger: true });
    pick(items, "Restore").onSelect();
    pick(items, "Delete now…").onSelect();
    expect(h.restore).toHaveBeenCalledOnce();
    expect(h.askDeleteNow).toHaveBeenCalledOnce();
  });

  it("with no labels yet, still offers New label…", () => {
    const items = personMenuItems(state({ labels: [], carriedByAll: new Set() }), handlers());
    expect(names(items)).toEqual([
      "Open",
      "Edit…",
      "Star",
      "—",
      "New label…",
      "—",
      "Move to Trash",
    ]);
  });
});

describe("personMenuLabel", () => {
  it("names the person, or the selection", () => {
    expect(personMenuLabel("Ada Lovelace", 1)).toBe("Person Ada Lovelace");
    expect(personMenuLabel("Ada Lovelace", 3)).toBe("People: 3 selected");
  });
});
