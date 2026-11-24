// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Token-driven text input (DESIGN_TOKENS.md §7). Terminal flavour: mono + accent caret.
// (The ❯ prompt adornment is applied at call sites that want it, e.g. the composer/palette.)

import type { ComponentPropsWithRef } from "react";
import { useTheme } from "../theme";
import { cn } from "./cn";

export function Input({ className, ...rest }: ComponentPropsWithRef<"input">) {
  const { system } = useTheme();
  return (
    <input
      className={cn(
        // The edge is `--ink4`, not `--border2`: a field's border is what marks it out, so it owes
        // 3:1 (WCAG 1.4.11), and `--border2` measures 1.33-1.84:1 at the default Contrast. Focus
        // moves to `--accent-text` (never `--accent`, which falls to 1.4:1 on a pale accent in a
        // light theme and would make the focused edge weaker than the resting one). contrast.test.ts
        // pins both. Disabled drops back to `--border2`: a disabled field is exempt and should recede.
        "w-full rounded-[var(--radius-sm)] border border-ink4 bg-surface px-3 py-2 text-sm text-ink2 outline-none transition placeholder:text-ink4 focus:border-accent-text disabled:cursor-not-allowed disabled:border-border2 disabled:bg-transparent disabled:text-ink3",
        system === "terminal" && "font-mono caret-[var(--accent-text)]",
        className,
      )}
      {...rest}
    />
  );
}
