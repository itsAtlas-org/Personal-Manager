// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";

import { mcpOffered } from "./ipc";

/**
 * Whether PM offers connected apps on this computer (Mods spec 1.16): decided by Rust at build time
 * (`mods::mcp::commands::offered`), so a release build shows nothing until its operating system has
 * been tested. `false` until Rust has answered, and if it cannot: a section that may not exist is
 * never drawn on a guess. The Settings rail and the section itself both read this, so they agree.
 */
export function useMcpOffered(): boolean {
  const [offered, setOffered] = useState(false);
  useEffect(() => {
    let alive = true;
    // Through a promise, so a bridge that is not there rejects here rather than throwing in the effect.
    Promise.resolve()
      .then(mcpOffered)
      .then((o) => {
        if (alive) setOffered(o);
      })
      .catch(() => {
        /* stays false: not offered */
      });
    return () => {
      alive = false;
    };
  }, []);
  return offered;
}
