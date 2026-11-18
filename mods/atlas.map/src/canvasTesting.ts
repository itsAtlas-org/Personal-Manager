// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A canvas 2-D context that writes down what it is asked, for the Map's drawing tests. Every call is
// logged with the context's state at that moment (fill and stroke colour, line width and dash, alpha,
// font), so a test can say "the circle drawn for this document was filled with this colour at this
// alpha" rather than count calls. Only tests import it, so no bundle holds it.

export interface State {
  fillStyle: unknown;
  strokeStyle: unknown;
  lineWidth: unknown;
  globalAlpha: unknown;
  font: unknown;
  textAlign: unknown;
  textBaseline: unknown;
  lineJoin: unknown;
  lineDash: number[];
}

export interface Call {
  op: string;
  args: unknown[];
  state: State;
}

export interface Recorder {
  ctx: CanvasRenderingContext2D;
  /** Every method call, in order. */
  calls: Call[];
  /** The calls of one method. */
  of(op: string): Call[];
  /** Forget what has been drawn so far (the state stays). */
  clear(): void;
}

/** `measure` is the text width the context reports: by default six pixels a character. */
export function recordingContext(
  measure: (text: string) => number = (t) => t.length * 6,
): Recorder {
  const calls: Call[] = [];
  const state: State = {
    fillStyle: "#000000",
    strokeStyle: "#000000",
    lineWidth: 1,
    globalAlpha: 1,
    font: "10px sans-serif",
    textAlign: "start",
    textBaseline: "alphabetic",
    lineJoin: "miter",
    lineDash: [],
  };
  const proxy = new Proxy(
    {},
    {
      get(_target, prop) {
        const name = String(prop);
        if (name === "measureText") return (text: string) => ({ width: measure(text) });
        if (name in state) return (state as unknown as Record<string, unknown>)[name];
        return (...args: unknown[]) => {
          if (name === "setLineDash") state.lineDash = [...(args[0] as number[])];
          calls.push({ op: name, args, state: { ...state, lineDash: [...state.lineDash] } });
        };
      },
      set(_target, prop, value) {
        (state as unknown as Record<string, unknown>)[String(prop)] = value;
        return true;
      },
    },
  );
  return {
    ctx: proxy as unknown as CanvasRenderingContext2D,
    calls,
    of: (op) => calls.filter((c) => c.op === op),
    clear: () => {
      calls.length = 0;
    },
  };
}
