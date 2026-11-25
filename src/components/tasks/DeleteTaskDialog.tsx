// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The "Delete this task?" question, in one place. The task editor asks it from its Delete button
// and the Runway asks it from a card's right-click menu, so the words, the red button and the
// promise ("its project, milestone and event stay") are the same wherever a task is deleted. The
// dialog only asks: whoever opens it does the delete, and says what went wrong.

import { ConfirmDialog } from "../ui";

export function DeleteTaskDialog({
  open,
  title,
  busy,
  onConfirm,
  onClose,
}: {
  open: boolean;
  /** The task's title, quoted in the question. */
  title: string | undefined;
  busy?: boolean;
  onConfirm: () => void;
  onClose: () => void;
}) {
  return (
    <ConfirmDialog
      open={open}
      title="Delete this task?"
      confirmLabel="Delete"
      danger
      busy={busy}
      onConfirm={onConfirm}
      onClose={onClose}
    >
      “{title}” goes from every list it shows on. Its project, milestone and event stay as they are.
    </ConfirmDialog>
  );
}
