// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Which addresses in a note open from the Pinboard (Mods plan K7, "links that stop working"). Core's
// Pinboard opened `http`, `https` and `mailto` to any host. A frame can only ask PM to open a link
// (`shell.openLink`), and PM opens an `https` address to a real website name and nothing else: no
// `http`, no `mailto`, no user name or password, no address that is an IP in any spelling, no
// `localhost`, no name with a single label (`intranet`), no more than 2,048 bytes
// (`check_link` in `src-tauri/src/mods/api/link_calls.rs`, which a test reads as text and holds this
// to). This is the same rule on the page, so the page offers a link only when PM will open it; any
// other address is drawn as plain text the person can still select and copy, and a click does
// nothing.

const MAX_LINK_BYTES = 2048;

/** Whether PM's `shell.openLink` will open `href`. Pure; never throws. */
export function canOpenLink(href: string): boolean {
  if (new TextEncoder().encode(href).length > MAX_LINK_BYTES) return false;
  // No control character, and no space around it (PM refuses both).
  if (href.trim() !== href || /\p{Cc}/u.test(href)) return false;
  let url: URL;
  try {
    url = new URL(href);
  } catch {
    return false;
  }
  if (url.protocol !== "https:" || url.username !== "" || url.password !== "") return false;
  const host = url.hostname;
  if (host === "") return false;
  // An IPv6 address keeps its brackets; an IPv4 one, in any spelling (`2130706433`, `0x7f.1`), has
  // been read into dotted form by the URL parser.
  if (host.startsWith("[") || /^\d{1,3}(\.\d{1,3}){3}$/.test(host)) return false;
  const name = host.replace(/\.+$/, "");
  return name !== "" && name.includes(".") && name !== "localhost" && !name.endsWith(".localhost");
}
