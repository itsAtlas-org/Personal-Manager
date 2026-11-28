// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { Callout, IconButton } from "./ui";

/** A line about what a model-list edit did that is information, not a failure (the first edit's
 *  "N other saved models weren't in use"): a neutral strip the person can dismiss, announced politely.
 *  It is deliberately not the chat's error banner, which reads as something having gone wrong. */
export function ListNotice({ text, onDismiss }: { text: string; onDismiss: () => void }) {
  return (
    <Callout
      tone="info"
      variant="strip"
      size="md"
      live
      className="flex items-center gap-2 font-ui"
      data-testid="list-notice"
    >
      <span className="min-w-0 flex-1">{text}</span>
      <IconButton label="Dismiss" variant="subtle" onClick={onDismiss} className="shrink-0">
        ✕
      </IconButton>
    </Callout>
  );
}
