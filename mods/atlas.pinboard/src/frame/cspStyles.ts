// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Styles a library writes at run time, kept alive under the frame's policy. A mod's frame is served
// with `style-src <the mod's own app folder>`, no 'unsafe-inline', no nonce (src-tauri/src/mods/
// headers.rs), so the engine refuses a <style> element made by script: its text is never applied.
// CodeMirror makes exactly that (style-mod, for a Document root: a <style> in the head whose
// textContent is its base theme, plus every EditorView.theme), so without this the Markdown note
// editor, the one a note the rich editor cannot keep exactly opens in, loses its flex column, its
// wrapping, its cursor and its selection layers. Measured in WebKitGTK (the C1 harness): the <style>
// is in the DOM and not applied.
//
// A constructable stylesheet on `document.adoptedStyleSheets` is not an inline <style>: no element, no
// parse of markup, so `style-src` has nothing to refuse (also measured). So this module mirrors each
// <style> element in the head into one, and keeps it in step: a changed text updates its sheet, a
// removed element drops it. The mod's own CSS is a <link> to a file and is not touched.
//
// ORDER. The cascade puts an adopted sheet after every document sheet, where the library's <style> (which
// style-mod inserts first in the head) sat before them, so a library rule would now win a tie against the
// mod's own rule. Each mirrored sheet is therefore wrapped in a cascade layer (`@layer`): an unlayered rule
// beats a layered one whatever its specificity or position, so the mod's rules win exactly the ties they
// won before, and a layered rule still beats the layers Tailwind writes (it did, as an unlayered rule).
// The one difference: a library rule that was MORE specific than a mod rule it conflicts with used to win
// and now loses. The mod's CSS for CodeMirror (src/style.css, `.pm-note-editor .cm-*`) has no such pair,
// and the C1 harness compares every `.cm-*` element's computed style with and without the policy.
//
// Where `CSSStyleSheet` can't be constructed, or the document has no `adoptedStyleSheets`, this does
// nothing, and the page is as it was.

/** The layer the mirrored rules sit in. */
export const MIRROR_LAYER = "pm-mirrored-style";

/** The sheet's text for a <style>'s text: the rules, in the layer. */
export function layered(text: string): string {
  return `@layer ${MIRROR_LAYER} {\n${text}\n}`;
}

type Win = Window & typeof globalThis;

/** What this document can do: the constructor and `replaceSync`, `adoptedStyleSheets` on the
 *  document, and a MutationObserver. Null where any is missing. */
function support(doc: Document): { win: Win } | null {
  const win = doc.defaultView as Win | null;
  if (!win || typeof win.MutationObserver !== "function") return null;
  const Sheet = win.CSSStyleSheet as typeof CSSStyleSheet | undefined;
  if (typeof Sheet !== "function" || typeof Sheet.prototype?.replaceSync !== "function")
    return null;
  if (!("adoptedStyleSheets" in doc) || doc.adoptedStyleSheets == null) return null;
  return { win };
}

/**
 * Start mirroring `doc`'s head <style> elements into adopted sheets. Call once, before anything that
 * may create a <style> (before an editor mounts). Returns the way to stop (disconnect and drop every
 * sheet it adopted); a no-op where the engine can't do it.
 */
export function startCspStyles(doc: Document = document): () => void {
  const can = support(doc);
  if (!can) return () => undefined;
  const { win } = can;
  const head = doc.head;
  if (!head) return () => undefined;

  /** A mirrored sheet for each <style> element, in the order elements were first seen. */
  const mirrored = new Map<HTMLStyleElement, CSSStyleSheet>();
  /** Sheets of elements that went away, to be taken off the document's list by the next `publish`. */
  const dropped = new Set<CSSStyleSheet>();

  const isStyle = (n: Node): n is HTMLStyleElement => n.nodeName === "STYLE";

  /** The sheet for `el`, made or brought up to date with its text. */
  const sync = (el: HTMLStyleElement): void => {
    let sheet = mirrored.get(el);
    try {
      if (!sheet) {
        const media = el.media;
        sheet = media ? new win.CSSStyleSheet({ media }) : new win.CSSStyleSheet();
        mirrored.set(el, sheet);
      }
      sheet.replaceSync(layered(el.textContent ?? ""));
    } catch {
      // A text the engine can't take leaves the sheet as it was (or none): never throw into the page.
    }
  };

  /** Put the mirrored sheets, in document order of their elements, after whatever else is adopted. */
  const publish = (): void => {
    const ours = new Set<CSSStyleSheet>(mirrored.values());
    const ordered: CSSStyleSheet[] = [];
    for (const el of Array.from(head.querySelectorAll("style"))) {
      const sheet = mirrored.get(el);
      if (sheet) ordered.push(sheet);
    }
    const others = Array.from(doc.adoptedStyleSheets).filter(
      (s) => !ours.has(s) && !dropped.has(s),
    );
    dropped.clear();
    const next = [...others, ...ordered];
    const now = Array.from(doc.adoptedStyleSheets);
    if (next.length === now.length && next.every((s, i) => s === now[i])) return;
    try {
      doc.adoptedStyleSheets = next;
    } catch {
      // not settable here: the page is as it was
    }
  };

  /** The <style> element a mutation is about, if any. */
  const owner = (n: Node | null): HTMLStyleElement | null => {
    for (let cur = n; cur && cur !== head; cur = cur.parentNode) {
      if (isStyle(cur)) return cur;
    }
    return null;
  };

  const handle = (records: MutationRecord[]): void => {
    const touched = new Set<HTMLStyleElement>();
    for (const record of records) {
      for (const node of Array.from(record.removedNodes)) {
        const sheet = isStyle(node) && !head.contains(node) ? mirrored.get(node) : undefined;
        if (sheet) {
          mirrored.delete(node as HTMLStyleElement);
          dropped.add(sheet);
        }
      }
      for (const node of Array.from(record.addedNodes)) {
        if (isStyle(node)) touched.add(node);
      }
      if (record.type === "childList" && isStyle(record.target)) touched.add(record.target);
      if (record.type === "characterData") {
        const el = owner(record.target);
        if (el) touched.add(el);
      }
      if (record.type === "attributes" && isStyle(record.target)) touched.add(record.target);
    }
    for (const el of touched) if (head.contains(el)) sync(el);
    publish();
  };

  const observer = new win.MutationObserver(handle);
  observer.observe(head, {
    childList: true,
    subtree: true,
    characterData: true,
    attributes: true,
    attributeFilter: ["media"],
  });

  for (const el of Array.from(head.querySelectorAll("style"))) sync(el);
  publish();

  return () => {
    observer.disconnect();
    const ours = new Set<CSSStyleSheet>(mirrored.values());
    mirrored.clear();
    try {
      doc.adoptedStyleSheets = Array.from(doc.adoptedStyleSheets).filter((s) => !ours.has(s));
    } catch {
      // as above
    }
  };
}
