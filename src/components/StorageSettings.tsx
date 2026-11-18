// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Settings → Storage tab: an inventory of the large, regenerable on-device components (the Python
// document engine, the photo-OCR stack, and the active search model)
// with their sizes and a reference-counted teardown. A heavy shared library can only be removed once
// nothing still needs it — its Remove button is greyed with a pill that jumps to the dependent to
// remove first; the cascade is also enforced in the backend. numpy is never offered (shared with the
// search model). Removing the big libraries asks for an extra confirm that recommends keeping them.
// The photo-OCR stack is also installed from here (its row offers Install when absent, and Install
// beside Remove when an update left it on disk but not working), so the whole feature is
// enabled/disabled in one place.
//
// Two folds (Mods spec §1.14), each remembered on this device (lib/foldPrefs): "Built in" is all of
// the above, and "Mods" is what each installed mod keeps (StorageMods), uninstalled through the mod
// runtime rather than the component teardown. The Local AI switch is NOT mirrored here ("one control,
// one home"): Built in points at the Local AI tab, where the switch and the local models live.

import { useCallback, useEffect, useState } from "react";
import {
  installOptionalOcr,
  listStorageComponents,
  onOcrInstall,
  removeStorageComponent,
} from "../lib/ipc";
import type { StorageComponent, StorageReport } from "../lib/types";
import { useRememberedFold } from "../lib/foldPrefs";
import { formatBytes } from "../lib/format";
import { localAiShown, useLocalAiSwitch } from "../lib/localAiSwitch";
import { modsTotal } from "../lib/modStorage";
import { prefersReducedMotion, scrollBehavior } from "../theme";
import { Button, Callout, Collapsible, ConfirmDialog, SectionInfo, SectionLabel } from "./ui";
import { IngestProgress } from "./IngestProgress";
import { StorageMods } from "./StorageMods";

/** Human-friendly size; estimates are prefixed with "~". An *estimated* zero is unknown, not empty.
 *  The formatting itself is {@link formatBytes} — this is only the copy decision around it. The old
 *  local version floored at MB, so every sub-MB component (pyclipper) rendered a useless "0 MB". */
function formatSize(bytes: number, approximate: boolean): string {
  if (bytes <= 0) return approximate ? "—" : "0 MB";
  const s = formatBytes(bytes);
  return approximate ? `~${s}` : s;
}

/** The OCR image libraries — removed in the cascade after photo text recognition; their own confirm. */
const OCR_LIBS = new Set(["opencv-python", "shapely", "pyclipper"]);

function confirmCopy(c: StorageComponent): { title: string; body: string; danger: boolean } {
  if (OCR_LIBS.has(c.id)) {
    return {
      title: `Remove ${c.label}?`,
      danger: true,
      body: "This library is only used by photo text recognition. We recommend keeping it unless you're short on space — removing it means another download if you reinstall photo text recognition.",
    };
  }
  if (c.id === "ocr") {
    return {
      title: "Remove photo text recognition?",
      danger: true,
      body: "New photos and screenshots will be indexed by their date and location only, with no text. You can reinstall it any time — from here, or the next time you drop a photo.",
    };
  }
  return { title: `Remove ${c.label}?`, danger: true, body: "This component will be removed." };
}

export function StorageSettings({ onNavigate }: { onNavigate: (tab: string) => void }) {
  const [report, setReport] = useState<StorageReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<StorageComponent | null>(null);
  const [removingId, setRemovingId] = useState<string | null>(null);
  // The optional photo-OCR stack installs in place from its inventory row: `installingId` marks the
  // row mid-download and `installFrac` (0..1, from the `ocr://install` progress event) drives its bar.
  const [installingId, setInstallingId] = useState<string | null>(null);
  const [installFrac, setInstallFrac] = useState(0);
  // Both folds are remembered on this device; a section nobody folded starts open.
  const [builtInOpen, setBuiltInOpen] = useRememberedFold("settings.storage.builtIn", true);
  const [modsOpen, setModsOpen] = useRememberedFold("settings.storage.mods", true);
  const localAi = useLocalAiSwitch();

  const load = useCallback(() => {
    listStorageComponents()
      .then(setReport)
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  useEffect(() => {
    let cancelled = false;
    const un = onOcrInstall((e) => {
      if (!cancelled) setInstallFrac(e.fraction);
    });
    return () => {
      cancelled = true;
      void un.then((fn) => fn());
    };
  }, []);

  function scrollTo(anchor: string) {
    const el = document.getElementById(`storage-${anchor}`);
    el?.scrollIntoView({ behavior: scrollBehavior(), block: "center" });
    // A Web Animations flash, so `index.css`'s `animation-duration: 0.001ms !important` cannot reach
    // it — that property governs CSS animations only. Gate it in JS or Reduced motion misses it.
    if (!prefersReducedMotion())
      el?.animate?.([{ opacity: 0.4 }, { opacity: 1 }], { duration: 600 });
  }

  function confirmRemove() {
    const c = pending;
    if (!c) return;
    setRemovingId(c.id);
    setError(null);
    removeStorageComponent(c.id)
      .then(() => load())
      .catch((e) => setError(String(e)))
      .finally(() => {
        setRemovingId(null);
        setPending(null);
      });
  }

  // Install an optional component in place (photo OCR is the only installable one). Re-scans on
  // success so the row flips to its installed/removable form (with the image-library children).
  function installComponent(c: StorageComponent) {
    if (c.id !== "ocr") return;
    setInstallingId(c.id);
    setInstallFrac(0);
    setError(null);
    installOptionalOcr()
      .then(() => load())
      .catch((e) => setError(String(e)))
      .finally(() => setInstallingId(null));
  }

  const copy = pending ? confirmCopy(pending) : null;
  // A small control's edge is --ink4 (3:1), not --border2 (~1.6:1): the outline is how it reads as one.
  const pill =
    "rounded-full border border-ink4 px-2 py-0.5 text-xs text-ink3 transition hover:text-ink hover:border-ink2";

  return (
    <div data-help="settings-storage">
      {error && <Callout className="mb-3">{error}</Callout>}

      <Collapsible
        open={builtInOpen}
        onOpenChange={setBuiltInOpen}
        title={<SectionLabel as="span">Built in</SectionLabel>}
        meta={report ? formatSize(report.total_bytes, false) : undefined}
      >
        <div data-help="settings-storage-builtin">
          <div className="mt-3 divide-y divide-border rounded-[var(--radius)] border border-border">
            {report?.components.map((c) => (
              <ComponentRow
                key={c.id}
                c={c}
                busy={removingId === c.id}
                installing={installingId === c.id}
                frac={installFrac}
                onRemove={() => setPending(c)}
                onInstall={() => installComponent(c)}
                onPill={scrollTo}
                onManage={onNavigate}
              />
            ))}
            {!report && <div className="px-4 py-6 text-center text-sm text-ink4">Scanning…</div>}
          </div>

          {/* A pointer, not a mirror of the switch: Local AI's switch and its models live in the
              Local AI tab, and unknown (the switch hasn't answered) says nothing. */}
          {localAi !== null && (
            <div className="mt-3 flex flex-wrap items-center gap-2 text-xs text-ink4">
              <span>
                {localAiShown(localAi)
                  ? "Models you run on this computer with Local AI take their own space and aren't counted here."
                  : "Local AI is off, so PM isn't using a model on this computer."}
              </span>
              <button
                type="button"
                onClick={() => onNavigate(localAiShown(localAi) ? "localai" : "ai")}
                className={pill}
              >
                {localAiShown(localAi) ? "Local AI →" : "AI & Models →"}
              </button>
            </div>
          )}

          <SectionInfo>
            <p>
              What PM has downloaded to this device, and what you can safely remove. The built-in
              components download again when you need them. Mods are different: see Mods below.
            </p>
          </SectionInfo>
        </div>
      </Collapsible>

      <Collapsible
        className="mt-6"
        open={modsOpen}
        onOpenChange={setModsOpen}
        title={<SectionLabel as="span">Mods</SectionLabel>}
        meta={report ? modsTotal(report.mods) : undefined}
      >
        <div className="mt-3">
          {report ? (
            <StorageMods mods={report.mods} reload={load} />
          ) : (
            <div className="rounded-[var(--radius)] border border-border px-4 py-6 text-center text-sm text-ink4">
              Scanning…
            </div>
          )}
        </div>
      </Collapsible>

      <ConfirmDialog
        open={pending != null}
        title={copy?.title ?? ""}
        confirmLabel="Remove"
        danger={copy?.danger}
        busy={removingId != null}
        onConfirm={confirmRemove}
        onClose={() => setPending(null)}
      >
        {copy?.body}
      </ConfirmDialog>
    </div>
  );
}

function StatusChip({ status }: { status: StorageComponent["status"] }) {
  // Removable, installable and stale rows carry their own action buttons — no chip needed.
  if (status === "removable" || status === "installable" || status === "stale") return null;
  // `blocked` means installed and held by a dependent, not missing — say "Installed" first, or a
  // shared library reads as something still to be downloaded. The blocker pill names the holder.
  const label =
    status === "required" ? "Required" : status === "in_use" ? "In use" : "Installed — in use";
  return (
    <span className="rounded-[var(--radius-sm)] bg-bg px-1.5 py-0.5 font-mono text-[0.6875rem] text-ink4">
      {label}
    </span>
  );
}

function ComponentRow({
  c,
  busy,
  installing,
  frac,
  onRemove,
  onInstall,
  onPill,
  onManage,
}: {
  c: StorageComponent;
  busy: boolean;
  installing: boolean;
  frac: number;
  onRemove: () => void;
  onInstall: () => void;
  onPill: (anchor: string) => void;
  onManage: (tab: string) => void;
}) {
  return (
    <div id={`storage-${c.id}`} className={c.child ? "pl-9" : ""}>
      <div className="flex items-start justify-between gap-4 px-4 py-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <span className="truncate text-sm text-ink2">{c.label}</span>
            <span className="shrink-0 font-mono text-xs text-ink4">
              {formatSize(c.size_bytes, c.approximate)}
            </span>
            <StatusChip status={c.status} />
          </div>
          <p className="mt-0.5 text-xs text-ink4">{c.detail}</p>
          {c.note && <p className="mt-0.5 text-xs text-ink4">{c.note}</p>}
          {(c.blockers.length > 0 || c.manage) && (
            <div className="mt-1.5 flex flex-wrap gap-1.5">
              {c.blockers.map((b) => (
                <button
                  key={b.anchor}
                  type="button"
                  onClick={() => onPill(b.anchor)}
                  className="rounded-full border border-ink4 px-2 py-0.5 text-xs text-ink3 transition hover:text-ink hover:border-ink2"
                >
                  {b.label} →
                </button>
              ))}
              {c.manage && (
                <button
                  type="button"
                  onClick={() => onManage(c.manage!.tab)}
                  className="rounded-full border border-ink4 px-2 py-0.5 text-xs text-ink3 transition hover:text-ink hover:border-ink2"
                >
                  {c.manage.label} →
                </button>
              )}
            </div>
          )}
        </div>
        <div className="flex shrink-0 gap-2">
          {c.status === "stale" && (
            <Button variant="secondary" onClick={onInstall} disabled={installing || busy}>
              {installing ? "Installing…" : "Install again"}
            </Button>
          )}
          {(c.status === "removable" || c.status === "stale") && (
            <Button variant="secondary" onClick={onRemove} disabled={busy || installing}>
              {busy ? "Removing…" : "Remove"}
            </Button>
          )}
          {c.status === "blocked" && (
            <Button variant="secondary" disabled>
              Remove
            </Button>
          )}
          {c.status === "installable" && (
            <Button variant="secondary" onClick={onInstall} disabled={installing}>
              {installing ? "Installing…" : "Install"}
            </Button>
          )}
        </div>
      </div>
      {installing && (
        <IngestProgress
          mode="percent"
          processed={Math.round(frac * 100)}
          total={100}
          label="Downloading photo text recognition"
          className="px-4 pb-3"
        />
      )}
    </div>
  );
}
