// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Map's page: it loads, draws and reacts. The engine (settings, documents, the two layouts, the
// change notice) and the hands (stage.ts) are other files; this joins them to the furniture (shell.ts)
// and decides what shows when.
//
// THE ORDER OF A START: the theme (so the first paint is PM's), the settings, then the documents and
// the layout. A tab switch rebuilds the frame, so every open is a start: a saved layout opens at once
// (three calls), and only the first open after a change shows the "working it out" strip.
//
// TWO TOKENS KEEP AN OLD ANSWER FROM LANDING ON A NEW QUESTION. `loadRun` numbers a (re)load of the
// documents, `arrangeRun` numbers a layout request; a switch of arrangement, a refresh or a changed cap
// while the earlier one is still computing makes its answer stale, and a stale answer is dropped.
// A layout already worked out this session is kept (one for each arrangement), so flipping between the
// two arrangements, or changing cohesion (a render-time blend), costs no recompute; a refresh or a new
// cap drops what it makes stale.
//
// WHAT FAILS IS ALWAYS SAID IN THE MAP'S OWN WORDS (copy.ts), never PM's: a failed list or layout is one
// strip with Try again, a failed save or a failed open is one with Dismiss, and the map the person had
// stays where it can.

import { watchDocumentChanges, type ChangeNotice } from "./changes";
import type { Compute } from "./compute";
import {
  ARRANGE_FAILED,
  LOAD_FAILED,
  nearestCohesion,
  noticeText,
  OPEN_FAILED,
  phaseText,
  SETTINGS_READ_FAILED,
  SETTINGS_SAVE_FAILED,
  subtitle,
} from "./copy";
import { listDocuments, type MapHost } from "./hostApi";
import { buildSemanticLayout, type BaseLayout } from "./layoutModel";
import { graphColor } from "./palette";
import { loadProjectLayout } from "./projectLayout";
import { loadSemanticCoords, type SemanticResult } from "./semantic";
import {
  DEFAULT_SETTINGS,
  loadSettings,
  NODE_CAP_MAX,
  NODE_CAP_MIN,
  parseSettings,
  saveSettings,
  type LayoutMode,
  type MapSettings,
} from "./settings";
import { buildShell, fillCard, fillLegend, setStageMessage, type StageMessage } from "./shell";
import { createStage } from "./stage";
import { applyTheme, canvasColors, DEFAULT_THEME, readTheme, type MapTheme } from "./theme";
import type { MapDoc } from "./types";

export interface MapAppOptions {
  doc: Document;
  win: Window & typeof globalThis;
  /** Where the page is built (the document's body, in the frame). */
  mount: HTMLElement;
  host: MapHost;
  compute: Compute;
  /** The page's root element, which PM's theme variables are set on (the `<html>` element). */
  themeRoot: HTMLElement;
}

export interface MapApp {
  /** Settles when the first load has finished, however it ended. */
  ready: Promise<void>;
  /** Load the documents and the layout again (the Refresh button). */
  reload(): Promise<void>;
  /** Stop everything: listeners, the layout worker, the change watcher. */
  destroy(): void;
}

/** Where a strip's text came from: `data` ends with the next load or layout, `note` stays until it is
 *  dismissed or another note replaces it. */
type Problem = { source: "data" | "note"; text: string; retry: (() => void) | null };

export function mountMap(options: MapAppOptions): MapApp {
  const { doc, win, host, compute } = options;
  const shell = buildShell(doc);
  options.mount.append(shell.root);

  let dead = false;
  let theme: MapTheme = DEFAULT_THEME;
  let applied = new Set<string>();
  let settings: MapSettings = { ...DEFAULT_SETTINGS };
  let docs: MapDoc[] = [];
  let docsReady = false;
  let loadFailed = false;
  // What has been worked out this session, one per arrangement.
  let semantic: SemanticResult | null = null;
  let project: BaseLayout | null = null;
  let projectDone = false;
  let base: BaseLayout | null = null;
  let arranging = false;
  let phase: string | null = null;
  let notice: ChangeNotice | null = null;
  let problem: Problem | null = null;
  let hovered: MapDoc | null = null;
  let projectIndex = new Map<string, number>();
  let loadRun = 0;
  let arrangeRun = 0;
  let saving: Promise<void> = Promise.resolve();

  // ---- drawing the page from its state ---------------------------------------------------------------

  const colorFor = (name: string) =>
    graphColor(projectIndex.get(name) ?? 0, theme.mode, theme.colorblind);

  const stage = createStage({
    wrap: shell.stage,
    canvas: shell.canvas,
    win,
    onHover(next) {
      hovered = next;
      fillCard(shell, doc, next, theme.depth === "power");
    },
    onOpen(next) {
      // PM opens its own reader over the map; the answer is nothing, and a refusal (the document went
      // while the map was open) is the only thing to say.
      host.call("ui.openDocument", { id: next.id }).catch(() => {
        if (!dead) setProblem({ source: "note", text: OPEN_FAILED, retry: null });
      });
    },
  });

  const pushLook = () => {
    stage.setLook({ colors: canvasColors(theme), colorFor, showLabels: settings.labels });
    fillLegend(
      shell,
      doc,
      (base?.projectNames ?? []).map((name) => ({ name, color: colorFor(name) })),
    );
  };

  const stageMessage = (): StageMessage => {
    if (loadFailed) return "failed";
    if (!docsReady) return "loading";
    if (docs.length === 0) return "empty";
    if (base) return "none";
    if (arranging) return "preparing";
    // No layout and nothing running: by meaning with nothing embedded yet, or a layout that failed.
    return settings.layoutMode === "semantic" && semantic !== null ? "nothing-to-place" : "failed";
  };

  const renderStrips = () => {
    shell.computing.hidden = phase === null;
    shell.computingText.textContent = phase ?? "";
    shell.notice.hidden = notice === null;
    shell.noticeText.textContent = notice ? noticeText(notice) : "";
    shell.error.hidden = problem === null;
    shell.errorText.textContent = problem?.text ?? "";
    shell.retry.textContent = problem?.retry ? "Try again" : "Dismiss";
  };

  const renderControls = () => {
    const semanticMode = settings.layoutMode === "semantic";
    shell.byProject.setAttribute("aria-pressed", String(!semanticMode));
    shell.semantic.setAttribute("aria-pressed", String(semanticMode));
    shell.byProject.classList.toggle("active", !semanticMode);
    shell.semantic.classList.toggle("active", semanticMode);
    shell.cohesionLabel.hidden = !semanticMode;
    shell.cohesion.value = String(nearestCohesion(settings.cohesion));
    shell.names.setAttribute("aria-pressed", String(settings.labels));
    shell.names.classList.toggle("active", settings.labels);
    shell.names.title = settings.labels ? "Hide file names" : "Show file names";
    // Not while it is being typed in: the box would jump under the cursor.
    if (doc.activeElement !== shell.nodeCap) shell.nodeCap.value = String(settings.nodeCap);
  };

  const renderSubtitle = () => {
    const text = docsReady
      ? subtitle(
          settings.layoutMode,
          docs.length,
          base?.projectNames.length ?? 0,
          semantic?.placed ?? 0,
        )
      : "";
    shell.subtitle.textContent = text;
    shell.canvas.setAttribute("aria-label", text ? `Map: ${text}` : "Map");
  };

  const render = () => {
    renderControls();
    renderSubtitle();
    renderStrips();
    setStageMessage(shell, stageMessage());
  };

  const setProblem = (next: Problem | null) => {
    problem = next;
    renderStrips();
  };

  const setPhase = (next: string | null) => {
    phase = next;
    renderStrips();
  };

  /** Show a new layout (or none): the stage fits to it and the legend follows its projects. */
  const setBase = (next: BaseLayout | null) => {
    if (next === base) return;
    base = next;
    projectIndex = new Map((base?.projectNames ?? []).map((name, i) => [name, i]));
    stage.setBase(base);
    pushLook();
  };

  // ---- the work --------------------------------------------------------------------------------------

  const arrange = async (): Promise<void> => {
    const run = ++arrangeRun;
    const mode = settings.layoutMode;
    const stale = () => dead || run !== arrangeRun;
    if (problem?.source === "data") problem = null;
    if (docs.length === 0) {
      arranging = false;
      setBase(null);
      setPhase(null);
      render();
      return;
    }
    try {
      if (mode === "project") {
        if (!projectDone) {
          arranging = true;
          setBase(null);
          render();
          const result = await loadProjectLayout(host, compute, docs, (p) => {
            if (!stale()) setPhase(phaseText(p));
          });
          if (stale()) return;
          project = result?.layout ?? null;
          projectDone = true;
        }
        setBase(project);
      } else {
        if (!semantic) {
          arranging = true;
          setBase(null);
          render();
          const result = await loadSemanticCoords(host, compute, {
            nodeCap: settings.nodeCap,
            onPhase: (p) => {
              if (!stale()) setPhase(phaseText(p));
            },
          });
          if (stale()) return;
          semantic = result;
        }
        setBase(
          semantic.coords.length > 0
            ? buildSemanticLayout(docs, semantic.coords, settings.cohesion)
            : null,
        );
      }
    } catch {
      if (stale()) return;
      arranging = false;
      setBase(null);
      setPhase(null);
      // Nothing was kept for the arrangement that failed (a layout is remembered only once it is
      // whole), so trying again simply asks again.
      setProblem({ source: "data", text: ARRANGE_FAILED, retry: () => void arrange() });
      render();
      return;
    }
    arranging = false;
    setPhase(null);
    render();
  };

  const load = async (): Promise<void> => {
    const run = ++loadRun;
    arrangeRun += 1;
    docsReady = false;
    loadFailed = false;
    arranging = false;
    semantic = null;
    project = null;
    projectDone = false;
    setBase(null);
    setPhase(null);
    notice = null;
    if (problem?.source === "data") problem = null;
    render();
    let listed: MapDoc[];
    try {
      listed = await listDocuments(host);
    } catch {
      if (dead || run !== loadRun) return;
      loadFailed = true;
      setProblem({ source: "data", text: LOAD_FAILED, retry: () => void load() });
      render();
      return;
    }
    if (dead || run !== loadRun) return;
    docs = listed;
    docsReady = true;
    await arrange();
  };

  /** Keep a settings change, one save at a time and in the order made (a later write of the same
   *  field must be the one that stays). */
  const persist = (patch: Partial<MapSettings>) => {
    saving = saving
      .then(() => saveSettings(host, patch))
      .then(
        () => undefined,
        () => {
          if (!dead) setProblem({ source: "note", text: SETTINGS_SAVE_FAILED, retry: null });
        },
      );
  };

  const setMode = (mode: LayoutMode) => {
    if (settings.layoutMode === mode) return;
    settings = { ...settings, layoutMode: mode };
    persist({ layoutMode: mode });
    render();
    // Until the documents are in, the load that is running will use the new arrangement.
    if (docsReady) void arrange();
  };

  const setCohesion = (value: number) => {
    const next = parseSettings({ ...settings, cohesion: value }).cohesion;
    if (next === settings.cohesion) return;
    settings = { ...settings, cohesion: next };
    persist({ cohesion: next });
    // A blend at drawing time over the saved coordinates: no recompute, no storage of the layout.
    if (docsReady && semantic && settings.layoutMode === "semantic" && semantic.coords.length > 0) {
      setBase(buildSemanticLayout(docs, semantic.coords, next));
      render();
    } else {
      renderControls();
    }
  };

  const setLabels = (value: boolean) => {
    settings = { ...settings, labels: value };
    persist({ labels: value });
    pushLook();
    renderControls();
  };

  const setNodeCap = (value: number) => {
    // A typed number is clamped to the range. (`parseSettings` reads a negative or fractional STORED
    // value as the default, which is not what a person typing -40 means.)
    const next = Math.min(NODE_CAP_MAX, Math.max(NODE_CAP_MIN, value));
    shell.nodeCap.value = String(next);
    if (next === settings.nodeCap) return;
    settings = { ...settings, nodeCap: next };
    persist({ nodeCap: next });
    // The saved layout was made for the old cap (its fingerprint says so), so the next by-meaning
    // layout is worked out again.
    semantic = null;
    renderControls();
    if (docsReady && settings.layoutMode === "semantic") void arrange();
  };

  const setTheme = (next: MapTheme) => {
    theme = next;
    applied = applyTheme(options.themeRoot, next, applied);
    pushLook();
    // The card shows more at Power depth.
    fillCard(shell, doc, hovered, theme.depth === "power");
  };

  // ---- the controls ----------------------------------------------------------------------------------

  const listeners: [EventTarget, string, EventListener][] = [];
  const on = <T extends Event>(target: EventTarget, type: string, handler: (e: T) => void) => {
    target.addEventListener(type, handler as EventListener);
    listeners.push([target, type, handler as EventListener]);
  };

  on(shell.byProject, "click", () => setMode("project"));
  on(shell.semantic, "click", () => setMode("semantic"));
  on(shell.names, "click", () => setLabels(!settings.labels));
  on(shell.cohesion, "change", () => setCohesion(Number(shell.cohesion.value)));
  on(shell.zoomIn, "click", () => stage.zoom(1.3));
  on(shell.zoomOut, "click", () => stage.zoom(1 / 1.3));
  on(shell.fit, "click", () => stage.fit());
  on(shell.refresh, "click", () => void load());
  on(shell.dismiss, "click", () => {
    notice = null;
    renderStrips();
  });
  on(shell.retry, "click", () => {
    const retry = problem?.retry;
    if (retry) retry();
    else setProblem(null);
  });

  const closePopover = (returnFocus: boolean) => {
    if (shell.settingsPanel.hidden) return;
    shell.settingsPanel.hidden = true;
    shell.settingsButton.setAttribute("aria-expanded", "false");
    if (returnFocus) shell.settingsButton.focus();
  };
  on(shell.settingsButton, "click", () => {
    if (shell.settingsPanel.hidden) {
      shell.settingsPanel.hidden = false;
      shell.settingsButton.setAttribute("aria-expanded", "true");
    } else {
      closePopover(false);
    }
  });
  on(shell.nodeCap, "change", () => {
    const typed = Number(shell.nodeCap.value);
    // A box left empty or filled with something that is not a number is put back, not saved.
    if (shell.nodeCap.value.trim() === "" || !Number.isFinite(typed)) {
      shell.nodeCap.value = String(settings.nodeCap);
      return;
    }
    setNodeCap(Math.round(typed));
  });
  on<KeyboardEvent>(doc, "keydown", (e) => {
    if (e.key === "Escape") closePopover(true);
  });
  on<MouseEvent>(doc, "mousedown", (e) => {
    const target = e.target as Node | null;
    if (
      !shell.settingsPanel.hidden &&
      target &&
      !shell.settingsPanel.contains(target) &&
      !shell.settingsButton.contains(target)
    ) {
      closePopover(false);
    }
  });

  // ---- PM's word ---------------------------------------------------------------------------------------

  const watcher = watchDocumentChanges(host, (next) => {
    notice = next;
    renderStrips();
  });
  const offEvents = host.onEvent((event) => {
    if (event.name !== "theme.changed") return;
    const next = readTheme(event.payload);
    if (next) setTheme(next);
  });

  render();
  const ready = (async () => {
    try {
      const next = readTheme(await host.call("theme.get", {}));
      if (next && !dead) setTheme(next);
    } catch {
      // The stylesheet's own fallbacks and the default colours stand until a `theme.changed` comes.
    }
    if (dead) return;
    try {
      settings = (await loadSettings(host)).settings;
    } catch {
      setProblem({ source: "note", text: SETTINGS_READ_FAILED, retry: null });
    }
    if (dead) return;
    render();
    await load();
  })();

  return {
    ready,
    reload: load,
    destroy() {
      if (dead) return;
      dead = true;
      stage.destroy();
      watcher.stop();
      offEvents();
      compute.close();
      for (const [target, type, handler] of listeners) target.removeEventListener(type, handler);
      shell.root.remove();
    },
  };
}
