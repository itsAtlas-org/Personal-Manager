// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the frame draws: the Pinboard under the kit's providers (PM's theme, the task data its list
// cards read through), the one right-click menu host a frame has, and the notice a link that did not
// open leaves. Core mounts the same pieces around its own Pinboard (App.tsx, main.tsx).

import {
  Button,
  ContextMenuHost,
  TaskDataProvider,
  ThemeProvider,
  type TaskData,
  type ThemeStore,
} from "../../../../src/kit";
import { PinboardView } from "../components/PinboardView";
import { dismissLinkFailure, openLink, useLinkFailure } from "../lib/linkFailure";

/** The notice a link that did not open leaves, in PM's own words, until it is dismissed or the next
 *  link is clicked. */
function LinkNotice() {
  const text = useLinkFailure();
  if (text === null) return null;
  return (
    <div
      role="status"
      className="fixed bottom-3 left-3 right-3 z-50 flex items-center justify-between gap-3 rounded border border-ink4 bg-surface px-3 py-2 text-sm text-ink2"
    >
      <span className="min-w-0">{text}</span>
      <Button variant="tertiary" size="xs" onClick={dismissLinkFailure}>
        Dismiss
      </Button>
    </div>
  );
}

export function App({ theme, tasks }: { theme: ThemeStore; tasks: TaskData }) {
  return (
    <ThemeProvider store={theme}>
      <TaskDataProvider data={tasks}>
        <div className="flex h-full min-w-0 flex-col">
          <PinboardView />
        </div>
        <ContextMenuHost openUrl={openLink} />
        <LinkNotice />
      </TaskDataProvider>
    </ThemeProvider>
  );
}
