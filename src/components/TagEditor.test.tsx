// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TagEditor } from "./TagEditor";

afterEach(cleanup);

describe("TagEditor", () => {
  // #1115: the add field was named only by its placeholder, which goes as soon as you type.
  it("names its add field, and Enter adds what was typed", () => {
    const onChange = vi.fn();
    render(<TagEditor tags={["finance"]} onChange={onChange} />);
    const field = screen.getByRole("textbox", { name: "Add a tag" });
    fireEvent.change(field, { target: { value: "Q3 Report" } });
    fireEvent.keyDown(field, { key: "Enter" });
    expect(onChange).toHaveBeenCalledWith(["finance", "q3 report"]);
  });

  // In a project's list every document row has one, so each names its document.
  it("takes a label naming what the tag goes on", () => {
    render(<TagEditor tags={[]} onChange={vi.fn()} label="Add a tag to Budget.xlsx" />);
    expect(screen.getByRole("textbox", { name: "Add a tag to Budget.xlsx" })).toBeTruthy();
  });
});
