// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** Where to turn the Google People API on when the error carries no link of its own. */
export const PEOPLE_API_ENABLE_URL =
  "https://console.cloud.google.com/apis/library/people.googleapis.com";

/** The two hosts Google's own enable link lives on (the backend keeps to the same two). */
const CONSOLE_LINK = /^https:\/\/console\.(developers|cloud)\.google\.com\//;

/**
 * Recognise Google's "the People API isn't enabled for your Cloud project" 403 on any Contacts call,
 * and pull the project-specific enable link out of the message: the Calendar page's twin, so the
 * error is something to act on, not a dead end. The backend answers it in its own words (they start
 * "Google's People API isn't turned on", `contacts_accounts::API_DISABLED_MARKER`) and never
 * copies Google's reply, but Google's own wording is matched too. Returns `null` for any other error,
 * which is shown as it is.
 */
export function contactsApiDisabled(
  error: string | null | undefined,
): { enableUrl: string } | null {
  if (!error) return null;
  if (!/People API isn.t turned on|has not been used in project|accessNotConfigured/i.test(error)) {
    return null;
  }
  const url = error.match(/https?:\/\/[^\s"'<>]+/)?.[0]?.replace(/[.,;:]+$/, "");
  // Only a Google console link is offered as the place to turn the API on; any other URL in the text
  // is not ours to link.
  return { enableUrl: url !== undefined && CONSOLE_LINK.test(url) ? url : PEOPLE_API_ENABLE_URL };
}
