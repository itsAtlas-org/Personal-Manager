// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The shared primitives (components/ui) drawn exactly as core draws them today: the exact DOM of a
// Button in each variant and size, a Callout, a Select, an Input, a SegmentedControl, a Collapsible,
// an open Popover, a shown Tooltip, a Dialog in both chromes and a ConfirmDialog. Moving them into the
// mod kit (the Mods kit, PR C1 K6b) must leave every attribute, class and node of each as it was, and
// the old import paths (`./Button`, `./index`) must still give core the same components: this file is
// the proof, and its expected DOM (src/components/ui/__dom__/) is written once, from the code before the move. It uses the REAL
// ThemeProvider (no `useTheme` stub), so it also holds that core and the kit share one theme context.

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import type { ReactElement } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../lib/ipc", () => ({
  getPref: vi.fn(async () => null),
  setPref: vi.fn(async () => {}),
}));

import { ThemeProvider } from "../../theme";
import {
  Button,
  Callout,
  Collapsible,
  ConfirmDialog,
  Dialog,
  Input,
  Popover,
  SegmentedControl,
  Select,
  Tooltip,
} from "./index";

beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

beforeEach(() => {
  localStorage.clear();
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  localStorage.clear();
});

function mount(ui: ReactElement) {
  return render(<ThemeProvider>{ui}</ThemeProvider>);
}

describe("components/ui as core draws it (exact DOM)", () => {
  it("Button: every variant and size, disabled, and the terminal System", async () => {
    const variants = ["primary", "secondary", "tertiary", "danger"] as const;
    const sizes = ["xs", "sm", "md", "lg", "bar"] as const;
    const { container } = mount(
      <div>
        {variants.map((v) =>
          sizes.map((s) => (
            <Button key={`${v}${s}`} variant={v} size={s}>
              {v} {s}
            </Button>
          )),
        )}
        <Button disabled>Off</Button>
      </div>,
    );
    await expect(container).toMatchFileSnapshot("./__dom__/buttons.slate.dom.txt");
    cleanup();
    localStorage.setItem("pm:theme:system", "terminal");
    const terminal = mount(
      <div>
        <Button>Plain</Button>
        <Select aria-label="Pick">
          <option>One</option>
        </Select>
        <Input aria-label="Name" defaultValue="Atlas" />
        <Collapsible title="Fold">Inside</Collapsible>
      </div>,
    );
    await expect(terminal.container).toMatchFileSnapshot("./__dom__/controls.terminal.dom.txt");
  });

  it("Callout, Select, Input, SegmentedControl and Collapsible", async () => {
    const { container } = mount(
      <div>
        <Callout tone="warning">Careful</Callout>
        <Callout tone="danger" variant="strip" size="md">
          Broken
        </Callout>
        <Select aria-label="Pick" compact defaultValue="b">
          <option value="a">A</option>
          <option value="b">B</option>
        </Select>
        <Select aria-label="Bar" bar>
          <option>One</option>
        </Select>
        <Input aria-label="Name" defaultValue="Atlas" placeholder="Name" />
        <SegmentedControl
          ariaLabel="View"
          value="b"
          onChange={() => {}}
          options={[
            { value: "a", label: "Alpha" },
            { value: "b", label: "Beta", title: "The second" },
            { value: "c", label: "Gamma", disabled: true },
          ]}
        />
        <Collapsible title="Open fold" meta="3">
          Body
        </Collapsible>
        <Collapsible title="Shut fold" defaultOpen={false}>
          Hidden
        </Collapsible>
      </div>,
    );
    await expect(container).toMatchFileSnapshot("./__dom__/controls.slate.dom.txt");
  });

  it("Popover: the open panel, inline and escaping its clip", async () => {
    mount(
      <div>
        <Popover
          open
          ariaLabel="Inline"
          trigger={({ toggle }) => <button onClick={toggle}>Inline trigger</button>}
        >
          <p>Inline panel</p>
        </Popover>
        <Popover
          open
          escapeClipping
          ariaLabel="Escaped"
          trigger={({ toggle }) => <button onClick={toggle}>Escaped trigger</button>}
        >
          <p>Escaped panel</p>
        </Popover>
      </div>,
    );
    await expect(document.body).toMatchFileSnapshot("./__dom__/popover.dom.txt");
  });

  it("Tooltip: shown after the delay, above and below", async () => {
    vi.useFakeTimers();
    mount(
      <div>
        <Tooltip label="Above it">
          <button>One</button>
        </Tooltip>
        <Tooltip label="Below it" placement="below">
          <button>Two</button>
        </Tooltip>
      </div>,
    );
    for (const name of ["One", "Two"]) {
      const trigger = Array.from(document.querySelectorAll("button")).find(
        (b) => b.textContent === name,
      )!;
      fireEvent.pointerEnter(trigger);
    }
    act(() => {
      vi.advanceTimersByTime(300);
    });
    await expect(document.body).toMatchFileSnapshot("./__dom__/tooltip.dom.txt");
  });

  it("Dialog: the card and the bar chrome, and ConfirmDialog", async () => {
    mount(
      <div>
        <Dialog
          open
          onClose={() => {}}
          title="Card dialog"
          subtitle="A sub line"
          eyebrow="Step 1 of 2"
          footer={<Button>Done</Button>}
        >
          <p>Card body</p>
        </Dialog>
        <Dialog open onClose={() => {}} title="Bar dialog" chrome="bar" tone="danger">
          <p>Bar body</p>
        </Dialog>
        <ConfirmDialog
          open
          title="Delete it?"
          confirmLabel="Delete"
          danger
          initialFocus="cancel"
          onConfirm={() => {}}
          onClose={() => {}}
        >
          This cannot be undone.
        </ConfirmDialog>
      </div>,
    );
    await expect(document.body).toMatchFileSnapshot("./__dom__/dialogs.dom.txt");
  });
});
