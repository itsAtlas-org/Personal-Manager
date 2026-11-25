// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// PM's right-click menu host, as the page mounts it (main.tsx, once per window). The menu, its store
// and its fallback live in the mod kit (src/kit/ui/ContextMenuHost.tsx, src/kit/lib/contextMenu.ts),
// which holds no way to open a link: core gives it the one it has, `lib/linkFailure`'s `openLink`,
// whose failure the link notice reports. A mod frame mounts the kit's host with its own opener.

import { ContextMenuHost as KitContextMenuHost } from "../../kit/ui/ContextMenuHost";
import { openLink } from "../../lib/linkFailure";

export function ContextMenuHost() {
  return <KitContextMenuHost openUrl={openLink} />;
}
