// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The words for "open this event where it came from", shared by the details popover's button and
// the event's right-click menu so the two always say the same thing.

/** "Open in Google" / "Open in Outlook" / a generic label from the provider. The right-click menu
 *  says the same words, so it reads this one. */
export function sourceLabel(provider: string | undefined): string {
  switch (provider) {
    case "google":
      return "Open in Google Calendar";
    case "outlook":
    case "microsoft":
      return "Open in Outlook";
    default:
      return "Open in calendar";
  }
}
