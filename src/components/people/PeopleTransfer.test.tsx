// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The vCard import and export dialogs (Contacts, slice 6b): the preview's lines, the label option,
// commit and its result, people back from Trash, a stale preview, a cancelled picker, export with and
// without the timeline, and Export in the selection bar and the row menu using the ticked people.
// Invented people and files only.

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  PeopleFilter,
  PeopleImportPreview,
  PeopleImportResult,
  PersonLabel,
  PersonRow,
} from "../../lib/types";

const ipc = vi.hoisted(() => ({
  listPeople: vi.fn(),
  listLabels: vi.fn(),
  pickVcardAndPreview: vi.fn(),
  commitVcardImport: vi.fn(),
  discardVcardPreview: vi.fn(),
  exportPeople: vi.fn(),
  peopleDuplicatesSnapshot: vi.fn(),
}));
vi.mock("../../lib/ipc", () => {
  const known: Record<string, unknown> = {
    ...ipc,
    onPeopleChanged: vi.fn(async () => () => {}),
    getPref: vi.fn(async () => null),
    setPref: vi.fn(async () => {}),
  };
  return new Proxy(known, {
    has: () => true,
    get(target, prop: string) {
      if (prop === "then") return undefined;
      if (!(prop in target)) {
        target[prop] = /^on[A-Z]/.test(prop) ? vi.fn(async () => () => {}) : vi.fn(async () => []);
      }
      return target[prop];
    },
  });
});

import { ExportPeopleDialog } from "./ExportPeopleDialog";
import { ImportVcardDialog } from "./ImportVcardDialog";
import { PeopleView } from "./PeopleView";
import { ThemeProvider } from "../../theme";
import { ContextMenuHost } from "../ui/ContextMenuHost";
import { closeContextMenu } from "../../lib/contextMenu";
import { selectionExportChoice } from "../../lib/peopleTransfer";

const STALE = "That file was read a while ago, or PM has changed since. Pick the file again.";
const TOO_BIG =
  "This file is over 128 MB, almost always because of photos. Export again without photos and import that.";

function preview(over: Partial<PeopleImportPreview> = {}): PeopleImportPreview {
  return {
    preview_id: "prev-1",
    file_name: "friends.vcf",
    people: 5,
    already: 0,
    from_trash: 0,
    look_alike: 0,
    no_name: 0,
    repeated: 0,
    photos_skipped: false,
    complete: true,
    warnings: [],
    ignored: [],
    ...over,
  };
}

function result(over: Partial<PeopleImportResult> = {}): PeopleImportResult {
  return { added: 5, updated: 0, unchanged: 0, restored: [], kept: [], review: 0, ...over };
}

const LABELS: PersonLabel[] = [
  { id: 1, uid: "l1".padEnd(32, "0"), name: "Friends", people: 1 },
  { id: 2, uid: "l2".padEnd(32, "0"), name: "Climbing", people: 0 },
];

function row(id: number, name: string, over: Partial<PersonRow> = {}): PersonRow {
  return {
    id,
    uid: String(id).padStart(32, "0"),
    display_name: name,
    primary: null,
    company: null,
    labels: [],
    starred: false,
    last_met: null,
    google_state: null,
    trashed_at: null,
    birthday: null,
    leap_day: null,
    ...over,
  };
}
const PEOPLE = [
  row(1, "Ada Example", { labels: [{ id: 1, name: "Friends" }] }),
  row(2, "Ben Sample"),
  row(3, "Cal Placeholder"),
];

function answer(filter: PeopleFilter): PersonRow[] {
  if (filter === "Trash") return [];
  if (filter === "All") return PEOPLE;
  if (typeof filter === "object")
    return PEOPLE.filter((r) => r.labels.some((l) => l.id === filter.Label));
  return PEOPLE;
}

function wrap(node: React.ReactNode) {
  return render(
    <ThemeProvider>
      {node}
      <ContextMenuHost />
    </ThemeProvider>,
  );
}

beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
  Element.prototype.scrollIntoView ??= () => {};
});

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  ipc.listPeople.mockImplementation(async (f: PeopleFilter) => answer(f));
  ipc.listLabels.mockResolvedValue(LABELS);
  ipc.peopleDuplicatesSnapshot.mockResolvedValue(null);
  ipc.discardVcardPreview.mockResolvedValue(undefined);
});

afterEach(() => {
  act(() => closeContextMenu());
  cleanup();
});

async function importDialog(props: Partial<React.ComponentProps<typeof ImportVcardDialog>> = {}) {
  const onClose = vi.fn();
  const onReview = vi.fn();
  wrap(<ImportVcardDialog labels={LABELS} onClose={onClose} onReview={onReview} {...props} />);
  await act(async () => {});
  return { onClose, onReview };
}

describe("the import dialog's preview", () => {
  it("opens the picker once and heads the preview with the count and the file", async () => {
    ipc.pickVcardAndPreview.mockResolvedValue(preview());
    await importDialog();
    expect(ipc.pickVcardAndPreview).toHaveBeenCalledOnce();
    expect(screen.getByRole("dialog", { name: "5 people in friends.vcf" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Import" })).toBeTruthy();
  });

  it("says 1 person for a file of one", async () => {
    ipc.pickVcardAndPreview.mockResolvedValue(preview({ people: 1 }));
    await importDialog();
    expect(screen.getByRole("dialog", { name: "1 person in friends.vcf" })).toBeTruthy();
  });

  it("shows each count in the spec's words", async () => {
    ipc.pickVcardAndPreview.mockResolvedValue(
      preview({
        people: 20,
        already: 4,
        look_alike: 2,
        no_name: 12,
        repeated: 1,
        photos_skipped: true,
      }),
    );
    await importDialog();
    for (const words of [
      "4 are already in PM. PM fills in what's missing and keeps what you have.",
      "2 look like people you have. You can merge them after the import.",
      "12 cards had no name, email or phone and were skipped.",
      "1 card repeated an earlier card's id, so it was added as a new person.",
      "Photos are skipped.",
    ]) {
      expect(screen.getByText(words), words).toBeTruthy();
    }
  });

  it("says what was left out, by count", async () => {
    ipc.pickVcardAndPreview.mockResolvedValue(
      preview({
        ignored: [{ name: "GEO", count: 3 }],
        warnings: [{ kind: "value_cut", count: 2 }],
      }),
    );
    await importDialog();
    expect(screen.getByText(/map positions \(3\)/)).toBeTruthy();
    expect(screen.getByText("2 very long values were cut short.")).toBeTruthy();
  });

  it("renders a hostile file name as text", async () => {
    const name = "<img src=x onerror=alert(1)>.vcf";
    ipc.pickVcardAndPreview.mockResolvedValue(preview({ file_name: name }));
    await importDialog();
    expect(screen.getByRole("dialog", { name: `5 people in ${name}` })).toBeTruthy();
    expect(document.querySelector("img")).toBeNull();
  });

  it("closes without a word when the picker is cancelled", async () => {
    ipc.pickVcardAndPreview.mockResolvedValue(null);
    const { onClose } = await importDialog();
    expect(onClose).toHaveBeenCalledOnce();
    expect(ipc.commitVcardImport).not.toHaveBeenCalled();
  });

  it("can be closed while the picker is out, and drops a late answer", async () => {
    let late: (p: PeopleImportPreview) => void = () => {};
    ipc.pickVcardAndPreview.mockReturnValue(new Promise((r) => (late = r)));
    const { onClose } = await importDialog();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalledOnce();
    // The caller drops the dialog; the picker answering afterwards changes nothing.
    cleanup();
    await act(async () => late(preview()));
    expect(onClose).toHaveBeenCalledOnce();
    // The file it read after all is let go of at once: nothing will ever import it.
    expect(ipc.discardVcardPreview).toHaveBeenCalledExactlyOnceWith("prev-1");
  });

  it("forgets the file when the dialog closes without importing", async () => {
    // Cancel, Escape and the backdrop all end here; so does the dialog simply going away.
    ipc.pickVcardAndPreview.mockResolvedValue(preview());
    const first = await importDialog();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(first.onClose).toHaveBeenCalledOnce();
    expect(ipc.discardVcardPreview).toHaveBeenCalledExactlyOnceWith("prev-1");
    // Once: going away after that is nothing more to say.
    cleanup();
    expect(ipc.discardVcardPreview).toHaveBeenCalledOnce();

    ipc.discardVcardPreview.mockClear();
    ipc.pickVcardAndPreview.mockResolvedValue(preview({ preview_id: "prev-2" }));
    await importDialog();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(ipc.discardVcardPreview).toHaveBeenCalledExactlyOnceWith("prev-2");
    cleanup();

    ipc.discardVcardPreview.mockClear();
    ipc.pickVcardAndPreview.mockResolvedValue(preview({ preview_id: "prev-3" }));
    await importDialog();
    cleanup();
    expect(ipc.discardVcardPreview).toHaveBeenCalledExactlyOnceWith("prev-3");
  });

  it("does not discard when the picker is cancelled, or after a file was imported", async () => {
    ipc.pickVcardAndPreview.mockResolvedValue(null);
    await importDialog();
    cleanup();
    expect(ipc.discardVcardPreview).not.toHaveBeenCalled();

    ipc.pickVcardAndPreview.mockResolvedValue(preview());
    ipc.commitVcardImport.mockResolvedValue(result());
    await importDialog();
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await act(async () => {});
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    cleanup();
    expect(ipc.discardVcardPreview).not.toHaveBeenCalled();
  });

  it("keeps the file while an import fails for another reason, and lets it go on Cancel", async () => {
    ipc.pickVcardAndPreview.mockResolvedValue(preview());
    ipc.commitVcardImport.mockRejectedValueOnce("Something broke.");
    await importDialog();
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await act(async () => {});
    expect(ipc.discardVcardPreview).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(ipc.discardVcardPreview).toHaveBeenCalledExactlyOnceWith("prev-1");
  });

  it("says the too-big message plainly and offers to pick again", async () => {
    ipc.pickVcardAndPreview.mockRejectedValueOnce(TOO_BIG);
    await importDialog();
    expect(screen.getByText(TOO_BIG)).toBeTruthy();
    ipc.pickVcardAndPreview.mockResolvedValueOnce(preview({ people: 2 }));
    fireEvent.click(screen.getByRole("button", { name: "Pick the file again" }));
    await act(async () => {});
    expect(screen.getByRole("dialog", { name: "2 people in friends.vcf" })).toBeTruthy();
  });

  it("cannot import a file with nobody in it", async () => {
    ipc.pickVcardAndPreview.mockResolvedValue(preview({ people: 0, no_name: 3 }));
    await importDialog();
    expect((screen.getByRole("button", { name: "Import" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
  });
});

describe("the label option", () => {
  it("is off by default, so no label is sent", async () => {
    ipc.pickVcardAndPreview.mockResolvedValue(preview());
    ipc.commitVcardImport.mockResolvedValue(result());
    await importDialog();
    const tick = screen.getByRole("checkbox", {
      name: "Label everyone imported:",
    }) as HTMLInputElement;
    expect(tick.checked).toBe(false);
    expect(screen.queryByRole("combobox")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await act(async () => {});
    expect(ipc.commitVcardImport).toHaveBeenCalledWith("prev-1", { label: null });
  });

  it("sends an existing label's name when one is picked", async () => {
    ipc.pickVcardAndPreview.mockResolvedValue(preview());
    ipc.commitVcardImport.mockResolvedValue(result());
    await importDialog();
    fireEvent.click(screen.getByRole("checkbox", { name: "Label everyone imported:" }));
    fireEvent.change(screen.getByRole("combobox", { name: "Label for everyone imported" }), {
      target: { value: "2" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await act(async () => {});
    expect(ipc.commitVcardImport).toHaveBeenCalledWith("prev-1", { label: "Climbing" });
  });

  it("makes a new label from a typed name, and waits until there is one", async () => {
    ipc.pickVcardAndPreview.mockResolvedValue(preview());
    ipc.commitVcardImport.mockResolvedValue(result());
    await importDialog();
    fireEvent.click(screen.getByRole("checkbox", { name: "Label everyone imported:" }));
    fireEvent.change(screen.getByRole("combobox", { name: "Label for everyone imported" }), {
      target: { value: "new" },
    });
    const go = screen.getByRole("button", { name: "Import" }) as HTMLButtonElement;
    expect(go.disabled).toBe(true);
    fireEvent.change(screen.getByRole("textbox", { name: "Name of the new label" }), {
      target: { value: "  Conference 2026 " },
    });
    expect(screen.getByText("PM makes this label.")).toBeTruthy();
    expect(go.disabled).toBe(false);
    fireEvent.click(go);
    await act(async () => {});
    expect(ipc.commitVcardImport).toHaveBeenCalledWith("prev-1", { label: "Conference 2026" });
  });

  it("offers the name box straight away when there are no labels", async () => {
    ipc.pickVcardAndPreview.mockResolvedValue(preview());
    await importDialog({ labels: [] });
    fireEvent.click(screen.getByRole("checkbox", { name: "Label everyone imported:" }));
    expect(screen.queryByRole("combobox")).toBeNull();
    expect(screen.getByRole("textbox", { name: "Name of the new label" })).toBeTruthy();
  });

  it("says a new name that matches a label uses that label", async () => {
    ipc.pickVcardAndPreview.mockResolvedValue(preview());
    await importDialog();
    fireEvent.click(screen.getByRole("checkbox", { name: "Label everyone imported:" }));
    fireEvent.change(screen.getByRole("combobox", { name: "Label for everyone imported" }), {
      target: { value: "new" },
    });
    fireEvent.change(screen.getByRole("textbox", { name: "Name of the new label" }), {
      target: { value: "friends" },
    });
    expect(
      screen.getByText("You already have a label with that name, so PM uses it."),
    ).toBeTruthy();
  });
});

describe("commit and its result", () => {
  async function imported(r: PeopleImportResult, p = preview()) {
    ipc.pickVcardAndPreview.mockResolvedValue(p);
    ipc.commitVcardImport.mockResolvedValue(r);
    const handlers = await importDialog();
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await act(async () => {});
    return handlers;
  }

  it("shows how many people were imported", async () => {
    await imported(result({ added: 3 }));
    expect(screen.getByRole("dialog", { name: "Import finished" })).toBeTruthy();
    expect(screen.getByText("Imported 3 people.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Review possible duplicates" })).toBeNull();
  });

  it("lists where PM kept its own details", async () => {
    await imported(
      result({
        added: 1,
        updated: 1,
        kept: [
          { name: "Ada Example", fields: ["birthday", "notes"] },
          { name: "Ben Sample", fields: ["name"] },
        ],
      }),
    );
    expect(screen.getByText("Kept PM's details where they differed:")).toBeTruthy();
    expect(screen.getByText("Ada Example: birthday, notes")).toBeTruthy();
    expect(screen.getByText("Ben Sample: name")).toBeTruthy();
  });

  it("names anyone who came back from Trash", async () => {
    await imported(result({ added: 0, updated: 2, restored: ["Cal Placeholder", "Dee Invented"] }));
    expect(
      screen.getByText("2 people came back from Trash: Cal Placeholder, Dee Invented."),
    ).toBeTruthy();
  });

  it("offers Review possible duplicates, which opens the review and closes the dialog", async () => {
    const { onReview, onClose } = await imported(result({ added: 4, review: 2 }));
    fireEvent.click(screen.getByRole("button", { name: "Review possible duplicates" }));
    expect(onReview).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("says a stale preview plainly, wrote nothing, and offers to pick the file again", async () => {
    ipc.pickVcardAndPreview.mockResolvedValueOnce(preview());
    ipc.commitVcardImport.mockRejectedValueOnce(STALE);
    await importDialog();
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await act(async () => {});
    expect(screen.getByText(STALE)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Import" })).toBeNull();
    ipc.pickVcardAndPreview.mockResolvedValueOnce(preview({ preview_id: "prev-2", people: 6 }));
    fireEvent.click(screen.getByRole("button", { name: "Pick the file again" }));
    await act(async () => {});
    expect(screen.getByRole("dialog", { name: "6 people in friends.vcf" })).toBeTruthy();
    ipc.commitVcardImport.mockResolvedValueOnce(result({ added: 6 }));
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await act(async () => {});
    expect(ipc.commitVcardImport).toHaveBeenLastCalledWith("prev-2", { label: null });
  });

  it("keeps the preview, with the failure said, when an import fails for another reason", async () => {
    ipc.pickVcardAndPreview.mockResolvedValue(preview());
    ipc.commitVcardImport.mockRejectedValueOnce("PM couldn't save that. Nothing was changed.");
    await importDialog();
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await act(async () => {});
    expect(screen.getByText("PM couldn't save that. Nothing was changed.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Import" })).toBeTruthy();
  });
});

describe("the export dialog", () => {
  async function exportDialog(count = 3) {
    const onClose = vi.fn();
    wrap(
      <ExportPeopleDialog
        choices={[selectionExportChoice([1, 2, 3].slice(0, count))]}
        onClose={onClose}
      />,
    );
    await act(async () => {});
    return onClose;
  }

  it("is titled with the count, and the timeline tick is off", async () => {
    await exportDialog();
    expect(screen.getByRole("dialog", { name: "Export 3 people" })).toBeTruthy();
    const tick = screen.getByRole("checkbox", {
      name: "Include the timeline (a spreadsheet file)",
    }) as HTMLInputElement;
    expect(tick.checked).toBe(false);
  });

  it("says 1 person for one", async () => {
    await exportDialog(1);
    expect(screen.getByRole("dialog", { name: "Export 1 person" })).toBeTruthy();
  });

  it("exports without the timeline by default and names the file written", async () => {
    ipc.exportPeople.mockResolvedValue({ people: 3, files: ["/home/x/people.vcf"] });
    await exportDialog();
    fireEvent.click(screen.getByRole("button", { name: "Export" }));
    await act(async () => {});
    expect(ipc.exportPeople).toHaveBeenCalledWith({ Ids: [1, 2, 3] }, false);
    expect(screen.getByText("Exported 3 people.")).toBeTruthy();
    expect(screen.getByText("The file:")).toBeTruthy();
    expect(screen.getByText("/home/x/people.vcf")).toBeTruthy();
  });

  it("with the timeline ticked asks for it and names both files", async () => {
    ipc.exportPeople.mockResolvedValue({
      people: 3,
      files: ["/home/x/people.vcf", "/home/x/people-timeline.csv"],
    });
    await exportDialog();
    fireEvent.click(screen.getByRole("checkbox", { name: /Include the timeline/ }));
    fireEvent.click(screen.getByRole("button", { name: "Export" }));
    await act(async () => {});
    expect(ipc.exportPeople).toHaveBeenCalledWith({ Ids: [1, 2, 3] }, true);
    expect(screen.getByText("The files:")).toBeTruthy();
    expect(screen.getByText("/home/x/people-timeline.csv")).toBeTruthy();
  });

  it("stays quiet when the save dialog is cancelled", async () => {
    ipc.exportPeople.mockResolvedValue(null);
    const onClose = await exportDialog();
    fireEvent.click(screen.getByRole("button", { name: "Export" }));
    await act(async () => {});
    expect(screen.getByRole("dialog", { name: "Export 3 people" })).toBeTruthy();
    expect(screen.queryByText(/Exported/)).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("says a failed export in words", async () => {
    ipc.exportPeople.mockRejectedValue(
      "PM couldn't write that file. Check the folder and try again.",
    );
    await exportDialog();
    fireEvent.click(screen.getByRole("button", { name: "Export" }));
    await act(async () => {});
    expect(
      screen.getByText("PM couldn't write that file. Check the folder and try again."),
    ).toBeTruthy();
  });
});

async function view() {
  wrap(<PeopleView />);
  await act(async () => {});
}
const tick = (name: string) =>
  fireEvent.click(screen.getByRole("checkbox", { name: `Select ${name}` }));

describe("in the People tab", () => {
  it("opens the picker from Import… in the header", async () => {
    ipc.pickVcardAndPreview.mockResolvedValue(preview({ people: 2 }));
    await view();
    fireEvent.click(screen.getByRole("button", { name: "Import…" }));
    await act(async () => {});
    expect(ipc.pickVcardAndPreview).toHaveBeenCalledOnce();
    expect(screen.getByRole("dialog", { name: "2 people in friends.vcf" })).toBeTruthy();
  });

  it("opens Review possible duplicates after an import", async () => {
    ipc.pickVcardAndPreview.mockResolvedValue(preview());
    ipc.commitVcardImport.mockResolvedValue(result({ review: 1 }));
    ipc.peopleDuplicatesSnapshot.mockResolvedValue({
      pairs: [],
      cards: [],
      hidden: 1,
      skipped: [],
      cut_short: false,
      scanned: 3,
    });
    await view();
    fireEvent.click(screen.getByRole("button", { name: "Import…" }));
    await act(async () => {});
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await act(async () => {});
    fireEvent.click(screen.getByRole("button", { name: "Review possible duplicates" }));
    await act(async () => {});
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(
      screen.getByRole("button", { name: "Possible duplicates" }).getAttribute("aria-pressed"),
    ).toBe("true");
  });

  it("exports everyone from the header, counted from the list", async () => {
    ipc.exportPeople.mockResolvedValue(null);
    await view();
    fireEvent.click(screen.getByRole("button", { name: "Export…" }));
    expect(screen.getByRole("dialog", { name: "Export 3 people" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Export" }));
    await act(async () => {});
    expect(ipc.exportPeople).toHaveBeenCalledWith("All", false);
  });

  it("counts the header's export itself when the list could not be read", async () => {
    ipc.listPeople.mockRejectedValueOnce("The list failed.");
    ipc.exportPeople.mockResolvedValue(null);
    await view();
    ipc.listPeople.mockImplementation(async (f: PeopleFilter) => answer(f));
    fireEvent.click(screen.getByRole("button", { name: "Export…" }));
    await act(async () => {});
    expect(screen.getByRole("dialog", { name: "Export 3 people" })).toBeTruthy();
    expect(screen.queryByText("There is nobody here to export.")).toBeNull();
  });

  it("offers a label's people first when a label is showing", async () => {
    ipc.exportPeople.mockResolvedValue(null);
    await view();
    fireEvent.click(
      within(screen.getByRole("navigation", { name: "People filters" })).getByRole("button", {
        name: /^Friends/,
      }),
    );
    await act(async () => {});
    fireEvent.click(screen.getByRole("button", { name: "Export…" }));
    await act(async () => {});
    expect(screen.getByRole("dialog", { name: "Export 1 person" })).toBeTruthy();
    const mine = screen.getByRole("radio", { name: /People labelled Friends/ }) as HTMLInputElement;
    expect(mine.checked).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Export" }));
    await act(async () => {});
    expect(ipc.exportPeople).toHaveBeenLastCalledWith({ Label: 1 }, false);

    // Everyone is the other choice; its count is read when the dialog opens.
    fireEvent.click(screen.getByRole("radio", { name: /Everyone/ }));
    await act(async () => {});
    expect(screen.getByRole("dialog", { name: "Export 3 people" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Export" }));
    await act(async () => {});
    expect(ipc.exportPeople).toHaveBeenLastCalledWith("All", false);
  });

  it("has no Export… in the Trash view", async () => {
    await view();
    fireEvent.click(
      within(screen.getByRole("navigation", { name: "People filters" })).getByRole("button", {
        name: /^Trash/,
      }),
    );
    await act(async () => {});
    expect(screen.queryByRole("button", { name: "Export…" })).toBeNull();
  });

  it("exports exactly the people ticked from the selection bar", async () => {
    ipc.exportPeople.mockResolvedValue(null);
    await view();
    tick("Ada Example");
    tick("Cal Placeholder");
    const bar = within(screen.getByRole("region", { name: "Selected people" }));
    fireEvent.click(bar.getByRole("button", { name: "Export…" }));
    expect(screen.getByRole("dialog", { name: "Export 2 people" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Export" }));
    await act(async () => {});
    expect(ipc.exportPeople).toHaveBeenCalledWith({ Ids: [1, 3] }, false);
  });

  it("offers Export… in a ticked row's menu, for the ticked people, and not on an unticked row", async () => {
    ipc.exportPeople.mockResolvedValue(null);
    await view();
    const rowOf = (name: string) => screen.getByText(name).closest("li")!;
    fireEvent.contextMenu(rowOf("Ben Sample"), { clientX: 5, clientY: 5 });
    expect(screen.queryByRole("menuitem", { name: "Export…" })).toBeNull();
    act(() => closeContextMenu());

    tick("Ada Example");
    tick("Ben Sample");
    fireEvent.contextMenu(rowOf("Ben Sample"), { clientX: 5, clientY: 5 });
    fireEvent.click(screen.getByRole("menuitem", { name: "Export…" }));
    expect(screen.getByRole("dialog", { name: "Export 2 people" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Export" }));
    await act(async () => {});
    expect(ipc.exportPeople).toHaveBeenCalledWith({ Ids: [1, 2] }, false);
  });
});
