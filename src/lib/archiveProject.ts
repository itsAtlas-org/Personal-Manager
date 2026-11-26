// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Archive or unarchive a project, in one place. Archiving changes what the briefing and the flag
// layer reason over, so the briefing is regenerated after it; Focus did that and the project page's
// header button did not, which left the briefing naming an archived project until its next refresh.
// Focus's cards, the project page, and the right-click menus on a Focus card and on a sidebar project
// entry all come through here, so they can't disagree again.

import { useCallback } from "react";
import { setProjectArchived } from "./ipc";
import { useBriefing } from "./briefing";

/** `(name, on, alongside?)` archives (`on`) or restores `name`, then regenerates the briefing, running
 *  `alongside` (a view's own re-read) at the same time. Rejects with the write's error, which the
 *  caller shows where it shows its own; a failed regeneration is the briefing's to report, not this
 *  one's, since the project is archived either way. */
export function useArchiveProject(): (
  name: string,
  on: boolean,
  alongside?: () => Promise<unknown>,
) => Promise<void> {
  const { refresh: regenerateBriefing } = useBriefing();
  return useCallback(
    async (name, on, alongside) => {
      await setProjectArchived(name, on);
      await Promise.all([regenerateBriefing(), alongside?.()]);
    },
    [regenerateBriefing],
  );
}
