// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The mod kit's own surface (Mods spec D14): the names a mod's page may import, every one of them
// closed under the kit's own imports. A kit file imports only the kit, packages and `import type` from
// `src/lib/types.ts` (the fence in eslint.config.js, proved by kitFence.test.ts), so nothing here can
// pull `ipc`, a Tauri API or a core module into a mod's bundle.
//
// A mod imports THIS file, never core's `components/ui/index.ts`: that barrel also names components
// that reach `lib/linkFailure`, which imports `openUrl` from `ipc` (CopyLinkButton, NavItem, TitleBar),
// and the context-menu host core mounts. The kit's `ContextMenuHost` takes its link opener as a prop.
//
// The bridge side of the kit (host.ts, fakePm.ts) is not re-exported: a mod imports it by path.

// --- components (src/kit/ui) ---
export { cn } from "./ui/cn";
export { Button, type ButtonProps, type ButtonVariant, type ButtonSize } from "./ui/Button";
export {
  Callout,
  type CalloutProps,
  type CalloutTone,
  type CalloutVariant,
  type CalloutSize,
  type CalloutBody,
} from "./ui/Callout";
export { TONE_TOKEN, TONE_TEXT_TOKEN, TONE_MIX, toneMix, toneSurface, type Tone } from "./ui/tone";
export { Input } from "./ui/Input";
export { Select } from "./ui/Select";
export { Toggle, type ToggleProps } from "./ui/Toggle";
export {
  SegmentedControl,
  type SegOption,
  type SegmentedControlProps,
} from "./ui/SegmentedControl";
export { Collapsible, type CollapsibleProps } from "./ui/Collapsible";
export {
  Modal,
  type ModalProps,
  type ModalBaseProps,
  type ModalNameProps,
  type ModalPlacement,
} from "./ui/Modal";
export { Dialog, type DialogProps, type DialogChrome, type DialogTone } from "./ui/Dialog";
export { ConfirmDialog, type ConfirmDialogProps } from "./ui/ConfirmDialog";
export { Tooltip, type TooltipProps } from "./ui/Tooltip";
export { Popover } from "./ui/Popover";
export { VisuallyHidden } from "./ui/VisuallyHidden";
export { BulletIcon, ClearFormatIcon, LinkIcon, NumberIcon } from "./ui/FormatIcons";
export { DateField } from "./ui/DateField";
export { MonthPicker } from "./ui/MonthPicker";
export { ContextMenuHost } from "./ui/ContextMenuHost";

// --- tasks (src/kit/tasks, src/kit/taskData.ts): the list reads and writes through a TaskData ---
export { TaskList, type TaskListProps } from "./tasks/TaskList";
export { DeadlineChip, ProjectChip, StackChip, TaskStatusButton } from "./tasks/TaskBits";
export {
  TaskDataProvider,
  useTaskData,
  type NewTask,
  type TaskData,
  type TaskEditTarget,
} from "./taskData";

// --- theme (src/kit/theme.tsx, src/kit/design) ---
export { ThemeProvider, useTheme, type FontScale, type ThemeState, type ThemeStore } from "./theme";
export { useDepth, type DepthState } from "./design/depth";
export { prefersReducedMotion, scrollBehavior } from "./design/motion";
export { applyTheme, themeVars, type ThemeVars } from "./design/tokens";

// --- pure helpers (src/kit/lib) ---
export {
  closeContextMenu,
  openContextMenu,
  useContextMenu,
  wantsAppMenu,
  type MenuItem,
  type OpenMenu,
} from "./lib/contextMenu";
export { MILESTONE_STATUSES, milestoneStatus } from "./lib/milestoneStatus";
export { runMutation } from "./lib/runMutation";
export {
  REDACTED_EVENT_UID,
  REDACTED_MILESTONE_ID,
  fromModTask,
  type ModTaskDto,
} from "./lib/modTask";
export { subscribeUntilCleanup, type Unlisten } from "./lib/subscribe";
export { useDialogLayer } from "./lib/useDialogLayer";
