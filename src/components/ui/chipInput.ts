// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** The small text field at the end of a row of chips: add a tag, add another name, link a project.
 *  Its edge is what marks it out as a field, so it takes `Input`'s edge: `--ink4` (3:1, WCAG
 *  1.4.11; contrast.test.ts measures it on every surface), `--accent-text` on focus, and `--border2`
 *  when disabled, which is exempt and recedes. Its padding is a chip's `py-0.5` less the 1px edge,
 *  so it is exactly as tall as a plain chip beside it at every Text size (rem scales, the edge
 *  doesn't). The width is the caller's. designGuards.test.ts keeps every chip-row field on it. */
export const CHIP_INPUT =
  "rounded-[var(--radius-sm)] border border-ink4 bg-transparent px-1.5 py-[calc(0.125rem_-_1px)] text-xs text-ink2 outline-none transition placeholder:text-ink4 focus:border-accent-text disabled:cursor-not-allowed disabled:border-border2 disabled:text-ink3";
