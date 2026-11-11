// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A mod's sealed frame under the band PM draws for it (Mods spec §1.4).
//
// - The band shows the mod's name as React text and its tier badge. It sits above the frame and
//   outside it, so nothing the mod draws can cover it, and a mod's page can't pass itself off as a PM
//   dialog. It carries no links and no buttons.
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
//   `ui.openDocument`: PM's own document reader, on a document the backend found readable). The
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
//
// Nothing renders this yet; the Mods tab (spec §8, PR-1 part 6) will.

import { useEffect, useLayoutEffect, useRef, useState } from "react";

import { modCall, modEventsAck, modFrameClose, modFrameOpen, modThemeChanged } from "../../lib/ipc";
import { useReader } from "../../lib/reader";
import { useTheme } from "../../theme";
import { Callout } from "../ui/Callout";
import { cn } from "../ui/cn";
import { toneSurface } from "../ui/tone";
import {
  EventPump,
  FloodGate,
  INTERNAL_MESSAGE,
  MAX_IN_FLIGHT,
  PORT_MESSAGE,
  callError,
  callResult,
  errorText,
  isConnectRequest,
  readPortMessage,
  readReply,
  refusal,
  type BackendReply,
  type PageAction,
} from "./modBridge";
import { themeSnapshot, type ModThemeAxes } from "./modTheme";

/** A mod's trust tier (spec §1.1), as the backend names it. */
export type ModTier = "first" | "second" | "third";

const TIER_LABEL: Record<ModTier, string> = {
  first: "First party",
  second: "Second party",
  third: "Third party",
};

export interface ModFrameProps {
  /** The installed mod's id, from PM's own list. */
  modId: string;
  /** The mod's name, shown in the band as text. */
  name: string;
  tier: ModTier;
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
  }
}

/** Send the frame the backend's answer to call `id`, performing it first when it is a page action. */
function answerCall(live: Live, id: number, backend: BackendReply, ctx: PageContext) {
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

function SealedFrame({ modId, name, tier }: ModFrameProps) {
  const [phase, setPhase] = useState<Phase>({ kind: "opening" });
  const [height, setHeight] = useState<number | null>(null);
  const frameRef = useRef<HTMLIFrameElement>(null);
  const liveRef = useRef<Live | null>(null);
  // What a page action reads: the theme on screen when the answer is made, not when the frame's
  // listener was set up, and the app's own document reader.
  const theme = useTheme();
  // The reader is the app's own (the Map opens it the same way), reached through the provider
  // that wraps every view.
  const { openReaderById } = useReader();
  const pageRef = useRef<PageContext>({ theme, openDocument: openReaderById });
  useLayoutEffect(() => {
    pageRef.current = { theme, openDocument: openReaderById };
  });

  // One session per mount. Under StrictMode's mount-unmount-mount the first open may resolve after its
  // cleanup ran; that session is closed the moment it arrives.
  useEffect(() => {
    let cancelled = false;
    // This open's session, once it exists. Events can't arrive before it does (Rust sends none until
    // the frame's page was served, which is after this resolves), and are dropped if they somehow do.
    let mine: Live | null = null;
    modFrameOpen(modId, (message) => mine?.events.push(message)).then(
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
  }, [modId]);

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
          void modCall(live.session, message.method, message.args)
            .then(
              (raw) => answerCall(live, message.id, readReply(raw), pageRef.current),
              // The backend answers every refusal as a reply; a rejection is the IPC itself
              // failing, and its text is PM's, not the mod's to read.
              () => reply(live, callError(message.id, refusal("internal", INTERNAL_MESSAGE))),
            )
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
  }, [open]);

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
      setPhase({ kind: "stopped", reason: "navigated" });
    }
  };

  return (
    <section className="flex h-full min-h-0 flex-col" aria-label={`Mod: ${name}`}>
      <ModBand name={name} tier={tier} />
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
      {phase.kind === "opening" && <p className="px-4 py-3 text-sm text-ink3">Starting the mod…</p>}
      {phase.kind === "stopped" && (
        <Callout tone="warning" live className="m-4">
          {STOPPED[phase.reason]}
        </Callout>
      )}
      {phase.kind === "failed" && (
        <Callout className="m-4">PM couldn't start this mod: {phase.message}</Callout>
      )}
    </section>
  );
}

/** The band above every mod frame: PM's own drawing, never the mod's. */
function ModBand({ name, tier }: { name: string; tier: ModTier }) {
  // A tier PM doesn't know is shown as the least trusted one.
  const known = Object.prototype.hasOwnProperty.call(TIER_LABEL, tier);
  const shown: ModTier = known ? tier : "third";
  return (
    <div className="flex min-w-0 flex-none items-center gap-2 border-b border-rule bg-panel px-3 py-1.5 text-xs">
      <span className="flex-none text-ink3">Mod</span>
      <span className="min-w-0 truncate font-medium text-ink" title={name}>
        {name}
      </span>
      <span
        className={cn(
          "flex-none rounded-full border px-2 py-px",
          shown === "third" ? undefined : "border-border2 text-ink2",
        )}
        style={shown === "third" ? toneSurface("warning") : undefined}
      >
        {TIER_LABEL[shown]}
      </span>
    </div>
  );
}
