// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The right-click menu of a conversation row in the sidebar (the Global chats list and an open
// project's Conversations list). Each item repeats a control that is on screen and calls its handler:
// the row's own button opens it, the 📁 and 🗑 buttons ask the two questions, and the chat header's
// title renames it. So the menu is a shortcut for the two buttons that only show on hover, never the
// only way, and Delete… opens the existing "Delete this conversation?" confirm and deletes nothing
// itself. Rename is left out where the chat has no title control to rename it with (a project chat).

import type { MenuItem } from "../kit/lib/contextMenu";

export interface ConversationMenuHandlers {
  open: () => void;
  /** Select the chat and put its header title into edit mode. Absent for a project chat, whose
   *  title can't be edited anywhere yet. */
  rename?: () => void;
  /** Open "Move to a project". */
  askMove: () => void;
  /** Open "Delete this conversation?". */
  askDelete: () => void;
}

export function conversationMenuItems(h: ConversationMenuHandlers): MenuItem[] {
  return [
    { label: "Open", onSelect: h.open },
    ...(h.rename ? [{ label: "Rename", onSelect: h.rename } satisfies MenuItem] : []),
    { label: "Move to project…", onSelect: h.askMove },
    { kind: "separator" },
    { label: "Delete…", danger: true, onSelect: h.askDelete },
  ];
}
