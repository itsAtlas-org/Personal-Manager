// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The right-click menu of an assistant answer in the chat: Copy answer, Save as note and the helpful /
// missed rating. Save and the rating call what the buttons under the answer call. Copy is the one
// item that is new, and a message bubble can't take focus, so the answer also has a visible Copy
// button beside Save as note: the menu is a shortcut to it and never the only way (WCAG 2.1.1). An
// item that doesn't apply is left out: no Save as note while a save is running or done (the button
// is disabled then too), and no rating where the answer shows none. The user's own messages have no
// menu of this kind (the fallback still copies a selection).

import { copyViaHost, type MenuItem } from "../kit/lib/contextMenu";
import type { AnswerRating } from "./types";

export interface ChatMessageMenuHandlers {
  /** Save the answer as a note. Absent while a save is running or has finished. */
  save?: () => void;
  /** The answer's rating, and the choice of one. Absent where the answer shows no rating controls. */
  rate?: { current: AnswerRating | null; choose: (next: AnswerRating) => void };
}

export function chatMessageMenuItems(content: string, h: ChatMessageMenuHandlers): MenuItem[] {
  const rate = h.rate;
  return [
    { label: "Copy answer", onSelect: () => copyViaHost(content, "Answer copied") },
    ...(h.save ? [{ label: "Save as note", onSelect: h.save } satisfies MenuItem] : []),
    ...(rate
      ? ([
          { kind: "separator" },
          {
            label: "This answer was helpful",
            checked: rate.current === "up",
            onSelect: () => rate.choose("up"),
          },
          {
            label: "This answer missed",
            checked: rate.current === "down",
            onSelect: () => rate.choose("down"),
          },
        ] satisfies MenuItem[])
      : []),
  ];
}
