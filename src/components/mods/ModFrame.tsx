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
//   authorised, which PM performs here from what only the page holds (`theme.get`: PM's live theme).
//   The action comes from the backend, never from the frame.
// - A height request is a number PM clamps. Nothing a mod sends ever becomes an attribute, a class, a
//   style string or markup.
//
// Nothing renders this yet; the Mods tab (spec §8, PR-1 part 6) will.

import { useEffect, useLayoutEffect, useRef, useState } from "react";

import { modCall, modFrameClose, modFrameOpen } from "../../lib/ipc";
import { useTheme } from "../../theme";
import { Callout } from "../ui/Callout";
import { cn } from "../ui/cn";
import { toneSurface } from "../ui/tone";
import {
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
  /** Every message with an id spends a token; a frame that runs it dry is flooding. */
  flood: FloodGate;
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

function reply(live: Live, message: unknown) {
  if (live.ended || !live.port) return;
  try {
    live.port.postMessage(message);
  } catch {
    // Every answer is plain JSON from the backend or the page, so this can't fail to clone; if it
    // ever did, the frame's call simply goes unanswered.
  }
}

/** What the page answers for an action the backend authorised. */
function performPageAction(action: PageAction, theme: ModThemeAxes): unknown {
  switch (action) {
    case "theme.get":
      return themeSnapshot(document.documentElement, theme);
  }
}

/** Send the frame the backend's answer to call `id`, performing it first when it is a page action. */
function answerCall(live: Live, id: number, backend: BackendReply, theme: ModThemeAxes) {
  switch (backend.kind) {
    case "value":
      reply(live, callResult(id, backend.value));
      return;
    case "error":
      reply(live, callError(id, backend.error));
      return;
    case "page": {
      let result: unknown;
      try {
        result = performPageAction(backend.action, theme);
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
  // The theme a page action reads: the one on screen when the answer is made, not when the frame's
  // listener was set up.
  const theme = useTheme();
  const themeRef = useRef<ModThemeAxes>(theme);
  useLayoutEffect(() => {
    themeRef.current = theme;
  });

  // One session per mount. Under StrictMode's mount-unmount-mount the first open may resolve after its
  // cleanup ran; that session is closed the moment it arrives.
  useEffect(() => {
    let cancelled = false;
    modFrameOpen(modId).then(
      (opened) => {
        if (cancelled) {
          closeQuietly(opened.session);
          return;
        }
        liveRef.current = {
          session: opened.session,
          loads: 0,
          port: null,
          inFlight: 0,
          ended: false,
          flood: new FloodGate(performance.now()),
        };
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
              (raw) => answerCall(live, message.id, readReply(raw), themeRef.current),
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
    };

    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [open]);

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
