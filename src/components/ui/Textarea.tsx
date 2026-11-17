// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ComponentPropsWithRef } from "react";
import { useTheme } from "../../theme";
import { cn } from "./cn";

// Token-driven multiline input — same surface/border/focus treatment as Input. Uses
// ComponentPropsWithRef so callers (e.g. Composer's focus ref) can forward a ref.
export function Textarea({ className, ...rest }: ComponentPropsWithRef<"textarea">) {
  const { system } = useTheme();
  return (
    <textarea
      className={cn(
        // Edge `--ink4` (3:1, WCAG 1.4.11), focus `--accent-text`; see Input.tsx for the measurements.
        "w-full resize-none rounded-[var(--radius-sm)] border border-ink4 bg-surface px-3 py-2 text-sm text-ink2 outline-none transition placeholder:text-ink4 focus:border-accent-text disabled:cursor-not-allowed disabled:border-border2 disabled:bg-transparent disabled:text-ink3",
        system === "terminal" && "font-mono caret-[var(--accent-text)]",
        className,
      )}
      {...rest}
    />
  );
}
