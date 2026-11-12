// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Day/Week grid's top-left gutter corner. Two full-width rows, stacked: the calendar's "+ New
// event" (`corner`, one line across the whole gutter), then the zone row: a short code over each hour
// column ("GMT+5:30 · GMT"), which is ONE button. Clicking it opens the zone menu, the one home for
// adding and removing an extra timezone (it retired #355's hover ✕: a single button can't hold
// buttons). The codes are the offset, or a letter code like EST where the platform has one, for the
// week on screen; the city is on hover, in the accessible name and in the menu. The local column
// stays nearest the grid and is never removable. Widths are rem and come from one string
// (`gutterWidth`), shared with the hour gutter and the all-day band, so they grow with the Text size
// and can't drift. Token-driven.
//
// "+ New" is in the corner because the corner had room the header didn't: the header's left end now
// starts with Today.

import { Fragment, useMemo, type ReactNode } from "react";
import { deviceTimeZone } from "../../theme";
import { MAX_EXTRA_ZONES, gutterWidth, localColWidth, zoneColWidth } from "../../lib/calendarPrefs";
import { cityOf, gutterZoneCode, zoneOption } from "../../lib/zoneLabel";
import { Button, Popover, VisuallyHidden, cn } from "../ui";
import { ZoneSearchList } from "./ZoneSearchList";

interface Props {
  zones: string[];
  onChange: (zones: string[]) => void;
  /** The day the codes are read at: a zone's offset changes with its clocks, so they follow the
   *  week shown (as the hour labels do). */
  at: Date;
  /** When false, the zone row is not offered, for a compact embed (the Focus Upcoming grid) that has
   *  no room for extra zone columns and always runs with `zones = []`. Defaults to true. */
  allowAdd?: boolean;
  /** Drawn in the first row, across the whole gutter: the calendar's New event. */
  corner?: ReactNode;
}

/** The corner: "+ New", then the zone row. Width mirrors the body gutter (local + one column per
 *  extra zone). */
export function ZoneGutter({ zones, onChange, at, allowAdd = true, corner }: Props) {
  const local = deviceTimeZone();
  // Extra zones left to right, then this computer's: the order of the columns, local nearest the grid.
  const columns = useMemo(
    () =>
      [...zones, local].map((zone) => ({
        zone,
        city: cityOf(zone),
        code: gutterZoneCode(zone, at),
      })),
    [zones, local, at],
  );
  const title =
    zones.length > 0
      ? `${columns.map((c) => `${c.city} ${c.code}`.trim()).join(" · ")}. Add or remove time zones`
      : "Add a time zone";

  return (
    <div className="flex shrink-0 flex-col" style={{ width: gutterWidth(zones.length) }}>
      {corner && <div className="px-0.5">{corner}</div>}
      {allowAdd && (
        <Popover
          align="left"
          ariaLabel="Time zones"
          rootClassName="w-full"
          panelClassName="min-w-[19rem] p-2"
          trigger={({ toggle, open }) => (
            <button
              type="button"
              onClick={toggle}
              aria-expanded={open}
              title={title}
              className={cn(
                "flex min-h-[var(--tap-min,24px)] w-full items-stretch rounded-[var(--radius-sm)] font-mono text-[0.5625rem] uppercase leading-none tracking-tight text-ink4 transition hover:bg-surface hover:text-ink",
                zones.length === 0 && "justify-center border border-border2",
              )}
            >
              {zones.length === 0 ? (
                <span className="self-center px-0.5 text-[0.625rem] whitespace-nowrap">+ Zone</span>
              ) : (
                <>
                  {columns.map((c, i) => {
                    const isLocal = i === zones.length;
                    return (
                      <Fragment key={`${i}-${c.zone}`}>
                        <Cell
                          width={isLocal ? localColWidth : zoneColWidth}
                          ruled={!isLocal}
                          code={c.code}
                          city={c.city}
                        />
                        {/* The space between cells, for a screen reader (a flex row draws none). */}{" "}
                      </Fragment>
                    );
                  })}
                  <VisuallyHidden>change time zones</VisuallyHidden>
                </>
              )}
            </button>
          )}
        >
          {({ close }) => (
            <ZoneMenu zones={zones} local={local} at={at} onChange={onChange} close={close} />
          )}
        </Popover>
      )}
    </div>
  );
}

/** One column's code, right-aligned over its hour labels, with its city for assistive tech (after the
 *  visible code, so the name starts with what is on screen). */
function Cell({
  width,
  code,
  city,
  ruled = false,
}: {
  width: string;
  code: string;
  city: string;
  ruled?: boolean;
}) {
  return (
    <span
      className={cn(
        "flex shrink-0 items-center justify-end px-0.5 whitespace-nowrap",
        ruled && "border-l border-rule",
      )}
      style={{ width }}
    >
      {code}{" "}
      {/* The comma keeps a screen reader from running one cell into the next ("GMT+5:30 Kolkata, GMT
          London, change time zones"). The space after the code is a text node of its own: a flex
          container draws none, and it survives where a hidden span's own is trimmed. */}
      <VisuallyHidden>{city},</VisuallyHidden>
    </span>
  );
}

/** The zone menu: each extra zone with Remove, this computer's zone (not removable), then the search
 *  to add one, or a note at the cap. */
function ZoneMenu({
  zones,
  local,
  at,
  onChange,
  close,
}: {
  zones: string[];
  local: string;
  at: Date;
  onChange: (zones: string[]) => void;
  close: () => void;
}) {
  const atCap = zones.length >= MAX_EXTRA_ZONES;
  return (
    <div className="space-y-2">
      <ul className="space-y-0.5">
        {zones.map((z) => (
          <li key={z} className="flex items-center justify-between gap-2 px-2 text-xs text-ink2">
            <span className="min-w-0 truncate" title={z}>
              {zoneOption(z, at).label}
            </span>
            <span className="flex shrink-0 items-center gap-2">
              <span className="font-mono text-[0.625rem] text-ink4">{gutterZoneCode(z, at)}</span>
              <Button
                variant="tertiary"
                size="xs"
                aria-label={`Remove ${cityOf(z)}`}
                onClick={() => {
                  onChange(zones.filter((x) => x !== z));
                  // The Remove button unmounts with its row; closing hands focus back to the zone
                  // row (as adding does) rather than dropping it on the page.
                  close();
                }}
              >
                Remove
              </Button>
            </span>
          </li>
        ))}
        <li className="flex items-center justify-between gap-2 px-2 text-xs text-ink3">
          <span className="min-w-0 truncate" title={local}>
            {zoneOption(local, at).label} · this computer
          </span>
          <span className="shrink-0 font-mono text-[0.625rem] text-ink4">
            {gutterZoneCode(local, at)}
          </span>
        </li>
      </ul>
      {atCap ? (
        <p className="px-2 text-[0.6875rem] text-ink4">
          Up to {MAX_EXTRA_ZONES} extra time zones. Remove one to add another.
        </p>
      ) : (
        <ZoneSearchList
          // This computer's zone is already a column (listed above), so offering it would add a
          // twin with an identical code.
          exclude={[...zones, local]}
          autoFocus
          onPick={(zone) => {
            onChange([...zones, zone]);
            close();
          }}
        />
      )}
    </div>
  );
}
