// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Available section of the Mods tab (Mods spec §1.13): the signed catalogue. Each entry says who
// made it, which tier it has (first party, or second party "reviewed by Atlas at commit …"), and
// every permission it asks for in PM's words, in the order the catalogue lists them, before anything
// is downloaded. "Install" downloads the mod (checked against the hash in the safety list) and the
// backend answers with the consent screen (ConsentDialog); nothing is installed until that screen's
// button is pressed.
//
// The page decides nothing: whether an entry can be installed now, and why not, are the backend's
// (`installable`, `whyNot`, `blocked`, `usable`), and what the catalogue says about a mod (its name,
// description, maker) is the mod's own text, drawn as a plain line. The read is lib/useCatalogue;
// the download and everything after it run in lib/modInstall/modActions, so leaving the tab mid-way
// loses nothing.

import { useModActions } from "../../lib/modActions";
import { startCatalogueInstall } from "../../lib/modInstall";
import type { Catalogue, CatalogueEntry } from "../../lib/modsTypes";
import { Button, Callout, Card } from "../ui";
import { ModIconView } from "./ModIconView";
import { madeByLine, modText } from "./modText";
import { classWords, permissionWords } from "./permissionWords";
import { TierBadge } from "./TierBadge";

export function AvailableMods({
  catalogue,
  problem,
  retry,
  dialogFor,
}: {
  catalogue: Catalogue | null;
  /** Why the last read of the catalogue failed. */
  problem: string | null;
  retry: () => void;
  /** The mod whose consent dialog is open: it shows its own refusals. */
  dialogFor: string | null;
}) {
  if (!catalogue) {
    return problem ? (
      <Callout size="md">
        <p>PM couldn&apos;t read the catalogue: {problem}</p>
        <div className="pt-1">
          <Button variant="secondary" size="sm" onClick={retry}>
            Try again
          </Button>
        </div>
      </Callout>
    ) : (
      <p className="text-sm text-ink3">Loading…</p>
    );
  }
  return (
    <div className="space-y-3">
      {catalogue.unavailable && (
        <Callout tone="warning" size="md" live={false}>
          {catalogue.unavailable}
        </Callout>
      )}
      {catalogue.entries.length === 0 ? (
        <p className="text-sm text-ink3">Nothing is on offer yet.</p>
      ) : (
        <ul className="space-y-3">
          {catalogue.entries.map((entry) => (
            <Entry
              key={entry.id}
              entry={entry}
              usable={catalogue.usable}
              quiet={dialogFor === entry.id}
            />
          ))}
        </ul>
      )}
    </div>
  );
}

function Entry({
  entry,
  usable,
  quiet,
}: {
  entry: CatalogueEntry;
  usable: boolean;
  quiet: boolean;
}) {
  const { busy, errors } = useModActions();
  const working = busy.get(entry.id) ?? null;
  const error = quiet ? null : (errors.get(entry.id) ?? null);
  const name = modText(entry.name);
  // The safety list's own words come first; otherwise the reason the backend gives.
  const why = entry.blocked ?? entry.whyNot;
  const canInstall = usable && entry.installable && !entry.installed;

  return (
    <li>
      <Card className="space-y-2 p-4" data-mod-id={entry.id}>
        <div className="flex items-start gap-3">
          <ModIconView modId={entry.id} version={entry.version} name={name} hasIcon={false} />
          <div className="min-w-0 flex-1 space-y-1">
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
              <h3 className="overflow-hidden break-words font-head text-sm font-semibold text-ink">
                {name}
              </h3>
              <span className="text-xs text-ink3">version {modText(entry.version)}</span>
            </div>
            <p className="flex flex-wrap items-center gap-1.5 text-xs text-ink3">
              <TierBadge tier={entry.tier} />
              <span className="break-words">
                {madeByLine(undefined, entry.author)}
                {entry.tier === "second" && entry.commit
                  ? ` · reviewed by Atlas at commit ${modText(entry.commit).slice(0, 12)}`
                  : ""}
              </span>
            </p>
          </div>
          <div className="flex-none">
            {entry.installed ? (
              <span className="text-xs text-ink3">
                Installed
                {entry.installedVersion ? `, version ${modText(entry.installedVersion)}` : ""}
              </span>
            ) : (
              <Button
                variant="primary"
                size="sm"
                disabled={!canInstall || working !== null}
                onClick={() => void startCatalogueInstall(entry.id)}
                aria-label={working ? undefined : `Install ${name}`}
              >
                {working ? `${working}…` : "Install"}
              </Button>
            )}
          </div>
        </div>

        {entry.description && (
          <p className="break-words text-sm text-ink2">{modText(entry.description)}</p>
        )}

        <div className="text-xs text-ink3">
          {entry.permissions.length === 0 ? (
            <p>It doesn&apos;t ask for anything.</p>
          ) : (
            <>
              <p className="font-medium text-ink2">It asks to:</p>
              <ul className="list-disc pl-5">
                {entry.permissions.map((p) => {
                  const cls = classWords(p.class);
                  return (
                    <li key={p.id} className="break-words">
                      {permissionWords(p.id)}
                      {cls ? ` (${cls.charAt(0).toLowerCase()}${cls.slice(1)})` : ""}
                    </li>
                  );
                })}
              </ul>
            </>
          )}
        </div>

        {entry.installed && entry.updateAvailable && (
          <p className="text-xs text-ink3">Version {modText(entry.version)} is available.</p>
        )}
        {!entry.installed && !canInstall && (why || !usable) && (
          <p className="break-words text-xs text-ink3">
            {why ?? "PM can't install from the catalogue until its safety list is refreshed."}
          </p>
        )}
        {error && <Callout size="md">{error}</Callout>}
      </Card>
    </li>
  );
}
