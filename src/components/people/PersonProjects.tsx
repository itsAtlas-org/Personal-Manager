// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The projects a person is linked to: chips that open the project, an unlink on each, and a box to
// link another. The box suggests the projects PM already has and also takes a new name: the backend
// resolves what was typed to the project's own spelling (so "atlas" finds "Atlas") and makes the
// project when there is none, so a name nobody has used yet works the same as an old one.

import { useEffect, useId, useRef, useState } from "react";
import { linkPersonProject, listProjects, unlinkPersonProject } from "../../lib/ipc";
import { Button, Input } from "../ui";

interface Props {
  personId: number;
  projects: readonly string[];
  /** One write, with its failure said by the tab; resolves to whether it worked. */
  run: (write: () => Promise<unknown>) => Promise<boolean>;
  /** Open a project; without it the chips are plain text. */
  onOpenProject?: (project: string) => void;
  /** Linking and unlinking are for people who are not in Trash. */
  readOnly?: boolean;
}

export function PersonProjects({ personId, projects, run, onOpenProject, readOnly }: Props) {
  const [known, setKnown] = useState<string[]>([]);
  const [draft, setDraft] = useState("");
  const listId = useId();

  // The suggestions are only a convenience: if the list can't be read the box still takes a name.
  useEffect(() => {
    let live = true;
    listProjects()
      .then((names) => {
        if (live && Array.isArray(names)) setKnown(names);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, []);

  // Unlinking unmounts the button that has focus, which would drop it to the page: once the project
  // is gone, focus moves to the chip that took its place (or the one before, or the box). Only when
  // focus really did fall to the page, so a click elsewhere in the meantime is not undone.
  const at = useRef<HTMLDivElement>(null);
  const refocus = useRef<number | null>(null);
  const projectsKey = projects.join("\n");
  useEffect(() => {
    const i = refocus.current;
    if (i === null) return;
    refocus.current = null;
    const active = document.activeElement;
    if (active && active !== document.body) return;
    const xs = at.current?.querySelectorAll<HTMLElement>("button[data-unlink]");
    const next = xs && xs.length > 0 ? xs[Math.min(i, xs.length - 1)] : null;
    (next ?? at.current?.querySelector<HTMLElement>("input"))?.focus();
  }, [projectsKey]);

  const linked = new Set(projects.map((p) => p.toLowerCase()));
  const suggestions = known.filter((p) => !linked.has(p.toLowerCase()));

  async function link() {
    const name = draft.trim();
    if (!name) return;
    // Already linked (by any casing) is nothing to do, not an error.
    if (linked.has(name.toLowerCase())) {
      setDraft("");
      return;
    }
    // The box keeps what was typed until the write worked, so a refusal costs no retyping.
    if (await run(() => linkPersonProject(personId, name))) setDraft("");
  }

  return (
    <div ref={at} className="flex flex-col gap-2">
      {projects.length > 0 && (
        <ul className="m-0 flex list-none flex-wrap gap-1.5 p-0">
          {projects.map((p, i) => (
            <li
              key={p}
              className="inline-flex items-center gap-1 rounded-[var(--radius-sm)] border border-ink4 bg-surface px-2 py-0.5 text-xs text-ink2"
            >
              {onOpenProject ? (
                <button
                  type="button"
                  className="underline-offset-2 hover:underline"
                  aria-label={`Open project ${p}`}
                  onClick={() => onOpenProject(p)}
                >
                  {p}
                </button>
              ) : (
                <span>{p}</span>
              )}
              {!readOnly && (
                <button
                  type="button"
                  data-unlink
                  className="px-1 text-ink4 hover:text-ink"
                  aria-label={`Unlink ${p}`}
                  onClick={() => {
                    refocus.current = i;
                    void run(() => unlinkPersonProject(personId, p)).then((ok) => {
                      // A refused write changes nothing, so no focus move is owed.
                      if (!ok) refocus.current = null;
                    });
                  }}
                >
                  ×
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
      {!readOnly && (
        <div className="flex items-center gap-2">
          <div className="w-56">
            <Input
              aria-label="Link a project"
              placeholder="Link a project"
              list={listId}
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  void link();
                }
              }}
            />
            <datalist id={listId}>
              {suggestions.map((p) => (
                <option key={p} value={p} />
              ))}
            </datalist>
          </div>
          <Button
            variant="secondary"
            size="xs"
            disabled={draft.trim() === ""}
            onClick={() => void link()}
          >
            Link
          </Button>
        </div>
      )}
    </div>
  );
}
