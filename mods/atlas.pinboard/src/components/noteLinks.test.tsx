// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Links that stop working (Mods plan K7). Core opened `http`, `https` and `mailto` to any host; PM's
// `shell.openLink` opens `https` to a real website name and nothing else. The note editors offer a link
// only when PM will open it, and draw every other address as plain text the person can still select.

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import linkCallsRs from "../../../../src-tauri/src/mods/api/link_calls.rs?raw";
import { canOpenLink } from "../lib/pinboard/links";
import { dismissLinkFailure } from "../lib/linkFailure";
import { NoteEditor } from "./NoteEditor";
import { RichNoteEditor } from "./RichNoteEditor";

const openUrl = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock("../lib/pm", () => ({ openUrl }));

beforeAll(() => {
  const rect = { left: 0, right: 0, top: 0, bottom: 0, width: 0, height: 0, x: 0, y: 0 };
  const rects = () =>
    ({
      length: 0,
      item: () => null,
      [Symbol.iterator]: [][Symbol.iterator],
    }) as unknown as DOMRectList;
  Range.prototype.getClientRects = rects;
  Range.prototype.getBoundingClientRect = () => ({ ...rect, toJSON: () => rect }) as DOMRect;
  Element.prototype.getClientRects = rects;
  document.elementFromPoint = () => null;
});
beforeEach(() => openUrl.mockClear());
afterEach(() => {
  cleanup();
  dismissLinkFailure();
});

const OPENS = [
  "https://example.org",
  "https://sub.example.co.uk/a/b?q=1#x",
  "HTTPS://Example.ORG/",
];
const STAYS_PLAIN = [
  "http://example.org",
  "mailto:sam@example.org",
  "https://192.168.1.5/admin",
  "https://127.0.0.1",
  "https://2130706433",
  "https://0x7f.1",
  "https://[::1]/",
  "https://localhost:3000",
  "https://app.localhost",
  "https://intranet/wiki",
  "https://intranet./wiki",
  "https://user:pw@example.org",
  "https://trusted.com@evil.example",
  "file:///etc/passwd",
  "javascript:alert(1)",
  "ftp://example.org",
  "not a link",
  "",
  " https://example.org",
  `https://example.org/${"a".repeat(2100)}`,
];

describe("which addresses open", () => {
  it.each(OPENS)("%s opens", (href) => {
    expect(canOpenLink(href)).toBe(true);
  });
  it.each(STAYS_PLAIN)("%s is plain text", (href) => {
    expect(canOpenLink(href)).toBe(false);
  });

  it("is the rule PM holds the call to: its source says the same things", () => {
    // `check_link` is read as text: https only, no credentials, no IP, no `localhost`, a dot in the
    // name, at most LINK_MAX_BYTES. If PM's rule moves, this page's must move with it.
    expect(linkCallsRs).toContain('url.scheme() != "https"');
    expect(linkCallsRs).toContain("!url.username().is_empty() || url.password().is_some()");
    expect(linkCallsRs).toContain("net_screen::literal_ip(host).is_some()");
    expect(linkCallsRs).toContain("!name.contains('.')");
    expect(linkCallsRs).toContain('name == "localhost" || name.ends_with(".localhost")');
    expect(linkCallsRs).toContain("raw.len() > limits::LINK_MAX_BYTES");
  });
});

const NOTE = [
  "[web](https://example.org)",
  "[old](http://example.org)",
  "[mail](mailto:sam@example.org)",
  "[lan](https://192.168.1.5/)",
  "[box](http://localhost:3000)",
  "[wiki](https://intranet/x)",
].join("\n");

const editorProps = { onChange: () => {}, onFocus: () => {}, placeholder: "x" };

describe("the Markdown editor", () => {
  it("draws only an address PM would open as a link, and the rest as plain text with the address on hover", () => {
    const { container } = render(<NoteEditor {...editorProps} value={NOTE} keyEdit={() => null} />);
    const links = [...container.querySelectorAll("[data-href]")];
    expect(links.map((l) => l.getAttribute("data-href"))).toEqual(["https://example.org"]);
    const plain = [...container.querySelectorAll(".pm-cm-plainlink")];
    expect(plain.map((l) => l.textContent)).toEqual(["old", "mail", "lan", "box", "wiki"]);
    expect(plain.map((l) => l.getAttribute("title"))).toContain("mailto:sam@example.org");
    for (const p of plain) expect(p.hasAttribute("data-href")).toBe(false);
  });

  it("a click opens an openable link through PM and does nothing on a plain one", () => {
    const { container } = render(<NoteEditor {...editorProps} value={NOTE} keyEdit={() => null} />);
    const web = container.querySelector("[data-href]")!;
    fireEvent.mouseDown(web, { ctrlKey: true });
    expect(openUrl).toHaveBeenCalledTimes(1);
    expect(openUrl).toHaveBeenCalledWith("https://example.org");
    for (const p of container.querySelectorAll(".pm-cm-plainlink")) {
      fireEvent.mouseDown(p, { ctrlKey: true });
    }
    expect(openUrl).toHaveBeenCalledTimes(1);
  });
});

describe("the rich editor", () => {
  it("draws only an address PM would open as a link, and the rest as plain text", () => {
    const { container } = render(<RichNoteEditor {...editorProps} value={NOTE} />);
    const links = [...container.querySelectorAll(".pm-rich-link")];
    expect(links.map((l) => l.getAttribute("data-href"))).toEqual(["https://example.org"]);
    const plain = [...container.querySelectorAll(".pm-plain-link")];
    expect(plain.map((l) => l.textContent)).toEqual(["old", "mail", "lan", "box", "wiki"]);
    for (const p of plain) expect(p.hasAttribute("data-href")).toBe(false);
  });

  it("a click opens an openable link through PM and does nothing on a plain one", async () => {
    const { container } = render(<RichNoteEditor {...editorProps} value={NOTE} />);
    await act(async () => {
      fireEvent.mouseDown(container.querySelector(".pm-rich-link")!, { ctrlKey: true });
    });
    expect(openUrl).toHaveBeenCalledTimes(1);
    for (const p of container.querySelectorAll(".pm-plain-link")) {
      await act(async () => {
        fireEvent.mouseDown(p, { ctrlKey: true });
      });
    }
    expect(openUrl).toHaveBeenCalledTimes(1);
  });
});
