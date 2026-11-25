// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// How full the board is against what PM keeps in one save (Mods plan K7, Q1). PM takes one call of at
// most 1 MiB, and a save is the whole board in one call, so a board has a size it cannot grow past.
// Core's Pinboard had no such limit; the meter lets a person see it coming instead of meeting it as a
// refusal. The percentage is of the bytes `storage.set` would send, measured by usePinboard.

/** From this much full the meter is drawn as a warning. */
export const SIZE_WARN_AT = 80;

/** A byte count in the person's words: "840 bytes", "13.0 KB", "1.0 MB". */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} byte${bytes === 1 ? "" : "s"}`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function SizeMeter({ bytes, percent }: { bytes: number; percent: number }) {
  const near = percent >= SIZE_WARN_AT;
  return (
    <div className="mt-1 flex items-center gap-2 text-[0.625rem] text-ink4">
      <span className={near ? "text-st-due" : undefined}>
        Board size: {formatBytes(bytes)} of 1 MB ({percent}%)
      </span>
      <div
        role="meter"
        aria-label="How full the board is"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
        className="h-1.5 w-24 shrink-0 overflow-hidden rounded-full border border-ink4"
      >
        <div
          className={near ? "h-full bg-st-due" : "h-full bg-ink3"}
          style={{ width: `${Math.min(100, percent)}%` }}
        />
      </div>
    </div>
  );
}
