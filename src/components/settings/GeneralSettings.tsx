// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";

import { useHelp } from "../../lib/help";
import { getSettings, setTimeZone } from "../../lib/ipc";
import { focusViewPrefsAreDefault, resetFocusViewPrefs } from "../../lib/focusPrefs";
import {
  calendarFiltersAreDefault,
  readOpenOn,
  readSwipeDays,
  resetCalendarFilters,
  SWIPE_DAYS_MAX,
  SWIPE_DAYS_MIN,
  writeOpenOn,
  writeSwipeDays,
  type SwipeDays,
} from "../../lib/calendarPrefs";
import {
  chatSectionsAreDefault,
  resetChatSections,
  resetShowThinking,
  showThinkingIsDefault,
} from "../../lib/chatPrefs";
import {
  briefingPrefsAreDefault,
  readBriefingFloat,
  readBriefingInSidebar,
  resetBriefingPrefs,
  subscribeBriefingPrefs,
  writeBriefingFloat,
  writeBriefingInSidebar,
  type BriefingFloat,
} from "../../lib/briefingPrefs";
import { setBriefingWindowVisible } from "../../lib/ipc";
import { useModsNoticeFolded } from "../../lib/useModsNoticeFolded";
import { TrayIconRow } from "./TrayIconRow";
import { readConfirmDelete, writeConfirmDelete } from "../../lib/pinboard/prefs";
import { navPlacementIsDefault, resetNavPlacement, useNavPlacement } from "../../lib/nav";
import {
  ACCENTS,
  accentName,
  allTimeZones,
  coordsForTimezone,
  deviceCoords,
  deviceTimeZone,
  EIGENGRAU,
  formatCoords,
  MONO_ACCENT,
  useTheme,
} from "../../theme";
import {
  Button,
  Callout,
  Input,
  SectionInfo,
  SectionLabel,
  SegmentedControl,
  Select,
  SettingRow,
  Toggle,
} from "../ui";
import { ResetLink, TabResetSection } from "./ResetControls";

/** The General Settings tab: appearance (system/mode/accent/depth/location), time zone, and
 *  help mode. Everything here already persists immediately — theme axes through the theme context,
 *  the time zone on change — so there is nothing to batch. Errors surface inline. */
export function GeneralSettings() {
  const {
    system,
    setSystem,
    mode,
    modePref,
    setModePref,
    modeSource,
    modeCoords,
    modeNextChange,
    autoLocation,
    setAutoLocation,
    depth,
    setDepth,
    accent,
    setAccent,
    fontScale,
    setFontScale,
    modelsVisible,
    setModelsVisible,
    appearanceIsDefault,
    resetAppearance,
  } = useTheme();
  const help = useHelp();
  // Seeded from localStorage rather than watched: the toggle is the only writer here, and the
  // Pinboard reads the pref fresh at the moment you click delete (see pinboard/prefs.ts).
  const [confirmDelete, setConfirmDelete] = useState(readConfirmDelete);
  // Where the Calendar tab opens. Stored as a mode string; surfaced as a plain on/off because
  // "today" is the default and "where I left it" is the opt-in.
  const [openOnLast, setOpenOnLast] = useState(() => readOpenOn() === "last");
  // How far a sideways swipe moves the day grids. Seeded once: this is its only writer, and the grids
  // read the pref at each swipe (`swipeDays`), so nothing needs telling when it changes.
  const [swipe, setSwipe] = useState<SwipeDays>(readSwipeDays);
  // Where the briefing shows, beyond the Focus tab. Both off by default. SUBSCRIBED, not read once:
  // the always-on-top window can be dismissed from its own ✕ or by the tray going off, and both of
  // those write the pref from outside this component (see the briefing://closed listener in App).
  const [briefingSidebar, setBriefingSidebar] = useState(readBriefingInSidebar);
  const [briefingFloat, setBriefingFloat] = useState<BriefingFloat>(readBriefingFloat);
  useEffect(
    () =>
      subscribeBriefingPrefs(() => {
        setBriefingSidebar(readBriefingInSidebar());
        setBriefingFloat(readBriefingFloat());
      }),
    [],
  );
  function changeBriefingSidebar(on: boolean) {
    setBriefingSidebar(on);
    writeBriefingInSidebar(on);
  }
  function changeBriefingFloat(mode: BriefingFloat) {
    setBriefingFloat(mode);
    writeBriefingFloat(mode);
    // The "always on top" level is a real OS window the Rust side owns, so tell it the state we
    // want. Explicitly SET, never toggle — a toggle asked to be "not on top" showed the hidden
    // window instead, which is how picking "inside PM" used to open both at once.
    void setBriefingWindowVisible(mode === "onTop").catch(() => {
      /* best-effort: the in-app levels don't need it and a failure must not block the setting */
    });
  }
  const [timeZone, setTimeZoneState] = useState("");
  const [tzAuto, setTzAuto] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // The stored time zone (always set by the time you reach this tab; onboarding records it first-run).
  useEffect(() => {
    let cancelled = false;
    getSettings()
      .then((s) => {
        if (cancelled || !s.time_zone) return;
        setTimeZoneState(s.time_zone);
        setTzAuto(s.time_zone === deviceTimeZone());
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  // Time zone saves immediately. Auto persists the device zone; Manual persists the selected zone.
  // The shared time/location context re-reads on the event.
  function changeTz(auto: boolean, zone: string) {
    setTzAuto(auto);
    setTimeZoneState(zone);
    void setTimeZone(auto ? deviceTimeZone() : zone)
      .then(() => window.dispatchEvent(new Event("pm:settings-changed")))
      .catch((e) => setError(String(e)));
  }

  // --- Reset-to-default (#445) ---
  const confirmDeleteIsDefault = confirmDelete;
  const briefingIsDefault =
    !briefingSidebar && briefingFloat === "off" && briefingPrefsAreDefault();
  // The Focus tab's view prefs (layout, Upcoming, panels) are set on the tab, not here — but "Reset
  // Focus" still restores them, so this section has to know whether there is anything to reset.
  // Subscribed: the Focus tab stays mounted and live behind the Settings overlay, so a read taken at
  // mount would go stale the moment the user changed something there and came back.
  const [focusViewDefault, setFocusViewDefault] = useState(focusViewPrefsAreDefault);
  useEffect(() => {
    const onChanged = () => setFocusViewDefault(focusViewPrefsAreDefault());
    window.addEventListener("pm:settings-changed", onChanged);
    return () => window.removeEventListener("pm:settings-changed", onChanged);
  }, []);
  const focusIsDefault = briefingIsDefault && focusViewDefault;
  const helpIsDefault = !help.enabled;
  // Which tabs the sidebar shows, and in what order. Pinned and hidden from a right-click on the tab
  // itself; only the Reset lives here. Live: the sidebar stays mounted behind this overlay.
  const sidebarIsDefault = navPlacementIsDefault(useNavPlacement());
  // The Agenda's Hide done and the All | All-day filter are set on the Calendar, not here, but Reset
  // still turns them off: they hide data, so a forgotten one must be undoable from somewhere. Read
  // again on the app-wide settings signal (which Reset sends), as the Focus prefs above are.
  const [calendarFiltersDefault, setCalendarFiltersDefault] = useState(calendarFiltersAreDefault);
  useEffect(() => {
    const onChanged = () => setCalendarFiltersDefault(calendarFiltersAreDefault());
    window.addEventListener("pm:settings-changed", onChanged);
    return () => window.removeEventListener("pm:settings-changed", onChanged);
  }, []);
  const calendarIsDefault = !openOnLast && swipe === SWIPE_DAYS_MIN && calendarFiltersDefault;
  // The Chats-tab sidebar folds, same story as the Focus prefs above: the control lives beside what
  // it changes (one control, one home) and only the RESET lives here, so this needs the same live
  // subscription rather than a read at mount. There is no Chats section on this tab to hang a
  // dedicated ResetLink off, so the tab-level "Reset General" is its only home. The chat composer's
  // Thinking toggle rides the same flag: its only Settings presence is this tab-level reset.
  const [chatSectionsDefault, setChatSectionsDefault] = useState(
    () => chatSectionsAreDefault() && showThinkingIsDefault(),
  );
  useEffect(() => {
    const onChanged = () =>
      setChatSectionsDefault(chatSectionsAreDefault() && showThinkingIsDefault());
    window.addEventListener("pm:settings-changed", onChanged);
    return () => window.removeEventListener("pm:settings-changed", onChanged);
  }, []);
  // The Mods notice, folded on a mod's first screen by "Don't show this again". Its one home is the
  // row below (it appears only while folded); Reset General brings the notice back too.
  const modsNotice = useModsNoticeFolded();
  // The whole tab, minus the deliberately-excluded time zone (device-derived, not a preference).
  const generalIsDefault =
    appearanceIsDefault &&
    confirmDeleteIsDefault &&
    focusIsDefault &&
    calendarIsDefault &&
    chatSectionsDefault &&
    sidebarIsDefault &&
    !modsNotice.folded &&
    helpIsDefault;

  function resetConfirmDelete() {
    setConfirmDelete(true);
    writeConfirmDelete(true);
  }
  function resetFocus() {
    // Everything the Focus tab owns (layout, Upcoming mode / hours / days, panel visibility) —
    // the defaults live in focusPrefs beside the readers, so this never re-lists them.
    resetFocusViewPrefs();
    setFocusViewDefault(true);
    setBriefingSidebar(false);
    setBriefingFloat("off");
    resetBriefingPrefs();
    void setBriefingWindowVisible(false).catch(() => {});
  }
  function resetCalendar() {
    setOpenOnLast(false);
    writeOpenOn("today");
    setSwipe(SWIPE_DAYS_MIN);
    writeSwipeDays(SWIPE_DAYS_MIN);
    // Hide done and All-day, back to showing everything; the open Calendar follows on the signal.
    resetCalendarFilters();
    setCalendarFiltersDefault(true);
  }
  function resetGeneral() {
    resetAppearance();
    resetConfirmDelete();
    resetFocus();
    resetCalendar();
    resetChatSections(); // the Chats-tab sidebar folds, back to density-derived
    resetShowThinking(); // the chat Thinking toggle, back to off
    setChatSectionsDefault(true);
    resetNavPlacement(); // every tab back where it ships, nothing pinned or hidden
    help.setEnabled(false);
    void modsNotice.showAgain(); // the Mods notice, back on a new mod's first screen
    // Time zone is intentionally left alone — it's derived from the device, not a taste preference.
  }

  return (
    <>
      {error && <Callout className="mt-4">{error}</Callout>}

      <div
        id="sec-general-appearance"
        data-settings-section
        className="mt-5 border-t border-border pt-4"
        data-help="settings-appearance"
      >
        <SectionLabel
          action={
            !appearanceIsDefault && <ResetLink onReset={resetAppearance} label="Reset appearance" />
          }
        >
          Appearance
        </SectionLabel>
        <SettingRow label="System">
          {(a11y) => (
            <SegmentedControl
              {...a11y}
              value={system}
              onChange={setSystem}
              options={[
                {
                  value: "editorial",
                  label: "Editorial",
                  title: "Editorial — serif headings, warm paper tones",
                },
                {
                  value: "slate",
                  label: "Slate",
                  title: "Slate — clean sans, cool neutrals (default)",
                },
                {
                  value: "terminal",
                  label: "Terminal",
                  title: "Terminal — monospace, high contrast",
                },
              ]}
            />
          )}
        </SettingRow>
        <SettingRow label="Mode">
          {(a11y) => (
            <SegmentedControl
              {...a11y}
              value={modePref}
              onChange={setModePref}
              options={[
                { value: "light", label: "Light" },
                { value: "dark", label: "Dark" },
                {
                  value: "system",
                  label: "System",
                  title: "Follow your device's light/dark setting",
                },
                {
                  value: "auto",
                  label: "Auto",
                  title: "Follow sunrise and sunset at your location",
                },
              ]}
            />
          )}
        </SettingRow>
        {modePref === "system" && (
          <p className="mt-1.5 text-xs text-ink4">
            Following your device's light/dark setting — currently{" "}
            {mode === "dark" ? "dark" : "light"}.
          </p>
        )}
        {modePref === "auto" && (
          <>
            <p className="mt-1.5 text-xs text-ink4">
              {modeSource === "auto" ? (
                <>
                  Follows sunrise &amp; sunset — currently {mode === "dark" ? "dark" : "light"}
                  {modeCoords ? ` · ${formatCoords(modeCoords)}` : ""}
                  {modeNextChange
                    ? ` · switches to ${mode === "dark" ? "light" : "dark"} at ${modeNextChange.toLocaleTimeString(
                        [],
                        { hour: "2-digit", minute: "2-digit" },
                      )}`
                    : ""}
                  .
                </>
              ) : (
                <>
                  Couldn't determine your location, so it's following your device's light/dark
                  setting for now. Enter a location below for sunrise &amp; sunset.
                </>
              )}
            </p>
            <div className="mt-2 flex items-center justify-between gap-3">
              <span className="text-sm text-ink2">Location</span>
              <div className="flex items-center gap-2">
                <Input
                  value={autoLocation}
                  onChange={(e) => setAutoLocation(e.target.value)}
                  placeholder={
                    deviceCoords()
                      ? `${formatCoords(deviceCoords()!)} (detected)`
                      : "e.g. 51.51, -0.13"
                  }
                  aria-label="Location for sunrise and sunset, as latitude, longitude"
                  className="w-44"
                />
                {autoLocation && (
                  <button
                    type="button"
                    className="text-xs text-ink4 transition hover:text-ink"
                    onClick={() => setAutoLocation("")}
                  >
                    Reset
                  </button>
                )}
              </div>
            </div>
          </>
        )}
        <SettingRow label="Depth">
          {(a11y) => (
            <SegmentedControl
              {...a11y}
              value={depth}
              onChange={setDepth}
              options={[
                { value: "min", label: "Min" },
                { value: "standard", label: "Standard" },
                { value: "power", label: "Power" },
              ]}
            />
          )}
        </SettingRow>
        <SettingRow label="Accent">
          {(a11y) => (
            <div {...a11y} role="group" className="flex items-center gap-1.5">
              {ACCENTS[system].map((hex) => {
                const isMono = hex === MONO_ACCENT;
                const name = accentName(hex);
                return (
                  <button
                    key={hex}
                    type="button"
                    aria-label={isMono ? "Monochrome (Eigengrau)" : `Accent: ${name}`}
                    title={isMono ? "Monochrome — Eigengrau base, white text & accents" : name}
                    onClick={() => setAccent(hex)}
                    style={{
                      background: isMono ? EIGENGRAU : hex,
                      // The Eigengrau swatch is near-black; a white rim makes it legible and
                      // signals the "white accents" treatment.
                      border: isMono ? "1px solid rgba(255,255,255,0.55)" : undefined,
                    }}
                    className={`relative h-5 w-5 rounded-full transition before:absolute before:-inset-[2px] before:content-[''] ${
                      accent === hex
                        ? "ring-2 ring-ink ring-offset-2 ring-offset-[var(--surface)]"
                        : ""
                    }`}
                  />
                );
              })}
            </div>
          )}
        </SettingRow>
        {/* Text size — mirrored from the Accessibility tab (one setter). A mainstream comfort control,
            not only an accessibility need, so it's surfaced here too. */}
        <SettingRow label="Text size">
          {(a11y) => (
            <SegmentedControl
              {...a11y}
              value={fontScale}
              onChange={setFontScale}
              options={[
                { value: "small", label: "Small", title: "90%" },
                { value: "default", label: "Default", title: "100%" },
                { value: "large", label: "Large", title: "115%" },
                { value: "xlarge", label: "XL", title: "130%" },
              ]}
            />
          )}
        </SettingRow>
        <SettingRow
          label="Confirm before deleting a pinboard card"
          helpId="settings-pinboard-confirm-delete"
          aside={!confirmDeleteIsDefault && <ResetLink onReset={resetConfirmDelete} />}
        >
          {(a11y) => (
            <Toggle
              {...a11y}
              checked={confirmDelete}
              onChange={(on) => {
                setConfirmDelete(on);
                writeConfirmDelete(on);
              }}
            />
          )}
        </SettingRow>
        {/* The tabs are pinned and hidden where they are, from a right-click in the sidebar (one
            control, one home), so this row holds only the way back. The description says where
            the controls went: without it, the Review & Teach and Map switches that used to sit here
            would simply have vanished. The button stays when there is nothing to undo, disabled,
            so the row still says it exists. */}
        <SettingRow
          label="Sidebar tabs"
          helpId="settings-sidebar-placement"
          description="Right-click a tab in the sidebar to pin it to the top or hide it."
        >
          {(a11y) => (
            <Button
              size="sm"
              aria-describedby={a11y["aria-describedby"]}
              disabled={sidebarIsDefault}
              onClick={resetNavPlacement}
              className="shrink-0"
            >
              Reset sidebar placement
            </Button>
          )}
        </SettingRow>
        <SettingRow label="Calendar opens where you left it" helpId="settings-calendar-open-on">
          {(a11y) => (
            <Toggle
              {...a11y}
              checked={openOnLast}
              onChange={(v) => {
                setOpenOnLast(v);
                writeOpenOn(v ? "last" : "today");
              }}
            />
          )}
        </SettingRow>
        <SettingRow
          label="A sideways swipe moves the calendar"
          helpId="settings-calendar-swipe-days"
        >
          {(a11y) => (
            <Select
              {...a11y}
              value={String(swipe)}
              onChange={(e) => {
                const raw = e.currentTarget.value;
                const next: SwipeDays = raw === "view" ? "view" : Number(raw);
                setSwipe(next);
                writeSwipeDays(next);
              }}
            >
              {Array.from({ length: SWIPE_DAYS_MAX - SWIPE_DAYS_MIN + 1 }, (_, i) => {
                const n = SWIPE_DAYS_MIN + i;
                return (
                  <option key={n} value={n}>
                    {n === 1 ? "1 day" : `${n} days`}
                  </option>
                );
              })}
              <option value="view">All the days on screen</option>
            </Select>
          )}
        </SettingRow>
        <SettingRow label="Models in use" helpId="settings-models-footer">
          {(a11y) => <Toggle {...a11y} checked={modelsVisible} onChange={setModelsVisible} />}
        </SettingRow>
        {/* Appearance's explanation folds into this one disclosure at the foot. The auto-mode status
            line and its "couldn't find your location" fallback stay inline above: they're a readout
            and a gating hint, not explanation.

            It covers the five controls with no explanation anywhere else — System, Mode, Depth,
            Accent, Text size — and deliberately stops there. The pinboard confirm toggle and the
            sidebar tabs row already carry their own help-mode entries in lib/help.ts
            (`settings-pinboard-confirm-delete`, `settings-sidebar-placement`), and repeating those
            here would be a second copy to keep in step. Depth is the reason this was worth writing
            at all: it's the only control in the section with neither an option tooltip nor a word
            of prose. */}
        <SectionInfo title="What these settings do">
          <p>Everything here applies instantly and is remembered on this device.</p>
          <p>
            <strong>System</strong> is the overall look: Editorial has serif headings and warm paper
            tones, Slate is a clean sans on cool neutrals, Terminal is monospace and high contrast.
            It also decides which accent colours you can pick from.
          </p>
          <p>
            <strong>Mode</strong> is light or dark. System follows your device's setting; Auto
            follows sunrise and sunset where you are, so PM goes dark in the evening on its own.
          </p>
          <p>
            <strong>Depth</strong> controls how much detail PM shows — it reveals and hides
            information, it never rearranges the screen. Min keeps things spare: no meta lines,
            slightly larger type, more breathing room. Standard adds the secondary detail — meta
            lines, model footers, extra columns. Power adds the numbers you'd want if you're keeping
            an eye on the machinery: token counts, what a call cost, exact timestamps and elapsed
            times, and keyboard hints.
          </p>
          <p>
            <strong>Accent</strong> is the highlight colour used for selection, links and focus
            rings. The near-black swatch is Monochrome — it drops colour altogether and uses white
            for accents instead.
          </p>
          <p>
            <strong>Text size</strong> scales the whole interface's type. It's the same control as
            the one on the Accessibility tab, so changing it in either place changes both.
          </p>
          {modePref === "auto" && (
            <p>
              Location is a latitude, longitude pair. Blank uses your device's timezone
              {(() => {
                try {
                  const tz = Intl.DateTimeFormat().resolvedOptions().timeZone;
                  return tz && coordsForTimezone(tz) ? ` (${tz})` : "";
                } catch {
                  return "";
                }
              })()}
              . Nothing about your location leaves this device.
            </p>
          )}
        </SectionInfo>
      </div>

      <div
        id="sec-general-focus"
        data-settings-section
        className="mt-5 border-t border-border pt-4"
      >
        <SectionLabel
          action={!focusIsDefault && <ResetLink onReset={resetFocus} label="Reset Focus" />}
        >
          Focus
        </SectionLabel>
        <SettingRow label="Show today’s briefing in the sidebar" helpId="settings-briefing-sidebar">
          {(a11y) => (
            <Toggle {...a11y} checked={briefingSidebar} onChange={changeBriefingSidebar} />
          )}
        </SettingRow>
        <SettingRow label="Floating briefing" helpId="settings-briefing-window">
          {(a11y) => (
            <SegmentedControl
              {...a11y}
              value={briefingFloat}
              onChange={changeBriefingFloat}
              options={[
                { value: "off", label: "Off" },
                { value: "inApp", label: "Inside PM", title: "A panel inside PM's own window" },
                {
                  value: "onTop",
                  label: "Always on top",
                  title: "A separate window that floats over other apps",
                },
              ]}
            />
          )}
        </SettingRow>
        <TrayIconRow helpId="settings-tray-icon" />
        <SectionInfo helpId="settings-focus">
          <p>
            The Focus tab&rsquo;s own layout, Upcoming and panel controls live on the tab itself,
            beside the thing they change — nothing here mirrors them. What this section covers is
            where the briefing shows up <em>outside</em> that tab. &ldquo;Reset Focus&rdquo; puts
            all of it back, the tab&rsquo;s controls included.
          </p>
        </SectionInfo>
      </div>

      <div
        id="sec-general-timezone"
        data-settings-section
        className="mt-5 border-t border-border pt-4"
        data-help="settings-timezone"
      >
        <SectionLabel>Time zone</SectionLabel>
        <SettingRow label="Detection">
          {(a11y) => (
            <SegmentedControl
              {...a11y}
              value={tzAuto ? "auto" : "manual"}
              onChange={(v) => changeTz(v === "auto", timeZone)}
              options={[
                { value: "auto", label: "Auto" },
                { value: "manual", label: "Manual" },
              ]}
            />
          )}
        </SettingRow>
        {!tzAuto && (
          <SettingRow label="Zone">
            {(a11y) => (
              <Select
                {...a11y}
                value={timeZone}
                onChange={(e) => changeTz(false, e.target.value)}
                className="max-w-[14rem]"
              >
                {allTimeZones().map((z) => (
                  <option key={z} value={z}>
                    {z}
                  </option>
                ))}
              </Select>
            )}
          </SettingRow>
        )}
        {/* The zone in force is a readout — it stays visible; only what the zone *means* folds away. */}
        <p className="mt-2 text-xs text-ink4">
          {tzAuto ? `Following this device: ${deviceTimeZone()}` : `Selected: ${timeZone || "—"}`}
        </p>
        <SectionInfo title="What the time zone affects">
          <p>Sets “today”, “due soon”, and your calendar agenda. Auto follows this device.</p>
        </SectionInfo>
      </div>

      {modsNotice.folded && (
        <div className="mt-4 border-t border-border pt-4">
          <SettingRow
            label="Mods notice"
            helpId="settings-mods-notice"
            description="You chose not to see the whole Mods notice again, so it is folded to one line on a new mod's first screen."
            spacing="none"
          >
            {(a11y) => (
              <Button
                size="sm"
                aria-describedby={a11y["aria-describedby"]}
                onClick={() => void modsNotice.showAgain()}
                className="shrink-0"
              >
                Show it again
              </Button>
            )}
          </SettingRow>
        </div>
      )}

      <div
        id="sec-general-help"
        data-settings-section
        className="mt-4 border-t border-border pt-4"
        data-help="settings-help-mode"
      >
        <SettingRow
          label="Help mode"
          emphasis="strong"
          spacing="none"
          aside={!helpIsDefault && <ResetLink onReset={() => help.setEnabled(false)} />}
        >
          {(a11y) => <Toggle {...a11y} checked={help.enabled} onChange={help.setEnabled} />}
        </SettingRow>
        <SectionInfo title="What is help mode?">
          <p>
            When on, hovering any highlighted section shows a short explanation of what it does.
          </p>
        </SectionInfo>
      </div>

      <TabResetSection
        tabName="General"
        isDefault={generalIsDefault}
        onReset={resetGeneral}
        confirmBody={
          <>
            Restores appearance (System, Mode, Accent, Depth, Location), the pinboard delete
            confirmation, the Focus tab (which panels it shows and where the briefing appears),
            where the Calendar opens, how far a swipe moves it and its Hide done and All-day filters
            (off), the Chats sidebar's folds, the chat Thinking button (back to off), the sidebar's
            tabs (nothing pinned or hidden), the Mods notice (shown in full again on a new mod's
            first screen), and help mode to their defaults. Your time zone is left as-is.
          </>
        }
      />
    </>
  );
}
