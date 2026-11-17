// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Human-friendly, searchable labels for IANA time zones. Each option reads "Continent / Country /
// City" with a compact code (a real abbreviation like EDT when the platform exposes one, else the
// UTC offset), so the picker is findable by continent, country, city, offset OR code — and someone
// who can't find their city can search their country and pick the nearest listed one. Country names
// come from Intl.DisplayNames over the stored ZONE_COUNTRY code (so they follow the display locale);
// continent and city come from the id itself. Options are memoised — the derivation is pure per id.

import { allTimeZones } from "../theme";
import { ZONE_COUNTRY } from "./zoneCountry";

export interface ZoneOption {
  id: string;
  /** "Continent / Country / City" (country dropped when it equals the continent, e.g. Australia). */
  label: string;
  /** A real zone abbreviation when the runtime has one (e.g. "EDT"), else the UTC offset. */
  code: string;
  /** Lowercased haystack: id words + continent + country + city + offset + abbreviation. */
  search: string;
}

let regionNames: Intl.DisplayNames | null = null;
function countryName(id: string): string | null {
  const cc = ZONE_COUNTRY[id];
  if (!cc) return null;
  try {
    regionNames ??= new Intl.DisplayNames(["en"], { type: "region" });
    const n = regionNames.of(cc);
    return n && n !== cc ? n : null;
  } catch {
    return null;
  }
}

function zonePart(id: string, style: "short" | "shortOffset", at: Date): string | null {
  try {
    return (
      new Intl.DateTimeFormat("en", { timeZone: id, timeZoneName: style })
        .formatToParts(at)
        .find((p) => p.type === "timeZoneName")?.value ?? null
    );
  } catch {
    return null;
  }
}

/** Normalise the runtime's "GMT+5:30" / "GMT" to a padded "UTC+05:30" / "UTC+00:00". */
function utcOffset(id: string, at: Date): string {
  const g = zonePart(id, "shortOffset", at) ?? "GMT";
  const m = /^GMT([+-])(\d{1,2})(?::(\d{2}))?$/.exec(g);
  if (!m) return "UTC+00:00";
  return `UTC${m[1]}${m[2].padStart(2, "0")}:${m[3] ?? "00"}`;
}

/** A real letter abbreviation (EDT, ACST) if the platform has one — NOT the "GMT+x" offset fallback. */
function abbreviation(id: string, at: Date): string | null {
  const s = zonePart(id, "short", at);
  return s && /^[A-Za-z]{2,5}$/.test(s) && s !== "GMT" && s !== "UTC" ? s : null;
}

function continentOf(id: string): string {
  return (id.split("/")[0] ?? id).replace(/_/g, " ");
}
/** The ICU legacy spellings runtimes still list, by the modern city name people know them by. A
 *  runtime that lists Asia/Calcutta would otherwise label it "Calcutta" and make "kolkata" find
 *  nothing. A small table of its own: timezones.ts's TZ_ALIASES is a coordinates table that covers
 *  only some of these. */
const LEGACY_CITY: Readonly<Record<string, string>> = {
  "Asia/Calcutta": "Kolkata",
  "Asia/Katmandu": "Kathmandu",
  "Asia/Saigon": "Ho Chi Minh",
  "Asia/Rangoon": "Yangon",
  "Europe/Kiev": "Kyiv",
  "America/Godthab": "Nuuk",
};

/** A zone's city, as its label ends it: "New York" for America/New_York. */
export function cityOf(id: string): string {
  return LEGACY_CITY[id] ?? (id.split("/").pop() ?? id).replace(/_/g, " ");
}

/** The short code over a gutter column: a real letter abbreviation (EST, EDT) when the platform has
 *  one, else the runtime's own offset ("GMT+5:30", "GMT", "GMT+1"). Computed for `at`, not cached
 *  like `zoneOption(...).code`, so it is right for the week on screen across a clock change. Empty
 *  when the id or the runtime can't say; the caller still has the city for its title. */
export function gutterZoneCode(id: string, at: Date): string {
  const abbrev = abbreviation(id, at);
  if (abbrev) return abbrev;
  const raw = zonePart(id, "shortOffset", at);
  if (!raw) return "";
  // One spelling whichever ICU is behind the webview ("GMT+0" or "GMT", "GMT+05:30" or "GMT+5:30"), so
  // a code reads the same on every platform.
  const m = /^GMT([+-])(\d{1,2})(?::?(\d{2}))?$/.exec(raw);
  if (!m) return raw;
  const hours = Number(m[2]);
  const mins = m[3] && m[3] !== "00" ? `:${m[3]}` : "";
  return hours === 0 && !mins ? "GMT" : `GMT${m[1]}${hours}${mins}`;
}

function build(id: string, at: Date): ZoneOption {
  const continent = continentOf(id);
  const city = cityOf(id);
  const country = countryName(id);
  const offset = utcOffset(id, at);
  const abbrev = abbreviation(id, at);
  // Drop the country segment when it just repeats the continent (Australia, Antarctica).
  const showCountry = country && country.toLowerCase() !== continent.toLowerCase() ? country : null;
  const label = showCountry ? `${continent} / ${showCountry} / ${city}` : `${continent} / ${city}`;
  // The modern city joins the haystack beside the id's own words, so both spellings find it.
  const search = [id.replace(/[/_]/g, " "), continent, country ?? "", city, offset, abbrev ?? ""]
    .join(" ")
    .toLowerCase();
  return { id, label, code: abbrev ?? offset, search };
}

// Cache: pure per id (the offset is derived once at first build — a session that crosses a DST change
// shows a slightly stale offset until reload, an accepted trade for not rebuilding Intl formatters on
// every keystroke over ~400 zones).
const cache = new Map<string, ZoneOption>();
export function zoneOption(id: string, at: Date = new Date()): ZoneOption {
  let o = cache.get(id);
  if (!o) {
    o = build(id, at);
    cache.set(id, o);
  }
  return o;
}

let allCache: ZoneOption[] | null = null;
/** Every runtime zone as a searchable option, computed once and reused. */
export function allZoneOptions(): ZoneOption[] {
  if (!allCache) {
    const at = new Date();
    allCache = allTimeZones().map((id) => zoneOption(id, at));
  }
  return allCache;
}
