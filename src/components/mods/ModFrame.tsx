// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A mod's sealed frame under the band PM draws for it (Mods spec §1.4).
//
// - The band shows the mod's name as React text and its tier badge. It sits above the frame and
//   outside it, so nothing the mod draws can cover it, and a mod's page can't pass itself off as a PM
//   dialog. It carries no links. Its one button, "About this mod", is PM's and appears only when the
//   mod wrote help or a description: it opens ModAbout over the frame (B7), under a copy of this
//   band, and the frame keeps running beneath it.
// - The frame is `<iframe src sandbox="allow-scripts" allow="">` and nothing more: the ESLint fence
//   allows an iframe only here and only in that shape (INVARIANTS.md I-24). Its `src` is the one-time
//   entry URL `mod_frame_open` mints, set once; a new session gets a new element (keyed by session).
// - A second `load` means the frame navigated. PM closes the session and drops the frame: main's
//   frame-src allows every mod's path, and a frame that could load another page could load another
//   mod's code under this one's grants.
// - The load count isn't the wall that keeps another document out of the frame, since a page that
//   navigates away before its own `load` hands "first load" to the next one, and the port to
//   whichever document connects first. What decides which documents can get in at all is in Rust
//   and the engine: main's frame-src names only the mod origins (WebKit enforces it on a frame's own
//   navigation, measured; the sealed-frame job measures data: and blob: on every engine), the
//   navigation guard admits a pmmod page only as a pending entry URL (WebKit), the scheme renders a
//   page only for its one-time token, and the sinkhole (Linux, Windows) or the content rule list
//   (macOS) takes every web URL off the network. So the next document can only be about:blank or
//   the engine's own error page, and neither runs anyone's script.
// - The bridge is modBridge.ts. The mod is the session's: PM calls under the session it opened for
//   this frame, never anything a message names.
// - A frame that floods (one call more than MAX_IN_FLIGHT waiting, or a burst past the flood gate)
//   has its port closed and its session ended, and PM says so where the frame was.
// - The backend answers each call with a value, a refusal (`MOD-####`), or a page action it has
//   authorised, which PM performs here from what only the page holds (`theme.get`: PM's live theme;
//   `ui.openDocument`: PM's own document reader, on a document the backend found readable;
//   `secrets.set`: PM's key field and `notify`: a notice strip, both drawn by whatever provides
//   `ModPageHooksContext`, and an answer of PM's own failure when nothing does). The
//   action and its arguments come from the backend, never from the frame. The vectors come back as
//   bytes, which are transferred to the frame, not copied.
// - Events (spec §1.5): `mod_frame_open` is given a channel, and Rust keeps it with the session and
//   sends only what the mod's grants allow, so this page forwards every event it receives and
//   decides nothing. They wait in an `EventPump` until the frame's port exists, then go over it in
//   order; the frame's `{ ack }` is relayed to Rust, which is what bounds a slow frame. `theme.changed`
//   starts here: PM's theme snapshot is the page's, so when it changes each frame's session is
//   posted it (`modThemeChanged`) and Rust tells the frame, if its mod may hear it.
// - A height request is a number PM clamps. Nothing a mod sends ever becomes an attribute, a class, a
//   style string or markup.
// - FIRST USE (spec §1.5). A call the backend refuses with MOD-2002 and a `permission` is a mod using
//   something it declared and the person hasn't answered. The call is HELD (it still counts as in
//   flight, so a mod can't pile up more than MAX_IN_FLIGHT of them) while PM asks in its own dialog
//   (ModPermissionPrompt); the answer is recorded through `mod_permission_decide`, and after an Allow
//   the call is made once more under the same session. A No tells the mod it isn't granted; "Not now"
//   gives it the backend's MOD-2002 as it was and is remembered for this session, so a mod that calls
//   in a loop gets one question, not one per call. Only the backend's own reply can start a question:
//   a frame's `refuse` message is a refusal the page sends back to it and never asks anything.
//
// - While mounted it keeps the frozen-page watchdog's heartbeat going (`modWatchdog.ts`).
// - ModTab (the mod's tab) mounts it, and only while the mod's row says it can run. The frame's
//   session ends when the frame unmounts (a tab switch, the lock, a pause that changes the row) and
//   when the backend says `session.closed`: the owner is told through `onClosed` and decides what
//   replaces the frame. A store that closes under a mounted frame sends no goodbye, so the owner
//   unmounts it (ModTab does, on the vault fault).

import { useCallback, useContext, useEffect, useLayoutEffect, useRef, useState } from "react";

import {
  modCall,
  modEventsAck,
  modFrameClose,
  modFrameOpen,
  modPermissionDecide,
  modThemeChanged,
} from "../../lib/ipc";
import { MOD_TAB_HELP_ID } from "../../lib/nav";
import { failureText, type ModHelpEntry } from "../../lib/modsTypes";
import { frameMounted } from "../../lib/modWatchdog";
import { useReader } from "../../lib/reader";
import { useTheme } from "../../theme";
import { Button } from "../ui/Button";
import { Callout } from "../ui/Callout";
import { cn } from "../ui/cn";
import { ModAbout } from "./ModAbout";
import {
  ModPageHooksContext,
  readNotice,
  readSecretAsk,
  type ModPageHooks,
  type ModSource,
} from "./modPageHooks";
import { ModPermissionPrompt } from "./ModPermissionPrompt";
import { hasAbout } from "./modText";
import { TierBadge } from "./TierBadge";
import {
  EventPump,
  FloodGate,
  INTERNAL_MESSAGE,
  MAX_IN_FLIGHT,
  MOD_ERRORS,
  PORT_MESSAGE,
  callError,
  callResult,
  errorText,
  isConnectRequest,
  readEvent,
  readPortMessage,
  readReply,
  refusal,
  type BackendReply,
  type PageAction,
} from "./modBridge";
import { themeSnapshot, type ModThemeAxes } from "./modTheme";

/** A mod's trust tier (spec §1.1), as the backend names it. */
export type ModTier = "first" | "second" | "third";

export interface ModFrameProps {
  /** The installed mod's id, from PM's own list. */
  modId: string;
  /** The mod's name, shown in the band as text. */
  name: string;
  tier: ModTier;
  /** The backend ended this frame's session (`session.closed`), with its reason (`paused`,
   *  `uninstalled`, `replaced`, or null when it sent none PM reads). Called once the goodbye has
   *  been forwarded to the frame; the frame stays on screen until the owner replaces it. */
  onClosed?: (reason: string | null) => void;
  /** What the mod's maker wrote about it (the manifest's description and help). When there is
   *  something to show, the band offers "About this mod". */
  about?: { description: string; help: readonly ModHelpEntry[]; credit?: string };
}

export function ModFrame(props: ModFrameProps) {
  // Another mod is another frame: remount, so no session, state or element carries over.
  return <SealedFrame key={props.modId} {...props} />;
}

/** Why PM stopped a frame. */
type StopReason = "navigated" | "flooded";

const STOPPED: Record<StopReason, string> = {
  navigated: "This mod reloaded itself and was stopped.",
  flooded: "This mod sent PM too many requests at once and was stopped.",
};

type Phase =
  | { kind: "opening" }
  | { kind: "open"; session: string; entryUrl: string }
  | { kind: "stopped"; reason: StopReason }
  | { kind: "failed"; message: string };

/** One session's live state. Kept out of React state so every handler sees a change at once. */
interface Live {
  session: string;
  loads: number;
  port: MessagePort | null;
  inFlight: number;
  ended: boolean;
  /** The backend has said this session is over (`mod.paused` / `session.closed`). Unlike `ended` the
   *  port stays open, so the goodbye itself still reaches the frame, but no answer is performed. */
  over: boolean;
  /** Every message with an id (and every ack) spends a token; a frame that runs it dry is flooding. */
  flood: FloodGate;
  /** The backend's events for this session, held until the port exists and sent over it after. */
  events: EventPump;
}

/** A new session's live state. Its events post to its port through [`reply`], which does nothing
 *  once the session has ended. */
function newLive(session: string): Live {
  const live: Live = {
    session,
    loads: 0,
    port: null,
    inFlight: 0,
    ended: false,
    over: false,
    flood: new FloodGate(performance.now()),
    events: new EventPump((event) => reply(live, event)),
  };
  return live;
}

/** A rejection's text, for the "couldn't start" note (PM's own backend message). */
function reasonOf(e: unknown): string {
  return errorText(
    typeof e === "string" ? e : e instanceof Error ? e.message : "PM refused that call.",
  );
}

/** Closing is idempotent and best-effort: a session whose frame is gone has nobody left to call it,
 *  since only PM's page knows it. */
function closeQuietly(session: string) {
  modFrameClose(session).catch(() => {});
}

function endSession(live: Live) {
  if (live.ended) return;
  live.ended = true;
  live.port?.close();
  live.port = null;
  closeQuietly(live.session);
}

/** Post `message` to the frame's port, moving each of `transfer` (the vectors' buffer) rather than
 *  copying it. */
function reply(live: Live, message: unknown, transfer: Transferable[] = []) {
  if (live.ended || !live.port) return;
  try {
    live.port.postMessage(message, transfer);
  } catch {
    // Every answer is plain JSON, or views over a buffer PM owns, from the backend or the page, so
    // this can't fail to clone; if it ever did, the frame's call simply goes unanswered.
  }
}

/** What the page can do for a page action: PM's live theme, and its own reader. */
interface PageContext {
  theme: ModThemeAxes;
  openDocument: (id: number) => void;
  /** The mod this frame is, as PM's own list has it: what a key field or a notice names. */
  source: ModSource;
  /** What draws `secrets.set`'s field and `notify`'s strip (`modPageHooks.ts`). */
  hooks: ModPageHooks;
}

/** What the page answers for an action the backend authorised. The arguments are the backend's (it
 *  checked them), but they cross IPC as JSON, so each action reads its own as untrusted. */
function performPageAction(action: PageAction, args: unknown, ctx: PageContext): unknown {
  switch (action) {
    case "theme.get":
      return themeSnapshot(document.documentElement, ctx.theme);
    case "ui.openDocument": {
      const id = (args as { id?: unknown } | null)?.id;
      if (typeof id !== "number" || !Number.isSafeInteger(id) || id <= 0) {
        throw new Error("ui.openDocument: not a document id");
      }
      ctx.openDocument(id);
      return null;
    }
    case "secrets.set": {
      // PM's own field for one of the mod's declared slots. The mod is told only whether PM held a
      // key when it asked; the key never comes back to a frame.
      const ask = readSecretAsk(args);
      if (!ctx.hooks.askSecret) throw new Error("secrets.set: nothing to show the field with");
      ctx.hooks.askSecret(ctx.source, ask);
      return { set: ask.set };
    }
    case "notify": {
      const notice = readNotice(args);
      if (!ctx.hooks.showNotice) throw new Error("notify: nothing to show the strip with");
      ctx.hooks.showNotice(ctx.source, notice);
      return null;
    }
  }
}

/** Send the frame the backend's answer to call `id`, performing it first when it is a page action. */
function answerCall(live: Live, id: number, backend: BackendReply, ctx: PageContext) {
  // The backend stops a call in flight when its session ends (a pause, an uninstall, the lock, a
  // vault switch, the flood stop), but only up to its own answer: a page action authorised before
  // the end would still open the reader here, for a frame PM has already stopped. Nothing is done
  // for a session that is over.
  if (live.ended || live.over) return;
  switch (backend.kind) {
    case "value":
      reply(live, callResult(id, backend.value));
      return;
    case "error":
      reply(live, callError(id, backend.error));
      return;
    case "binary":
      // The views share one buffer, which moves to the frame: PM keeps no copy of the vectors.
      reply(live, callResult(id, backend.vectors), [backend.buffer]);
      return;
    case "page": {
      let result: unknown;
      try {
        result = performPageAction(backend.action, backend.args, ctx);
      } catch {
        reply(live, callError(id, refusal("internal", INTERNAL_MESSAGE)));
        return;
      }
      reply(live, callResult(id, result));
      return;
    }
  }
}

/** How a first-use question ended: recorded yes, recorded no, left unanswered, or moot because the
 *  session ended while it was open. */
type Outcome = "allowed" | "denied" | "later" | "gone";

/** The permission a MOD-2002 reply asks about, if it is one worth asking about: the backend names a
 *  permission by its id, so anything else (no id, a malformed one) goes to the mod as it is. */
function permissionToAsk(reply: BackendReply): string | null {
  if (reply.kind !== "error" || reply.error.code !== MOD_ERRORS.needsPrompt) return null;
  const permission = reply.error.permission;
  return typeof permission === "string" && /^[a-z][a-z0-9._-]{0,63}$/.test(permission)
    ? permission
    : null;
}

/** One call from the frame, start to answer: the backend's reply, and for a first-use refusal the
 *  question, held until the person answers, then a single retry after a yes. */
async function runCall(
  live: Live,
  message: { id: number; method: string; args: string },
  page: { current: PageContext },
  ask: (permission: string) => Promise<Outcome>,
): Promise<void> {
  const call = async () => readReply(await modCall(live.session, message.method, message.args));
  let backend = await call();
  const permission = permissionToAsk(backend);
  if (permission !== null) {
    const outcome = await ask(permission);
    // The session ended while the question was open: nothing is sent to a frame PM has stopped.
    if (outcome === "gone" || live.ended || live.over) return;
    if (outcome === "allowed") backend = await call();
    else if (outcome === "denied") {
      backend = {
        kind: "error",
        error: refusal("notGranted", "That permission is off for this mod."),
      };
    }
    // "later": the backend's own MOD-2002 stands.
  }
  answerCall(live, message.id, backend, page.current);
}

/** The reason a `session.closed` carries, as a short string; null for anything else. The backend
 *  writes it, but it crossed IPC as JSON, so it is read as untrusted. */
function closeReason(payload: unknown): string | null {
  const reason = (payload as { reason?: unknown } | null)?.reason;
  return typeof reason === "string" && reason.length <= 32 ? reason : null;
}

function SealedFrame({ modId, name, tier, onClosed, about }: ModFrameProps) {
  const [phase, setPhase] = useState<Phase>({ kind: "opening" });
  const [aboutOpen, setAboutOpen] = useState(false);
  const aboutButton = useRef<HTMLButtonElement>(null);
  const canAbout = about !== undefined && hasAbout(about.description, about.help);
  const closeAbout = useCallback(() => setAboutOpen(false), []);
  // Focus goes back to the button that opened the panel once the panel is gone (and only then: not
  // on first render, and not when the mod stops offering it).
  const aboutWasOpen = useRef(false);
  useEffect(() => {
    if (aboutOpen) aboutWasOpen.current = true;
    else if (aboutWasOpen.current) {
      aboutWasOpen.current = false;
      aboutButton.current?.focus();
    }
  }, [aboutOpen]);
  const onClosedRef = useRef(onClosed);
  useLayoutEffect(() => {
    onClosedRef.current = onClosed;
  });
  const [height, setHeight] = useState<number | null>(null);
  const frameRef = useRef<HTMLIFrameElement>(null);
  const liveRef = useRef<Live | null>(null);

  // First-use questions (see the header). `asking` is what is on screen, oldest first; the calls
  // held for each permission wait in `waitingRef` and are all released by its one answer. `snoozed`
  // holds the permissions left unanswered this session, which are not asked about again.
  const [asking, setAsking] = useState<string[]>([]);
  const [deciding, setDeciding] = useState(false);
  const [decideError, setDecideError] = useState<string | null>(null);
  const waitingRef = useRef(new Map<string, Array<(outcome: Outcome) => void>>());
  const snoozedRef = useRef(new Set<string>());

  const settle = useCallback((permission: string, outcome: Outcome) => {
    const waiters = waitingRef.current.get(permission) ?? [];
    waitingRef.current.delete(permission);
    if (outcome === "later") snoozedRef.current.add(permission);
    setAsking((queue) => queue.filter((p) => p !== permission));
    setDeciding(false);
    setDecideError(null);
    for (const release of waiters) release(outcome);
  }, []);

  /** The session is over (or the frame was stopped): every held call is released unanswered and the
   *  question comes down. */
  const dropQuestions = useCallback(() => {
    const held = [...waitingRef.current.values()].flat();
    waitingRef.current.clear();
    setAsking([]);
    setDeciding(false);
    setDecideError(null);
    for (const release of held) release("gone");
  }, []);
  useEffect(() => dropQuestions, [dropQuestions]);

  const ask = useCallback((permission: string): Promise<Outcome> => {
    if (snoozedRef.current.has(permission)) return Promise.resolve("later");
    return new Promise((resolve) => {
      const waiting = waitingRef.current.get(permission);
      if (waiting) {
        waiting.push(resolve);
        return;
      }
      waitingRef.current.set(permission, [resolve]);
      setAsking((queue) => [...queue, permission]);
    });
  }, []);

  async function answerQuestion(allow: boolean, capUsd?: number) {
    const permission = asking[0];
    if (permission === undefined || deciding) return;
    setDeciding(true);
    setDecideError(null);
    try {
      // The money prompt's monthly limit rides with the yes, written in the same transaction.
      if (capUsd === undefined) await modPermissionDecide(modId, permission, allow);
      else await modPermissionDecide(modId, permission, allow, capUsd);
    } catch (e) {
      // Nothing was recorded: the question stays, with the backend's reason, and can be tried again
      // or left ("Not now").
      setDeciding(false);
      setDecideError(failureText(e, "PM couldn't save that."));
      return;
    }
    settle(permission, allow ? "allowed" : "denied");
  }
  // What a page action reads: the theme on screen when the answer is made, not when the frame's
  // listener was set up, and the app's own document reader.
  const theme = useTheme();
  // The reader is the app's own (the Map opens it the same way), reached through the provider
  // that wraps every view.
  const { openReaderById } = useReader();
  const hooks = useContext(ModPageHooksContext);
  const source: ModSource = { modId, name, tier };
  const pageRef = useRef<PageContext>({ theme, openDocument: openReaderById, source, hooks });
  useLayoutEffect(() => {
    pageRef.current = { theme, openDocument: openReaderById, source, hooks };
  });

  // While this frame is mounted the page proves to PM that it is alive (the frozen-page watchdog: a
  // mod that starves this page's thread is paused, and the page reloaded). From the mount, before the
  // session exists, so PM's clock starts with a real heartbeat.
  useEffect(() => frameMounted(), []);

  // One session per mount. Under StrictMode's mount-unmount-mount the first open may resolve after its
  // cleanup ran; that session is closed the moment it arrives.
  useEffect(() => {
    let cancelled = false;
    // This open's session, once it exists. Events can't arrive before it does (Rust sends none until
    // the frame's page was served, which is after this resolves), and are dropped if they somehow do.
    let mine: Live | null = null;
    modFrameOpen(modId, (message) => {
      if (!mine) return;
      // Forwarded first, so the frame hears its goodbye; then nothing more is performed for it.
      mine.events.push(message);
      const event = readEvent(message);
      const name = event?.event;
      if (name === "mod.paused" || name === "session.closed") {
        mine.over = true;
        // A question about a call of a session that is over has nothing left to decide.
        dropQuestions();
      }
      if (name === "session.closed") onClosedRef.current?.(closeReason(event?.payload));
    }).then(
      (opened) => {
        if (cancelled) {
          closeQuietly(opened.session);
          return;
        }
        mine = newLive(opened.session);
        liveRef.current = mine;
        setPhase({ kind: "open", session: opened.session, entryUrl: opened.entryUrl });
      },
      (e: unknown) => {
        if (!cancelled) setPhase({ kind: "failed", message: reasonOf(e) });
      },
    );
    return () => {
      cancelled = true;
      const live = liveRef.current;
      liveRef.current = null;
      if (live) endSession(live);
    };
  }, [modId, dropQuestions]);

  // A layout effect: it runs in the same task as the commit that mounts the frame, so the listener is
  // in place before the frame's first message (a task of its own) can arrive.
  const open = phase.kind === "open";
  useLayoutEffect(() => {
    if (!open) return;

    // The port closes, the session ends (Rust refuses whatever it still had in flight), and the frame
    // is replaced by PM's note.
    const stopFlooding = (live: Live) => {
      endSession(live);
      if (liveRef.current === live) liveRef.current = null;
      dropQuestions();
      setPhase({ kind: "stopped", reason: "flooded" });
    };

    const onPortMessage = (live: Live, data: unknown) => {
      if (live.ended) return;
      const message = readPortMessage(data);
      switch (message.kind) {
        case "resize":
          setHeight(message.height);
          return;
        case "ack":
          // Relayed as it is: Rust clamps it to what it sent. It costs a token like a call, so a
          // frame can't make PM's IPC its own by acknowledging in a loop.
          if (!live.flood.take(performance.now())) {
            stopFlooding(live);
            return;
          }
          modEventsAck(live.session, message.seq).catch(() => {});
          return;
        case "refuse":
          if (!live.flood.take(performance.now())) {
            stopFlooding(live);
            return;
          }
          reply(live, callError(message.id, message.error));
          return;
        case "call": {
          if (live.inFlight >= MAX_IN_FLIGHT || !live.flood.take(performance.now())) {
            stopFlooding(live);
            return;
          }
          live.inFlight += 1;
          void runCall(live, message, pageRef, ask)
            // The backend answers every refusal as a reply; a rejection is the IPC itself
            // failing, and its text is PM's, not the mod's to read.
            .catch(() => reply(live, callError(message.id, refusal("internal", INTERNAL_MESSAGE))))
            .finally(() => {
              live.inFlight -= 1;
            });
          return;
        }
        case "drop":
          return;
      }
    };

    const onMessage = (event: MessageEvent) => {
      const frame = frameRef.current?.contentWindow;
      // Only the frame PM mounted, and only while its origin is opaque, as every sealed frame's is: a
      // frame whose origin isn't "null" has lost its sandbox, and PM doesn't talk to it.
      if (!frame || event.source !== frame || event.origin !== "null") return;
      if (!isConnectRequest(event.data)) return;
      const live = liveRef.current;
      if (!live || live.ended || live.port) return;
      const channel = new MessageChannel();
      live.port = channel.port1;
      channel.port1.onmessage = (e: MessageEvent) => onPortMessage(live, e.data);
      frame.postMessage(PORT_MESSAGE, "*", [channel.port2]);
      // The events that came before the port, in order, then everything after.
      live.events.connect();
    };

    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [open, ask, dropQuestions]);

  // `theme.changed`: whenever PM's theme snapshot differs from the last one this frame was given, post
  // it for this frame's session (Rust decides whether its mod may hear it). The theme provider applies
  // its custom properties to the root in an effect of its own, which runs after this component's, so
  // the snapshot is taken when the root actually changes (a mutation observer) or, for an axis that
  // moved without the root's attributes doing so, a task after the render.
  const session = phase.kind === "open" ? phase.session : null;
  const themeCheck = useRef<() => void>(() => {});
  useEffect(() => {
    if (!session) return;
    const root = document.documentElement;
    // The baseline is what the frame can read for itself (`theme.get`) when the session starts.
    let last = JSON.stringify(themeSnapshot(root, pageRef.current.theme));
    const check = () => {
      const live = liveRef.current;
      if (!live || live.ended || live.session !== session) return;
      const snapshot = themeSnapshot(root, pageRef.current.theme);
      const text = JSON.stringify(snapshot);
      if (text === last) return;
      last = text;
      modThemeChanged(session, snapshot).catch(() => {});
    };
    themeCheck.current = check;
    const observer = new MutationObserver(check);
    observer.observe(root, { attributes: true });
    return () => {
      themeCheck.current = () => {};
      observer.disconnect();
    };
  }, [session]);
  const { mode, system, accent, colorblind, depth } = theme;
  useEffect(() => {
    // A task after this render, so the provider's own effect has applied the root by then.
    const timer = setTimeout(() => themeCheck.current(), 0);
    return () => clearTimeout(timer);
  }, [session, mode, system, accent, colorblind, depth]);

  const onLoad = () => {
    const live = liveRef.current;
    if (!live || live.ended) return;
    live.loads += 1;
    if (live.loads > 1) {
      endSession(live);
      liveRef.current = null;
      dropQuestions();
      setPhase({ kind: "stopped", reason: "navigated" });
    }
  };

  return (
    // `data-help` is on the section because help mode can't see inside the frame: hovering the
    // page lands on the iframe element, and its nearest ancestor with one speaks for it.
    <section
      className="relative flex h-full min-h-0 flex-col"
      aria-label={`Mod: ${name}`}
      data-help={MOD_TAB_HELP_ID}
    >
      <ModBand
        name={name}
        tier={tier}
        onAbout={canAbout ? () => setAboutOpen(true) : undefined}
        aboutRef={aboutButton}
      />
      {phase.kind === "open" && (
        <iframe
          key={phase.session}
          ref={frameRef}
          src={phase.entryUrl}
          sandbox="allow-scripts"
          allow=""
          title={name}
          className={cn("block w-full border-0", height === null ? "min-h-0 flex-1" : "flex-none")}
          style={height === null ? undefined : { height }}
          onLoad={onLoad}
        />
      )}
      {phase.kind === "open" && asking.length > 0 && (
        <ModPermissionPrompt
          key={asking[0]}
          modId={modId}
          name={name}
          tier={tier}
          permission={asking[0]}
          working={deciding}
          error={decideError}
          onAnswer={(allow, capUsd) => void answerQuestion(allow, capUsd)}
          onLater={() => settle(asking[0], "later")}
        />
      )}
      {phase.kind === "opening" && <p className="px-4 py-3 text-sm text-ink3">Starting the mod…</p>}
      {phase.kind === "stopped" && (
        <Callout tone="warning" live className="m-4">
          {STOPPED[phase.reason]}
        </Callout>
      )}
      {phase.kind === "failed" && (
        <Callout className="m-4">PM couldn't start this mod: {phase.message}</Callout>
      )}
      {aboutOpen && canAbout && (
        // Over the whole section, band included, so the iframe can't show through or take a click;
        // the band is drawn again so the panel still sits under PM's own name-and-tier strip.
        <div className="absolute inset-0 z-10 flex min-h-0 flex-col bg-bg">
          <ModBand name={name} tier={tier} />
          <ModAbout
            description={about.description}
            help={about.help}
            credit={about.credit}
            onClose={closeAbout}
          />
        </div>
      )}
    </section>
  );
}

/** The band above every mod frame: PM's own drawing, never the mod's. */
function ModBand({
  name,
  tier,
  onAbout,
  aboutRef,
}: {
  name: string;
  tier: ModTier;
  onAbout?: () => void;
  aboutRef?: React.Ref<HTMLButtonElement>;
}) {
  return (
    <div className="flex min-w-0 flex-none items-center gap-2 border-b border-rule bg-panel px-3 py-1.5 text-xs">
      <span className="flex-none text-ink3">Mod</span>
      <span className="min-w-0 truncate font-medium text-ink" title={name}>
        {name}
      </span>
      <TierBadge tier={tier} />
      {onAbout && (
        <Button ref={aboutRef} variant="secondary" size="sm" className="ml-auto" onClick={onAbout}>
          About this mod
        </Button>
      )}
    </div>
  );
}
