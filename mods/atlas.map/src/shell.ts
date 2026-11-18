// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Map's page furniture: header, strips, stage, card, built once from DOM nodes made one by one.
// Text goes in as `textContent` and nothing is parsed from a string (the mods' lint bans every HTML
// sink, and the frame has no use for one). Looks are classes in style.css: the frame's CSP forbids the
// `style` attribute, so no element here carries one, and a project's colour is an SVG `fill`
// attribute (a presentation attribute, which that policy does not touch) on a legend dot.
//
// This file only builds and fills. What the controls do, and when, is app.ts.

import {
  CANCEL,
  COHESION_CHOICES,
  NO_DOCUMENTS,
  NOTHING_TO_PLACE,
  PREPARING,
  SHARPEN,
  SHARPENING_LABEL,
  TITLE,
} from "./copy";
import { formatDate } from "./format";
import { NODE_CAP_MAX, NODE_CAP_MIN } from "./settings";
import type { MapDoc } from "./types";

const SVG_NS = "http://www.w3.org/2000/svg";

export interface Shell {
  root: HTMLElement;
  subtitle: HTMLElement;
  legend: HTMLElement;
  cohesionLabel: HTMLElement;
  cohesion: HTMLSelectElement;
  names: HTMLButtonElement;
  byProject: HTMLButtonElement;
  semantic: HTMLButtonElement;
  settingsButton: HTMLButtonElement;
  settingsPanel: HTMLElement;
  nodeCap: HTMLInputElement;
  /** The Enhanced layout (t-SNE) switch. */
  enhanced: HTMLInputElement;
  computing: HTMLElement;
  /** The phase as the strip shows it. What a screen reader is told is `announcePhase`. */
  computingText: HTMLElement;
  /** The live regions: always in the page, never inside a strip that starts hidden (a region that appears
   *  together with its text is often not announced). A strip writes its words here as well as in itself. */
  announcePhase: HTMLElement;
  announceHeld: HTMLElement;
  announceNotice: HTMLElement;
  announceError: HTMLElement;
  /** The share of the enhanced layout done: a progressbar outside the live region. */
  progress: HTMLElement;
  progressFill: SVGRectElement;
  cancel: HTMLButtonElement;
  /** The note that stays after the quick layout was kept (Cancel, a deadline, a refused save). */
  held: HTMLElement;
  heldText: HTMLElement;
  sharpen: HTMLButtonElement;
  notice: HTMLElement;
  noticeText: HTMLElement;
  refresh: HTMLButtonElement;
  dismiss: HTMLButtonElement;
  error: HTMLElement;
  errorText: HTMLElement;
  retry: HTMLButtonElement;
  stage: HTMLElement;
  message: HTMLElement;
  messageText: HTMLElement;
  skeleton: HTMLElement;
  canvas: HTMLCanvasElement;
  nav: HTMLElement;
  zoomIn: HTMLButtonElement;
  zoomOut: HTMLButtonElement;
  fit: HTMLButtonElement;
  card: HTMLElement;
}

type Kid = Node | string;

function make<E extends HTMLElement>(element: E, className: string, kids: Kid[]): E {
  if (className) element.className = className;
  element.append(...kids);
  return element;
}

export function buildShell(doc: Document): Shell {
  const div = (className: string, ...kids: Kid[]) =>
    make(doc.createElement("div"), className, kids);
  const span = (className: string, ...kids: Kid[]) =>
    make(doc.createElement("span"), className, kids);
  const p = (className: string, ...kids: Kid[]) => make(doc.createElement("p"), className, kids);
  const button = (className: string, label: string, aria?: string) => {
    const b = make(doc.createElement("button"), className, [label]);
    b.type = "button";
    if (aria) b.setAttribute("aria-label", aria);
    return b;
  };

  const title = make(doc.createElement("h1"), "title", [TITLE]);
  const subtitle = p("subtitle");
  const legend = make(doc.createElement("ul"), "legend", []);
  legend.setAttribute("aria-label", "Projects");
  // The list scrolls when there are many projects; a scroller is only reachable by keyboard when it
  // can take focus (WebKit does not focus one on its own).
  legend.tabIndex = 0;

  const cohesion = doc.createElement("select");
  cohesion.id = "map-cohesion";
  for (const choice of COHESION_CHOICES) {
    const option = doc.createElement("option");
    option.value = String(choice.value);
    option.textContent = choice.label;
    cohesion.append(option);
  }
  const cohesionLabel = make(doc.createElement("label"), "field", [span("", "Cohesion"), cohesion]);
  cohesionLabel.htmlFor = cohesion.id;
  cohesionLabel.hidden = true;

  const names = button("chip", "Names");
  names.title = "Show file names";
  names.setAttribute("aria-pressed", "false");

  const byProject = button("seg-button", "By project");
  const semantic = button("seg-button", "Semantic");
  const segmented = div("seg", byProject, semantic);
  segmented.setAttribute("role", "group");
  segmented.setAttribute("aria-label", "How the map is arranged");

  const settingsButton = button("chip", "Settings");
  settingsButton.setAttribute("aria-haspopup", "dialog");
  settingsButton.setAttribute("aria-expanded", "false");
  settingsButton.setAttribute("aria-controls", "map-settings");

  const nodeCap = doc.createElement("input");
  nodeCap.id = "map-node-cap";
  nodeCap.type = "number";
  nodeCap.min = String(NODE_CAP_MIN);
  nodeCap.max = String(NODE_CAP_MAX);
  nodeCap.step = "100";
  const nodeCapLabel = make(doc.createElement("label"), "field-stack", [
    span("field-title", "Most documents placed by meaning"),
    nodeCap,
  ]);
  nodeCapLabel.htmlFor = nodeCap.id;
  const enhanced = doc.createElement("input");
  enhanced.id = "map-enhanced";
  enhanced.type = "checkbox";
  enhanced.className = "check";
  const enhancedLabel = make(doc.createElement("label"), "check-row", [
    enhanced,
    span("field-title", "Enhanced layout (t-SNE)"),
  ]);
  enhancedLabel.htmlFor = enhanced.id;
  const settingsPanel = div(
    "popover",
    nodeCapLabel,
    p(
      "hint",
      `From ${NODE_CAP_MIN} to ${NODE_CAP_MAX}. Documents past this are gathered at their project's spot. Raise it for a fuller picture, lower it on a slower computer.`,
    ),
    enhancedLabel,
    p(
      "hint",
      "Tighter clusters in the Semantic arrangement. After your documents change it takes a little longer, and a bar shows. Turn it off to keep the quick layout.",
    ),
  );
  settingsPanel.id = "map-settings";
  settingsPanel.setAttribute("role", "dialog");
  settingsPanel.setAttribute("aria-label", "Map settings");
  settingsPanel.hidden = true;

  const header = make(doc.createElement("header"), "bar", [
    div("titles", title, subtitle),
    div(
      "bar-right",
      legend,
      div(
        "controls",
        cohesionLabel,
        names,
        segmented,
        div("popover-anchor", settingsButton, settingsPanel),
      ),
    ),
  ]);

  // Strips under the header. Each is hidden until it has something to say; what it says is also written
  // into a live region that is always in the page (below). The progress bar and Cancel are outside every
  // live region, so a screen reader hears the phase once and not thirty percentages. The bar's fill is an SVG rect (the frame's
  // CSP forbids the style attribute, which a CSS width would need).
  const computingText = span("strip-text");
  const pulse = span("pulse");
  pulse.setAttribute("aria-hidden", "true");
  const progressFill = doc.createElementNS(SVG_NS, "rect");
  progressFill.setAttribute("x", "0");
  progressFill.setAttribute("y", "0");
  progressFill.setAttribute("width", "0");
  progressFill.setAttribute("height", "1");
  const progressSvg = doc.createElementNS(SVG_NS, "svg");
  progressSvg.setAttribute("viewBox", "0 0 100 1");
  progressSvg.setAttribute("preserveAspectRatio", "none");
  progressSvg.setAttribute("aria-hidden", "true");
  progressSvg.append(progressFill);
  const progress = div("progress", progressSvg);
  progress.setAttribute("role", "progressbar");
  progress.setAttribute("aria-label", SHARPENING_LABEL);
  progress.setAttribute("aria-valuemin", "0");
  progress.setAttribute("aria-valuemax", "100");
  progress.setAttribute("aria-valuenow", "0");
  progress.hidden = true;
  const cancel = button("link", CANCEL);
  cancel.hidden = true;
  const computing = div("strip strip-computing", pulse, computingText, progress, cancel);
  computing.hidden = true;

  const heldText = span("strip-text");
  const sharpen = button("link", SHARPEN);
  const held = div("strip strip-held", heldText, sharpen);
  held.hidden = true;

  const noticeText = span("strip-text");
  const refresh = button("link", "Refresh");
  const dismiss = button("link quiet", "Dismiss");
  const notice = div("strip", noticeText, refresh, dismiss);
  notice.hidden = true;

  const errorText = span("strip-text");
  const retry = button("link", "Try again");
  const error = div("strip strip-error", errorText, retry);
  error.hidden = true;

  // The live regions, visually hidden and never `hidden`: the strips above show the words, these say them.
  // Three spans share one polite region and none is atomic, so a change in one is read alone. The error
  // has a region of its own, assertive.
  const announcePhase = span("announce-phase");
  const announceHeld = span("announce-held");
  const announceNotice = span("announce-notice");
  const announcer = div("announcer", announcePhase, announceHeld, announceNotice);
  announcer.setAttribute("role", "status");
  announcer.setAttribute("aria-live", "polite");
  announcer.setAttribute("aria-atomic", "false");
  const announceError = span("announce-error");
  const alerter = div("announcer", announceError);
  alerter.setAttribute("role", "alert");

  // The stage: a message (or the shimmer while documents load) or the canvas with its controls.
  const messageText = span("");
  const skeleton = div(
    "skeleton",
    div("skel-row", div("skel s9"), div("skel s16"), div("skel s11")),
    div("skel-row wide", div("skel s7"), div("skel s14"), div("skel s8")),
  );
  skeleton.setAttribute("aria-hidden", "true");
  const message = div("message", skeleton, messageText);

  const canvas = doc.createElement("canvas");
  canvas.className = "canvas";
  canvas.setAttribute("role", "img");
  canvas.hidden = true;

  const zoomIn = button("nav-button", "+", "Zoom in");
  zoomIn.title = "Zoom in";
  const zoomOut = button("nav-button", "−", "Zoom out");
  zoomOut.title = "Zoom out";
  const fit = button("nav-button", "⤢", "Fit to view");
  fit.title = "Fit to view";
  const nav = div("nav", zoomIn, zoomOut, fit);
  nav.hidden = true;

  const card = div("card");
  card.hidden = true;

  const stage = div("stage", message, canvas, nav, card);

  const root = div("map", header, announcer, alerter, computing, held, notice, error, stage);
  return {
    root,
    subtitle,
    legend,
    cohesionLabel,
    cohesion,
    names,
    byProject,
    semantic,
    settingsButton,
    settingsPanel,
    nodeCap,
    enhanced,
    computing,
    computingText,
    announcePhase,
    announceHeld,
    announceNotice,
    announceError,
    progress,
    progressFill,
    cancel,
    held,
    heldText,
    sharpen,
    notice,
    noticeText,
    refresh,
    dismiss,
    error,
    errorText,
    retry,
    stage,
    message,
    messageText,
    skeleton,
    canvas,
    nav,
    zoomIn,
    zoomOut,
    fit,
    card,
  };
}

/** What the stage shows in place of the canvas. */
export type StageMessage =
  "loading" | "preparing" | "empty" | "nothing-to-place" | "failed" | "none";

/** Show `which` on the stage: a shimmer, a sentence, nothing (a failure says itself in the error strip
 *  above), or, for `none`, the canvas and its controls. */
export function setStageMessage(shell: Shell, which: StageMessage): void {
  const text: Record<StageMessage, string> = {
    loading: "",
    preparing: PREPARING,
    empty: NO_DOCUMENTS,
    "nothing-to-place": NOTHING_TO_PLACE,
    failed: "",
    none: "",
  };
  shell.message.hidden = which === "none";
  shell.skeleton.hidden = which !== "loading" && which !== "preparing";
  shell.messageText.textContent = text[which];
  shell.canvas.hidden = which !== "none";
  shell.nav.hidden = which !== "none";
}

/** The legend: one dot and one name per project, in the order the colours were given out. */
export function fillLegend(
  shell: Shell,
  doc: Document,
  entries: readonly { name: string; color: string }[],
): void {
  const items = entries.map((entry) => {
    const svg = doc.createElementNS(SVG_NS, "svg");
    svg.setAttribute("class", "dot");
    svg.setAttribute("viewBox", "0 0 10 10");
    svg.setAttribute("aria-hidden", "true");
    const circle = doc.createElementNS(SVG_NS, "circle");
    circle.setAttribute("cx", "5");
    circle.setAttribute("cy", "5");
    circle.setAttribute("r", "5");
    circle.setAttribute("fill", entry.color);
    svg.append(circle);
    const li = doc.createElement("li");
    li.append(svg, entry.name);
    return li;
  });
  shell.legend.replaceChildren(...items);
}

/** The hover card for `document`: title, a few facts, its tags. At Power depth the date it was
 *  ingested; never its path (a mod is not given one). */
export function fillCard(shell: Shell, doc: Document, mapDoc: MapDoc | null, power: boolean): void {
  const card = shell.card;
  if (!mapDoc) {
    card.hidden = true;
    card.replaceChildren();
    return;
  }
  const title = doc.createElement("div");
  title.className = "card-title";
  title.textContent = mapDoc.title;
  title.title = mapDoc.title;

  const list = doc.createElement("dl");
  const row = (label: string, value: string, capitalise = false) => {
    const r = doc.createElement("div");
    r.className = "card-row";
    const dt = doc.createElement("dt");
    dt.textContent = label;
    const dd = doc.createElement("dd");
    dd.textContent = value;
    if (capitalise) dd.className = "capitalise";
    r.append(dt, dd);
    return r;
  };
  list.append(
    row("Project", mapDoc.project || "Unsorted"),
    row("Importance", mapDoc.importance ?? "—", true),
    row("Chunks", String(mapDoc.chunkCount)),
    row("Reviewed", mapDoc.reviewed ? "yes" : "awaiting review"),
  );
  if (power) list.append(row("Ingested", formatDate(mapDoc.ingestedAt)));

  const kids: Node[] = [title, list];
  if (mapDoc.tags.length > 0) {
    const tags = doc.createElement("div");
    tags.className = "card-tags";
    for (const tag of mapDoc.tags) {
      const t = doc.createElement("span");
      t.className = "tag";
      t.textContent = tag;
      tags.append(t);
    }
    kids.push(tags);
  }
  card.replaceChildren(...kids);
  card.hidden = false;
}
