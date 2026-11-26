// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The duplicate review (spec 4.2), opened from "Possible duplicates (N)". It follows the Documents
// duplicate check's design (`DuplicatesPanel`): it only reports, and the person is the judge.
//
//   - every pair says WHY it was flagged in a plain sentence, never as a score;
//   - both people are drawn side by side in one grid, every row on both sides, so the labels line up
//     whatever either person holds and an unknown never shifts the other out of step;
//   - three answers, each naming what it does: Merge…, Not the same person (stored, so it is never
//     offered again; "Show them again" brings the hidden ones back) and Decide later (this visit
//     only);
//   - pairs that differ in nothing are grouped as "Nothing differs (N)" and merged in one action
//     after a preview line; "Similar name" pairs never join that group, and the backend judges each
//     pair again before merging it;
//   - a scan that did not look at everything says so and is never called clean.
//
// The snapshot is the view's (its header button shows the count); this panel draws it and asks the
// view to merge, because the view owns the Undo strip.

import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { markPeopleDistinct, restorePeopleDistinct } from "../../lib/ipc";
import { errorWords } from "../../lib/people";
import {
  cardRows,
  groupPairs,
  hiddenLine,
  incompleteLine,
  mergeTheseLabel,
  nothingDiffersLine,
  reasonLabels,
  reasonSentence,
} from "../../lib/peopleMerge";
import type { DupeCard, DupePair, DupesReport } from "../../lib/types";
import { Button, Callout, ConfirmDialog } from "../ui";

interface Props {
  report: DupesReport | null;
  /** Open the merge dialog for two people. */
  onMerge: (a: number, b: number) => void;
  /** Merge these pairs (each judged again by the backend). Rejects with a sentence on failure. */
  onMergeIdentical: (pairs: [number, number][]) => Promise<void>;
  /** A change was made here (a pair hidden or shown again): read the snapshot again. */
  onChanged: () => void;
  onClose: () => void;
}

function PairCard({
  pair,
  cards,
  today,
  busy,
  onMerge,
  onDistinct,
  onLater,
}: {
  pair: DupePair;
  cards: ReadonlyMap<number, DupeCard>;
  today: Date;
  busy: boolean;
  onMerge: () => void;
  onDistinct: () => void;
  onLater: () => void;
}) {
  const a = cards.get(pair.a);
  const b = cards.get(pair.b);
  if (!a || !b) return null;
  const rows = cardRows(a, b, today);
  const values = (list: string[]) =>
    list.length === 0 ? (
      <span className="text-ink4">—</span>
    ) : (
      list.map((v, i) => (
        <span key={i} className="block break-words text-ink2">
          {v}
        </span>
      ))
    );
  return (
    <li>
      <div
        role="group"
        aria-label={`${a.display_name} and ${b.display_name}`}
        className="rounded-lg border border-border p-3"
      >
        <p className="text-xs text-ink3">
          <span className="font-medium text-ink2">{reasonLabels(pair.reasons)}.</span>{" "}
          {reasonSentence(pair.reasons)}
        </p>
        <table className="mt-2 w-full table-fixed text-xs">
          <thead>
            <tr>
              <th scope="col" className="w-24 pb-1 text-left font-normal text-ink4">
                <span className="sr-only">Detail</span>
              </th>
              <th scope="col" className="pb-1 text-left text-sm font-medium text-ink">
                {a.display_name}
              </th>
              <th scope="col" className="pb-1 text-left text-sm font-medium text-ink">
                {b.display_name}
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <Fragment key={r.key}>
                <tr className="align-top">
                  <th scope="row" className="py-0.5 pr-2 text-left font-normal text-ink4">
                    {r.label}
                  </th>
                  <td className="py-0.5 pr-2">{values(r.left)}</td>
                  <td className="py-0.5">{values(r.right)}</td>
                </tr>
              </Fragment>
            ))}
          </tbody>
        </table>
        <div className="mt-3 flex flex-wrap gap-2">
          <Button variant="secondary" size="xs" disabled={busy} onClick={onMerge}>
            Merge…
          </Button>
          <Button variant="tertiary" size="xs" disabled={busy} onClick={onDistinct}>
            Not the same person
          </Button>
          <Button variant="tertiary" size="xs" disabled={busy} onClick={onLater}>
            Decide later
          </Button>
        </div>
      </div>
    </li>
  );
}

export function DuplicatePeoplePanel({
  report,
  onMerge,
  onMergeIdentical,
  onChanged,
  onClose,
}: Props) {
  const [later, setLater] = useState<ReadonlySet<string>>(new Set());
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const today = useMemo(() => new Date(), []);
  const cards = useMemo(
    () => new Map((report?.cards ?? []).map((c) => [c.id, c] as const)),
    [report],
  );
  const { identical, rest } = useMemo(
    () => (report ? groupPairs(report) : { identical: [], rest: [] }),
    [report],
  );
  const keyOf = (p: DupePair) => `${p.a}-${p.b}`;
  const waiting = rest.filter((p) => !later.has(keyOf(p)));

  async function guarded(work: () => Promise<unknown>) {
    setBusy(true);
    setProblem(null);
    try {
      await work();
    } catch (err) {
      setProblem(errorWords(err));
    } finally {
      setBusy(false);
    }
  }

  const notTheSame = (p: DupePair) =>
    guarded(async () => {
      await markPeopleDistinct(p.a, p.b);
      onChanged();
    });

  const showAgain = () =>
    guarded(async () => {
      await restorePeopleDistinct();
      onChanged();
    });

  async function mergeThem() {
    setConfirming(false);
    await guarded(() => onMergeIdentical(identical.map((p): [number, number] => [p.a, p.b])));
  }

  const decideLater = (p: DupePair) => setLater((prev) => new Set([...prev, keyOf(p)]));

  // A card goes when its pair is hidden, put off or merged, taking its focused button with it, and
  // focus would fall to the page. Move it to the next card to look at, or the heading when none is
  // left. Only when focus really was lost: a dialog or another control that holds it is left alone.
  const waitingRef = useRef<HTMLDivElement>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const shown = report ? [...identical, ...waiting].map(keyOf).sort().join(",") : null;
  const lastShown = useRef<string | null>(null);
  useEffect(() => {
    const before = lastShown.current;
    lastShown.current = shown;
    if (before === null || shown === null) return;
    const gone = before.split(",").some((k) => k !== "" && !shown.split(",").includes(k));
    if (!gone) return;
    const active = document.activeElement;
    if (active && active !== document.body) return;
    const next = waitingRef.current?.querySelector<HTMLElement>("button:not([disabled])");
    (next ?? headingRef.current)?.focus();
  }, [shown]);

  const incomplete = report ? incompleteLine(report) : null;
  const pairCards = (pairs: DupePair[]) => (
    <ul className="mt-2 flex flex-col gap-3">
      {pairs.map((p) => (
        <PairCard
          key={keyOf(p)}
          pair={p}
          cards={cards}
          today={today}
          busy={busy}
          onMerge={() => onMerge(p.a, p.b)}
          onDistinct={() => void notTheSame(p)}
          onLater={() => decideLater(p)}
        />
      ))}
    </ul>
  );

  return (
    <section
      aria-label="Possible duplicates"
      data-help="people-duplicates"
      className="min-h-0 flex-1 overflow-y-auto px-6 py-4"
    >
      <div className="flex items-start justify-between gap-3">
        <div>
          <h2
            ref={headingRef}
            tabIndex={-1}
            className="text-sm font-semibold text-ink outline-none"
          >
            Possible duplicates
          </h2>
          <p className="mt-1 text-xs text-ink3">
            PM shows you each pair and why it looks like one person. It never merges on its own.
          </p>
        </div>
        <Button variant="tertiary" size="xs" onClick={onClose}>
          Close
        </Button>
      </div>

      {problem && (
        <Callout tone="danger" variant="strip" size="sm" className="mt-3">
          {problem}
        </Callout>
      )}
      {incomplete && (
        <Callout tone="warning" variant="strip" size="sm" className="mt-3">
          {incomplete}
        </Callout>
      )}

      {report === null ? (
        <p className="mt-4 text-sm text-ink3">Looking for duplicates…</p>
      ) : report.pairs.length === 0 ? (
        <p className="mt-4 text-sm text-ink3">
          {report.skipped.length === 0 && !report.cut_short
            ? `No duplicates found among your ${report.scanned} ${report.scanned === 1 ? "person" : "people"}.`
            : "No duplicates found in what PM compared."}
        </p>
      ) : (
        <>
          {identical.length > 0 && (
            <div className="mt-4">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <h3 className="text-sm font-medium text-ink">
                  Nothing differs ({identical.length})
                </h3>
                <Button
                  variant="primary"
                  size="xs"
                  disabled={busy}
                  onClick={() => setConfirming(true)}
                >
                  {mergeTheseLabel(identical.length)}
                </Button>
              </div>
              <p className="mt-1 text-xs text-ink3">
                {nothingDiffersLine(identical.length, rest.length)}
              </p>
              <details className="mt-2">
                <summary className="cursor-pointer text-xs text-ink3 hover:text-ink">
                  Look at them one by one
                </summary>
                {pairCards(identical)}
              </details>
            </div>
          )}
          {rest.length > 0 && (
            <div className="mt-5" ref={waitingRef}>
              <h3 className="text-sm font-medium text-ink">
                Look at these yourself ({waiting.length})
              </h3>
              {waiting.length === 0 ? (
                <p className="mt-1 text-xs text-ink3">You've decided to look at these later.</p>
              ) : (
                pairCards(waiting)
              )}
            </div>
          )}
        </>
      )}

      {report !== null && report.hidden > 0 && (
        <p className="mt-4 text-xs text-ink3">
          {hiddenLine(report.hidden)}{" "}
          <button
            type="button"
            className="underline underline-offset-2 hover:text-ink"
            disabled={busy}
            onClick={() => void showAgain()}
          >
            Show them again
          </button>
        </p>
      )}

      <ConfirmDialog
        open={confirming}
        title={`Merge these ${identical.length}?`}
        confirmLabel={mergeTheseLabel(identical.length)}
        busy={busy}
        helpId="people-merge"
        onConfirm={() => void mergeThem()}
        onClose={() => setConfirming(false)}
      >
        <p className="mt-2 text-sm text-ink2">
          {nothingDiffersLine(identical.length, rest.length)} You can undo this for 30 seconds
          afterwards.
        </p>
      </ConfirmDialog>
    </section>
  );
}
