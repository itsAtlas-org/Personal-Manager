// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Connected apps" in Settings > Data & Security (Mods spec 1.16): let an AI app on this computer READ
// part of PM's data through a local connection, off by default and per computer. Rust decides whether
// the section exists at all (`mcpOffered`: a release build shows it only on a tested OS) and owns every
// fact on it (the switch, the apps, the token); this draws what it says.
//
// - The switch is this computer's own (a file Rust keeps, never the vault), so it says so beside it.
// - The warnings under it are visible text and never folded into the explanation (Settings'
//   info-vs-controls doctrine: a loss or exposure warning folded away is met as a consequence).
// - Connecting an app asks for a name and what it may read. Three ticks start on (tasks, projects and
//   milestones, calendar events without attendees); documents search starts OFF with its own plain line,
//   because it sends text from the person's documents. Nothing here offers to let an app change
//   anything: apps read, they cannot add, edit or delete.
// - The token is shown ONCE, inside the config text an app's client pastes. It is held in this
//   component's state only until the person says they have copied it, and is in no list, no state read
//   and no message. A failed copy never prints it anywhere else.
// - An app another computer made in the vault, or whose pairing here was lost, is listed as needing
//   re-pairing on this device; revoking it asks first, because it may also disconnect the app on the
//   other computer.

import { useCallback, useEffect, useState } from "react";

import { mcpConnectApp, mcpRevoke, mcpSetOn, mcpState } from "../../lib/ipc";
import { useMcpOffered } from "../../lib/mcpOffered";
import { failureText, type McpClient, type McpConnected, type McpScope } from "../../lib/modsTypes";
import { formatDate } from "../../lib/format";
import { modText } from "../mods/modText";
import {
  Button,
  Callout,
  ConfirmDialog,
  Field,
  Input,
  SectionInfo,
  SectionLabel,
  SettingRow,
  Toggle,
} from "../ui";
import {
  COPY_FAILED,
  DEFAULT_SCOPES,
  NAME_MAX,
  NOTE_UNTRUSTED,
  SCOPE_CHOICES,
  SCOPE_LABEL,
  WARN_CONFIG,
  WARN_PROVIDER,
} from "./connectedAppsWords";
import { MCP_SECTION_ID } from "./registry";

export function ConnectedApps() {
  const offered = useMcpOffered();
  if (!offered) return null;
  return <Section />;
}

function Section() {
  const [on, setOn] = useState(false);
  const [clients, setClients] = useState<McpClient[]>([]);
  const [listening, setListening] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [name, setName] = useState("");
  const [scopes, setScopes] = useState<ReadonlySet<McpScope>>(new Set(DEFAULT_SCOPES));
  // The one place a token is ever held: until the person has copied it and says so.
  const [created, setCreated] = useState<McpConnected | null>(null);
  const [copy, setCopy] = useState<"idle" | "copied" | "failed">("idle");
  const [confirming, setConfirming] = useState<McpClient | null>(null);

  const take = useCallback((s: Awaited<ReturnType<typeof mcpState>>) => {
    setOn(s.on);
    // Off hides the config block, so a token held for it would be lost unseen and come back when the
    // switch goes on again: shown once means gone the moment it is not on screen.
    if (!s.on) {
      setCreated(null);
      setCopy("idle");
    }
    setClients(s.clients);
    setListening(s.listening);
    setProblem(s.error);
    setLoaded(true);
  }, []);

  useEffect(() => {
    let alive = true;
    mcpState()
      .then((s) => {
        if (alive) take(s);
      })
      .catch((e) => {
        if (alive) setError(failureText(e, "PM couldn't read the connected apps."));
      });
    return () => {
      alive = false;
    };
  }, [take]);

  async function change(next: boolean) {
    setBusy(true);
    setError(null);
    try {
      take(await mcpSetOn(next));
    } catch (e) {
      setError(failureText(e, "PM couldn't change this."));
    } finally {
      setBusy(false);
    }
  }

  async function create() {
    setBusy(true);
    setError(null);
    try {
      const made = await mcpConnectApp(name.trim(), [...scopes]);
      setCreated(made);
      setCopy("idle");
    } catch (e) {
      setError(failureText(e, "PM couldn't connect the app."));
      setBusy(false);
      return;
    }
    // The app exists and its token is on screen: a list that fails to refresh is not a failure to connect.
    try {
      take(await mcpState());
    } catch {
      /* the list is read again the next time Settings opens */
    } finally {
      setBusy(false);
    }
  }

  async function copySnippet() {
    if (!created) return;
    try {
      await navigator.clipboard.writeText(created.snippet);
      setCopy("copied");
    } catch {
      setCopy("failed");
    }
  }

  /** The token is gone from the page here, and the form starts again. */
  function dismiss() {
    setCreated(null);
    setCopy("idle");
    setName("");
    setScopes(new Set(DEFAULT_SCOPES));
  }

  async function revoke(c: McpClient) {
    setBusy(true);
    setError(null);
    try {
      take(await mcpRevoke(c.id));
    } catch (e) {
      setError(failureText(e, "PM couldn't revoke the app."));
    } finally {
      setBusy(false);
      setConfirming(null);
    }
  }

  function toggleScope(id: McpScope, ticked: boolean) {
    setScopes((prev) => {
      const next = new Set(prev);
      if (ticked) next.add(id);
      else next.delete(id);
      return next;
    });
  }

  const trimmed = name.trim();
  const canCreate = trimmed.length > 0 && trimmed.length <= NAME_MAX && scopes.size > 0 && !busy;

  return (
    <div
      id={MCP_SECTION_ID}
      data-settings-section
      className="mt-5 border-t border-border pt-4"
      data-help="settings-connected-apps"
    >
      <SectionLabel>Connected apps</SectionLabel>
      <SettingRow
        label="Let apps on this computer read PM"
        description="Off until you turn it on, and on this computer only. Apps can read, not change: PM gives them no way to add, edit or delete anything."
      >
        {(a11y) => <Toggle {...a11y} checked={on} disabled={busy || !loaded} onChange={change} />}
      </SettingRow>

      <Callout tone="warning" body="ink" live={false} size="md" className="mt-3 space-y-1">
        <p>{WARN_PROVIDER}</p>
        <p>{WARN_CONFIG}</p>
      </Callout>
      <p className="mt-2 text-xs text-ink3">{NOTE_UNTRUSTED}</p>

      {error && (
        <Callout size="md" className="mt-3">
          {error}
        </Callout>
      )}
      {on && !listening && (
        <Callout size="md" className="mt-3">
          {problem ?? "PM isn't accepting connections right now. Try switching it off and on."}
        </Callout>
      )}

      {on && created && (
        <div className="mt-4 space-y-2 rounded-[var(--radius)] border border-border px-4 py-3">
          <p className="text-sm text-ink2">
            Paste this into the app&apos;s MCP settings. It holds the app&apos;s key, and PM
            can&apos;t show it again.
          </p>
          <pre
            aria-label="Configuration to paste into the app"
            className="max-h-56 select-all overflow-auto whitespace-pre-wrap break-all rounded-[var(--radius-sm)] border border-ink4 bg-surface px-3 py-2 font-mono text-xs text-ink2"
          >
            {created.snippet}
          </pre>
          <div className="flex flex-wrap items-center gap-2">
            <Button variant="secondary" onClick={() => void copySnippet()}>
              Copy
            </Button>
            <Button variant="primary" onClick={dismiss}>
              Done, I&apos;ve saved it
            </Button>
            {copy === "copied" && (
              <span role="status" className="text-xs text-ink3">
                Copied.
              </span>
            )}
            {copy === "failed" && (
              <span role="alert" className="text-xs text-st-due">
                {COPY_FAILED}
              </span>
            )}
          </div>
        </div>
      )}

      {on && !created && (
        <form
          className="mt-4 space-y-3"
          aria-label="Connect an app"
          onSubmit={(e) => {
            e.preventDefault();
            if (canCreate) void create();
          }}
        >
          <SectionLabel as="h3">Connect an app</SectionLabel>
          <Field label="App name" description="So you can tell it apart later.">
            {(a11y) => (
              <Input
                {...a11y}
                value={name}
                maxLength={NAME_MAX}
                placeholder="For example: My assistant"
                onChange={(e) => setName(e.target.value)}
                disabled={busy}
              />
            )}
          </Field>
          <fieldset className="space-y-1.5" disabled={busy}>
            <legend className="text-sm text-ink2">What it may read</legend>
            {SCOPE_CHOICES.map((s) => (
              <label key={s.id} className="flex cursor-pointer items-start gap-2 text-sm text-ink2">
                <input
                  type="checkbox"
                  checked={scopes.has(s.id)}
                  onChange={(e) => toggleScope(s.id, e.target.checked)}
                  className="mt-0.5 accent-[var(--accent)]"
                />
                <span>
                  {s.label}
                  <span className="block text-xs text-ink3">{s.detail}</span>
                </span>
              </label>
            ))}
          </fieldset>
          {scopes.size === 0 && (
            <p className="text-xs text-ink3">Choose at least one thing the app may read.</p>
          )}
          <Button type="submit" variant="primary" disabled={!canCreate}>
            {busy ? "Connecting…" : "Connect"}
          </Button>
        </form>
      )}

      {clients.length > 0 && (
        <ul aria-label="Connected apps" className="mt-4 space-y-2">
          {clients.map((c) => (
            <li
              key={c.id}
              className="flex flex-wrap items-start justify-between gap-3 rounded-[var(--radius)] border border-border px-4 py-3"
            >
              <div className="min-w-0">
                <p className="break-words text-sm text-ink2">{modText(c.name)}</p>
                <p className="break-words text-xs text-ink3">
                  {c.scopes.length > 0
                    ? c.scopes.map((s) => SCOPE_LABEL[s] ?? s).join(", ")
                    : "Nothing it may read"}
                </p>
                <p className="text-xs text-ink3">
                  Added {formatDate(c.createdAt)}
                  {" · "}
                  {c.lastUsedAt ? `Last used ${formatDate(c.lastUsedAt)}` : "Not used yet"}
                </p>
                {!c.paired && (
                  <p className="text-xs text-st-due">
                    Needs re-pairing on this device. It was connected on another computer, or its
                    pairing here was lost. Revoke it and connect it again to use it here.
                  </p>
                )}
              </div>
              <Button
                variant="secondary"
                disabled={busy}
                aria-label={`Revoke ${modText(c.name)}`}
                onClick={() => (c.paired ? void revoke(c) : setConfirming(c))}
              >
                Revoke
              </Button>
            </li>
          ))}
        </ul>
      )}
      {loaded && clients.length === 0 && (
        <p className="mt-4 text-xs text-ink3">No apps are connected.</p>
      )}

      <ConfirmDialog
        open={confirming !== null}
        title="Revoke this app?"
        confirmLabel="Revoke"
        danger
        busy={busy}
        initialFocus="cancel"
        onConfirm={() => confirming && void revoke(confirming)}
        onClose={() => setConfirming(null)}
      >
        <p className="text-sm text-ink2">
          This app isn&apos;t connected on this computer. Revoking it removes it from the vault,
          which may also disconnect it on another computer that shares this vault.
        </p>
      </ConfirmDialog>

      <SectionInfo title="How does connecting an app work?">
        <p>
          PM listens on a private connection point that only your account on this computer can
          reach. An app&apos;s MCP client starts PM with a key from the configuration you paste, and
          PM answers only what you ticked for that app. The key is shown once; PM keeps only a
          fingerprint of it, so a lost key means revoking the app and connecting it again.
        </p>
        <p className="mt-2">
          The switch and the pairing belong to this computer. A restored backup or a shared vault
          never turns it on somewhere else.
        </p>
      </SectionInfo>
    </div>
  );
}
