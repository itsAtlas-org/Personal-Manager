// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Developer kit's readouts in Settings → Developer (Mods spec §1.15): the API version with its
// deprecation window and the experimental methods, the endpoints, the limits, and what is running
// now. Every figure and every row is PM's own answer (`devKitInfo`, from the dispatch table and the
// build-time limits in Rust), so this page keeps no copy of any of it that could drift; the only words
// of its own are the labels. Test keys are not here: they need `secrets.set`, which is a later change.
//
// Each is a section of the tab's rail (registry.ts). While developer mode is off the kit can't be
// read (the backend refuses), so each section says that instead of showing nothing, and the rail's
// links still land somewhere.

import type { ReactNode } from "react";
import { useDevKit } from "../../lib/useDevKit";
import type { DevKitInfo, KitEndpoint } from "../../lib/modsTypes";
import { modText } from "../mods/modText";
import { Button, Callout, Collapsible, HScroll, SectionLabel } from "../ui";
import { LIMIT_KEYS, LIMIT_ROWS, OFF_HINT } from "./kitWords";

/** One section of the Developer tab: the rail's anchor, the help id and a heading. */
export function DevSection({
  id,
  helpId,
  title,
  action,
  children,
}: {
  id: string;
  helpId: string;
  title: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div
      id={id}
      data-settings-section
      data-help={helpId}
      className="mt-6 border-t border-border pt-4"
    >
      <SectionLabel action={action}>{title}</SectionLabel>
      <div className="mt-2 space-y-3">{children}</div>
    </div>
  );
}

function sinceWords(e: KitEndpoint): string {
  const parts = [`since ${e.added}`];
  if (e.deprecated) parts.push(`deprecated in ${e.deprecated}`);
  if (e.removed) parts.push(`removed in ${e.removed}`);
  if (e.experimental) parts.push("experimental");
  return parts.join(", ");
}

function EndpointTable({ rows, label }: { rows: readonly KitEndpoint[]; label: string }) {
  return (
    <HScroll label={`${label}, scrollable`}>
      <table aria-label={label} className="w-full text-left text-xs">
        <thead className="font-mono uppercase tracking-wide text-ink4">
          <tr className="border-b border-border">
            <th className="py-1 pr-3 font-medium">Name</th>
            <th className="py-1 pr-3 font-medium">Needs</th>
            <th className="py-1 pr-3 font-medium">Kind</th>
            <th className="py-1 font-medium">Version</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((e) => (
            <tr key={`${e.kind}:${e.name}`} className="border-b border-border align-top">
              <td className="py-1 pr-3 font-mono text-ink2">{e.name}</td>
              <td className="py-1 pr-3 font-mono text-ink3">{e.permission ?? "any session"}</td>
              <td className="py-1 pr-3 text-ink3">{e.class}</td>
              <td className="py-1 text-ink3">{sinceWords(e)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </HScroll>
  );
}

function Running({ kit }: { kit: DevKitInfo }) {
  const { engine, sessions } = kit;
  return (
    <>
      <p className="text-sm text-ink2">
        {engine.sealed
          ? "The web engine is sealed: a mod's frame can't reach the network on its own."
          : "The web engine is not sealed, so a mod nobody has checked can't be allowed to read your data."}
      </p>
      {!engine.sealed && engine.unsealedReasons.length > 0 && (
        <ul className="list-disc pl-5 text-xs text-ink3">
          {engine.unsealedReasons.map((r) => (
            <li key={r} className="break-words">
              {r}
            </li>
          ))}
        </ul>
      )}
      <p className="text-xs text-ink3">
        Network connections the engine refused since PM started:{" "}
        <span className="font-mono">{engine.sinkholeAttempts}</span>
      </p>
      {sessions.length === 0 ? (
        <p className="text-xs text-ink3">No mod frames are open.</p>
      ) : (
        <ul aria-label="Open mod frames" className="space-y-1 text-xs">
          {sessions.map((s, i) => (
            <li key={`${s.modId}:${i}`} className="flex flex-wrap gap-x-3 text-ink2">
              <span className="font-mono">{modText(s.modId)}</span>
              <span className="text-ink3">
                events sent {s.sent}, acknowledged {s.acked}, dropped {s.dropped}
              </span>
              {s.closed && <span className="text-ink4">ended</span>}
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

export function DeveloperKit({ on }: { on: boolean }) {
  const { kit, problem, refresh } = useDevKit(on);

  /** What a section holds: the hint while off, the reason a read failed, or the readout. */
  function body(show: (k: DevKitInfo) => ReactNode): ReactNode {
    if (!on) return <p className="text-sm text-ink3">{OFF_HINT}</p>;
    if (kit) return show(kit);
    if (problem) return <Callout size="md">{problem}</Callout>;
    return <p className="text-sm text-ink3">Reading…</p>;
  }

  return (
    <>
      <DevSection id="sec-dev-api" helpId="settings-developer-api" title="API version">
        {body((k) => (
          <>
            <p className="text-sm text-ink2">
              Mod API <span className="font-mono">{k.api.version}</span>
            </p>
            <p className="text-xs text-ink3">{k.api.window}</p>
            <div className="text-xs text-ink3">
              <p>Experimental, so they may change in any minor version:</p>
              {k.api.experimental.length === 0 ? (
                <p className="text-ink4">None.</p>
              ) : (
                <ul className="mt-1 flex flex-wrap gap-1.5">
                  {k.api.experimental.map((name) => (
                    <li
                      key={name}
                      className="rounded-[var(--radius-sm)] bg-surface px-1.5 py-0.5 font-mono text-ink2"
                    >
                      {name}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </>
        ))}
      </DevSection>

      <DevSection id="sec-dev-endpoints" helpId="settings-developer-endpoints" title="Endpoints">
        {body((k) => {
          const calls = k.endpoints.filter((e) => e.kind === "call");
          const events = k.endpoints.filter((e) => e.kind === "event");
          return (
            <>
              <p className="text-xs text-ink3">
                Straight from PM&apos;s dispatch table: what a mod may ask for, what PM tells it,
                and the permission each needs.
              </p>
              <Collapsible defaultOpen={false} title={`Calls a mod makes (${calls.length})`}>
                <div className="mt-2">
                  <EndpointTable rows={calls} label="Calls a mod makes" />
                </div>
              </Collapsible>
              <Collapsible defaultOpen={false} title={`Events PM sends (${events.length})`}>
                <div className="mt-2">
                  <EndpointTable rows={events} label="Events PM sends" />
                </div>
              </Collapsible>
            </>
          );
        })}
      </DevSection>

      <DevSection id="sec-dev-limits" helpId="settings-developer-limits" title="Limits">
        {body((k) => (
          <dl className="grid grid-cols-[1fr_auto] gap-x-4 gap-y-1 text-xs">
            {LIMIT_KEYS.map((key) => (
              <div key={key} className="contents">
                <dt className="text-ink3">{LIMIT_ROWS[key].label}</dt>
                <dd className="text-right font-mono text-ink2">
                  {LIMIT_ROWS[key].say(k.limits[key])}
                </dd>
              </div>
            ))}
          </dl>
        ))}
      </DevSection>

      <DevSection
        id="sec-dev-running"
        helpId="settings-developer-running"
        title="Running now"
        action={
          on ? (
            <Button variant="tertiary" size="sm" onClick={() => void refresh()}>
              Refresh
            </Button>
          ) : undefined
        }
      >
        {body((k) => (
          <Running kit={k} />
        ))}
        {on && kit && problem && <Callout size="md">{problem}</Callout>}
      </DevSection>
    </>
  );
}
