// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// One installed mod in the Mods tab (Mods spec §1.13): its icon, name, version, tier, how it stands
// (the backend's sentence, with PM's short label beside it), the one thing that brings it back when
// it can't run, Pause, and, folded under Details, what it is and what it may do, with each
// permission's state and an Allow / Turn off for it, and Uninstall.
//
// Everything the mod supplied (its name, description, author, repository, the reasons it gives for
// its permissions) is drawn as plain text; what each permission means is PM's wording; the tier, the
// standing, each permission's access and every refusal are the backend's. The row decides nothing:
// each button sends one command and the row changes when the rows are read again.
//
// Every command goes through `runModAction` (lib/modActions): one at a time per mod, and it runs to
// its end if the person leaves the Mods tab, so coming back shows "Pausing…" or the refusal rather
// than an idle row that invites a second press.

import {
  approveUpdate,
  modAiCapSet,
  modPermissionDecide,
  pauseMod,
  repairMod,
  resumeMod,
} from "../../lib/ipc";
import { useRememberedFold } from "../../lib/foldPrefs";
import { clearModError, runModAction, setModScreen, useModActions } from "../../lib/modActions";
import { modFoldKey } from "../../lib/modFolds";
import { useModTab } from "../../lib/nav";
import {
  pausedForDevMode,
  type ModInfo,
  type ModPermission,
  type ModStandingKind,
} from "../../lib/modsTypes";
import { Button, Callout, Card, Collapsible, VisuallyHidden } from "../ui";
import { toneSurface } from "../ui/tone";
import { DevModeOffMark } from "./DevModeOffMark";
import { ModAiLimit } from "./ModAiLimit";
import { ModIconView } from "./ModIconView";
import { ModSecretFields } from "./ModSecretFields";
import { modText } from "./modText";
import { classWords, permissionWords, standingLine } from "./permissionWords";
import { TierBadge } from "./TierBadge";

/** PM's short label for each standing, and whether it is one the person should look at. A kind PM
 *  doesn't know is "Not running": never blank, never claimed to be fine. */
const STANDING: Record<ModStandingKind, { label: string; attention: boolean }> = {
  running: { label: "On", attention: false },
  "waiting-for-you": { label: "Waiting for you", attention: true },
  "paused-by-you": { label: "Paused", attention: false },
  "paused-frozen": { label: "Stopped responding", attention: true },
  "paused-by-safety-list": { label: "Paused by the safety list", attention: true },
  "removed-by-safety-list": { label: "Removed by the safety list", attention: true },
  // The reason is the mark beside the name (DevModeOffMark), so this says only that it is paused.
  "paused-dev-mode": { label: "Paused", attention: true },
  "files-missing": { label: "Files missing", attention: true },
  "files-changed": { label: "Files changed", attention: true },
  "record-unreadable": { label: "Can't be read", attention: true },
  "api-unsupported": { label: "Doesn't work with this PM", attention: true },
  parked: { label: "Parked", attention: true },
  installing: { label: "Installing", attention: false },
  "paused-other": { label: "Paused", attention: true },
};
const UNKNOWN_STANDING = { label: "Not running", attention: true };

const SOURCE: Record<string, string> = {
  catalogue: "From the catalogue.",
  file: "Installed from a file.",
  folder: "A test mod loaded from a folder. It stays only until PM restarts.",
};

function permissionStanding(p: ModPermission): string {
  if (p.decision === "denied") return p.offBecause ?? "Off: you said no.";
  if (p.access === "allowed") {
    // First party's `ai` is allowed with no answer on record, and says what still comes: the money
    // question, at the first request that would cost something.
    return p.id === "ai" && p.when === "auto" && p.decision == null ? standingLine(p) : "Allowed.";
  }
  if (p.access === "asks") return "PM asks you the first time the mod uses it.";
  return p.offBecause ?? standingLine(p);
}

export function ModRow({
  mod,
  refresh,
  onUninstall,
  onTurnOn,
  quiet,
}: {
  mod: ModInfo;
  /** Read the rows again (the Mods tab's shared read). */
  refresh: () => Promise<void>;
  /** Open the uninstall dialog for this mod. */
  onUninstall: (id: string) => void;
  /** Open the first-open screen for a mod PM installed and the person hasn't turned on. */
  onTurnOn: (id: string) => void;
  /** A dialog for this mod is open and shows its own refusals: don't say them twice. */
  quiet: boolean;
}) {
  const { busy, errors } = useModActions();
  const tab = useModTab(mod.id);
  const standing = STANDING[mod.state.kind] ?? UNKNOWN_STANDING;
  const [open, setOpen] = useRememberedFold(modFoldKey(mod.id), standing.attention || !!mod.update);
  const [permsOpen, setPermsOpen] = useRememberedFold(modFoldKey(mod.id, "permissions"), true);

  const working = busy.get(mod.id) ?? null;
  const error = quiet ? null : (errors.get(mod.id) ?? null);

  function act(doing: string, failure: string, action: () => Promise<unknown>) {
    clearModError(mod.id);
    void runModAction(mod.id, doing, failure, action).then(() => refresh());
  }

  const { kind, fix, text } = mod.state;
  const removed = kind === "removed-by-safety-list";
  const canDecide = mod.consented && !removed;
  // The mod's own words, made one plain line each (invisible and direction-changing characters
  // can't reorder what is drawn beside them).
  const name = modText(mod.name);

  return (
    <li>
      <Card className="space-y-3 p-4" data-mod-id={mod.id} data-help="mods-installed-row">
        <div className="flex items-start gap-3">
          <ModIconView modId={mod.id} version={mod.version} name={name} hasIcon={mod.hasIcon} />
          <div className="min-w-0 flex-1 space-y-1">
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
              {/* Clipped to its own box: a name of stacked accents can't paint over the badges. */}
              <h3 className="overflow-hidden break-words font-head text-sm font-semibold text-ink">
                {name}
              </h3>
              <span className="text-xs text-ink3">version {mod.version}</span>
              {pausedForDevMode(mod) && <DevModeOffMark />}
            </div>
            <div className="flex flex-wrap items-center gap-1.5 text-xs">
              <TierBadge tier={mod.tier} />
              {mod.installedFrom === "folder" && (
                <span className="rounded-full border border-border2 px-2 py-px text-ink2">
                  Test
                </span>
              )}
              <span
                className="rounded-full border px-2 py-px"
                style={standing.attention ? toneSurface("warning") : undefined}
              >
                {standing.label}
              </span>
              {mod.update && (
                <span className="rounded-full border border-border2 px-2 py-px text-ink2">
                  Update waiting
                </span>
              )}
            </div>
            {text && <p className="break-words text-sm text-ink2">{modText(text)}</p>}
          </div>
          <div className="flex flex-none flex-wrap justify-end gap-2">
            {fix === "turn-on" && (
              <Button
                variant="primary"
                disabled={working !== null}
                aria-label={`Review and turn on ${name}`}
                onClick={() => onTurnOn(mod.id)}
              >
                Review and turn on
              </Button>
            )}
            {fix === "resume" && (
              <Button
                variant="primary"
                disabled={working !== null}
                aria-label={`Resume ${name}`}
                onClick={() =>
                  act("Resuming", "PM couldn't resume this mod.", () => resumeMod(mod.id))
                }
              >
                Resume
              </Button>
            )}
            {fix === "repair" && (
              <Button
                variant="primary"
                disabled={working !== null}
                aria-label={`Repair ${name}`}
                onClick={() =>
                  act("Repairing", "PM couldn't repair this mod.", () => repairMod(mod.id))
                }
              >
                Repair
              </Button>
            )}
            {fix === "uninstall" && (
              <Button
                variant="danger"
                disabled={working !== null}
                aria-label={`Uninstall ${name}`}
                onClick={() => onUninstall(mod.id)}
              >
                Uninstall…
              </Button>
            )}
            {mod.state.running && (
              <Button
                variant="secondary"
                disabled={working !== null}
                aria-label={`Pause ${name}`}
                onClick={() =>
                  act("Pausing", "PM couldn't pause this mod.", () => pauseMod(mod.id))
                }
              >
                Pause
              </Button>
            )}
            {tab.state === "hidden" && (
              <Button variant="secondary" aria-label={`Show tab for ${name}`} onClick={tab.show}>
                Show tab
              </Button>
            )}
          </div>
        </div>

        {working && (
          <p className="text-xs text-ink3" role="status">
            {working}…
          </p>
        )}

        {mod.update && (
          <Callout tone="warning" size="md" live={false}>
            <p className="break-words">{modText(mod.update.text)}</p>
            {mod.update.kind === "approval" && (
              <div className="pt-2">
                <Button
                  variant="primary"
                  disabled={working !== null}
                  aria-label={`Review update for ${name}`}
                  onClick={() =>
                    act("Getting the update", "PM couldn't get the update.", async () =>
                      // `restart` is how its screen is asked for again if it runs out of time.
                      setModScreen(mod.id, await approveUpdate(mod.id), () =>
                        approveUpdate(mod.id),
                      ),
                    )
                  }
                >
                  Review update
                </Button>
              </div>
            )}
          </Callout>
        )}

        {error && (
          <Callout size="md">
            <p className="break-words">{error}</p>
            <div className="pt-1">
              <Button variant="tertiary" size="sm" onClick={() => clearModError(mod.id)}>
                Dismiss
              </Button>
            </div>
          </Callout>
        )}

        <Collapsible
          title={
            <>
              Details <VisuallyHidden>of {name}</VisuallyHidden>
            </>
          }
          open={open}
          onOpenChange={setOpen}
        >
          <div className="space-y-3 pt-2 text-sm text-ink2">
            {mod.description && <p className="break-words">{modText(mod.description)}</p>}
            <p className="break-words text-xs text-ink3">
              {mod.author ? `Made by ${modText(mod.author)}` : "Maker not named"}
              {mod.repo ? ` · ${modText(mod.repo)}` : ""}
              {mod.tier === "second" && mod.commit
                ? ` · reviewed by Atlas at commit ${modText(mod.commit).slice(0, 12)}`
                : ""}
              {" · "}
              {SOURCE[mod.installedFrom] ?? "Source unknown."}
            </p>

            <div data-help="mods-permissions">
              <Collapsible
                title={
                  <>
                    What it can do <VisuallyHidden>for {name}</VisuallyHidden>
                  </>
                }
                meta={mod.permissions.length}
                open={permsOpen}
                onOpenChange={setPermsOpen}
              >
                {mod.permissions.length === 0 ? (
                  <p className="pt-2 text-ink3">It doesn&apos;t ask for anything.</p>
                ) : (
                  <ul className="space-y-2 pt-2">
                    {mod.permissions.map((p) => {
                      const words = permissionWords(p.id);
                      const cls = classWords(p.class);
                      const on = p.access !== "off";
                      return (
                        <li
                          key={p.id}
                          className="rounded-[var(--radius-sm)] border border-border p-2"
                        >
                          <div className="flex items-start justify-between gap-3">
                            <div className="min-w-0 space-y-0.5">
                              <p className="font-medium text-ink">
                                {words}
                                {cls && (
                                  <span className="pl-2 text-xs font-normal text-ink3">{cls}</span>
                                )}
                              </p>
                              {p.why && (
                                <p className="break-words text-xs text-ink3">
                                  The maker says: &ldquo;{modText(p.why)}&rdquo;
                                </p>
                              )}
                              <p className="text-xs text-ink2">{permissionStanding(p)}</p>
                            </div>
                            {canDecide && (
                              <div className="flex flex-none flex-wrap justify-end gap-2">
                                {/* "Asks" counts as on (a "Turn off" records the no), but it is
                                    not yet a yes: "Allow now" answers the prompt ahead of time. */}
                                {p.access === "asks" && (
                                  <Button
                                    variant="secondary"
                                    size="sm"
                                    disabled={working !== null}
                                    aria-label={`Allow now: ${words} for ${name}`}
                                    onClick={() =>
                                      act("Saving", "PM couldn't change that.", () =>
                                        modPermissionDecide(mod.id, p.id, true),
                                      )
                                    }
                                  >
                                    Allow now
                                  </Button>
                                )}
                                <Button
                                  variant="secondary"
                                  size="sm"
                                  disabled={working !== null}
                                  aria-label={`${on ? "Turn off" : "Allow"}: ${words} for ${name}`}
                                  onClick={() =>
                                    act("Saving", "PM couldn't change that.", () =>
                                      modPermissionDecide(mod.id, p.id, !on),
                                    )
                                  }
                                >
                                  {on ? "Turn off" : "Allow"}
                                </Button>
                              </div>
                            )}
                          </div>
                        </li>
                      );
                    })}
                  </ul>
                )}
                {mod.internetHosts.length > 0 && (
                  <p className="break-words pt-2 text-xs text-ink3">
                    Sites it may reach: {mod.internetHosts.map(modText).join(", ")}
                  </p>
                )}
              </Collapsible>
            </div>

            {canDecide && mod.permissions.some((p) => p.id === "ai") && (
              // Keyed by the stored limit, so the field starts again from it after every save.
              <ModAiLimit
                key={mod.aiCapUsd ?? "none"}
                mod={mod}
                disabled={working !== null}
                onSave={(cap) =>
                  act("Saving", "PM couldn't save that limit.", () => modAiCapSet(mod.id, cap))
                }
              />
            )}

            {canDecide && mod.secrets.length > 0 && (
              // The keys the manifest declares. The same field the Test keys section and a mod's
              // `secrets.set` dialog use; the rows are read again after a save or a remove.
              <ModSecretFields
                modId={mod.id}
                slots={mod.secrets}
                disabled={working !== null}
                onChanged={() => void refresh()}
              />
            )}

            {fix !== "uninstall" && (
              <div>
                <Button
                  variant="danger"
                  disabled={working !== null}
                  aria-label={`Uninstall ${name}`}
                  onClick={() => onUninstall(mod.id)}
                >
                  Uninstall…
                </Button>
              </div>
            )}
          </div>
        </Collapsible>
      </Card>
    </li>
  );
}
