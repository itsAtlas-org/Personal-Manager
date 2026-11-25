// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A stand-in for PM that the Pinboard's tests talk to over the real bridge (the kit's `fakePm`: the
// page's own reading rules, the backend's own limits). It keeps what PM keeps for this mod (storage
// keys with their revisions, as `storage.rs` does: a write over a stale revision is MOD-4001 and
// changes nothing) and answers every other call from a table the test fills in. Every call that
// reached PM is recorded, in order. Ships nowhere: only tests import it.

import { connectWith, type ModHost } from "../../../src/kit/host";
import { fakePm, type FakePm, type Handler } from "../../../src/kit/fakePm";
import { connectPm } from "./lib/pm";

/** A refusal PM would answer a call with. */
export interface Refusal {
  refuse: { code: string; message: string; revision?: number };
}

/** PM refuses with `code` and its sentence. */
export const refuse = (code: string, message = "PM refused.", revision?: number): Refusal => ({
  refuse: { code, message, ...(revision === undefined ? {} : { revision }) },
});

/** What a test says a call answers: a value, a refusal, or a function of the call's arguments. */
export type Answer = unknown;
export type Answers = Record<string, Answer | ((args: any) => Answer | Promise<Answer>)>; // eslint-disable-line @typescript-eslint/no-explicit-any

export interface CallRecord {
  method: string;
  args: unknown;
}

export interface MemoryPm {
  host: ModHost;
  pm: FakePm;
  /** Every call that reached PM, in order. */
  calls: CallRecord[];
  /** The storage keys as PM holds them. */
  storage: Map<string, { value: unknown; revision: number }>;
  /** The methods called, once each. */
  methods(): string[];
  /** The calls of one method. */
  of(method: string): CallRecord[];
  /** PM's answer to a method from now on. */
  answer(method: string, answer: Answer | ((args: never) => Answer | Promise<Answer>)): void;
  /** PM tells the frame something. */
  emit: FakePm["emit"];
  /** Wait for the messages in flight to drain. */
  settle(): Promise<void>;
}

/** What PM answers a call with, as the fake takes it. */
type BackendReply = Awaited<ReturnType<Handler>>;

const isRefusal = (v: unknown): v is Refusal =>
  typeof v === "object" && v !== null && "refuse" in v;

/** A fake PM already connected to a host, with the Pinboard's `lib/pm` pointed at it. `seed` fills
 *  storage (key to value, revision 1); `answers` fills the other calls. */
export async function memoryPm(
  options: { seed?: Record<string, unknown>; answers?: Answers } = {},
): Promise<MemoryPm> {
  const storage = new Map<string, { value: unknown; revision: number }>();
  for (const [key, value] of Object.entries(options.seed ?? {})) {
    storage.set(key, { value, revision: 1 });
  }
  const calls: CallRecord[] = [];
  const table: Answers = { ...options.answers };

  const handle = async (method: string, args: unknown): Promise<BackendReply> => {
    calls.push({ method, args });
    let result: unknown;
    if (method === "storage.get" && !(method in table)) {
      const key = (args as { key: string }).key;
      const row = storage.get(key);
      result = { value: row?.value ?? null, revision: row?.revision ?? 0 };
    } else if (method === "storage.set" && !(method in table)) {
      const a = args as { key: string; value: unknown; expectedRevision: number };
      const have = storage.get(a.key)?.revision ?? 0;
      if (a.expectedRevision !== have) {
        result = refuse("MOD-4001", "That value changed since it was read: read it again.", have);
      } else {
        storage.set(a.key, { value: a.value, revision: have + 1 });
        result = { revision: have + 1 };
      }
    } else if (method in table) {
      const answer = table[method];
      result = typeof answer === "function" ? await answer(args) : answer;
    } else {
      result = refuse("MOD-1001", "PM doesn't know that call.");
    }
    if (isRefusal(result)) return { kind: "error", error: result.refuse };
    return { kind: "value", value: result };
  };

  const pm = fakePm(handle);
  const pending = connectWith(pm.env);
  await pm.settle();
  const host = await pending;
  connectPm(host);
  return {
    host,
    pm,
    calls,
    storage,
    methods: () => [...new Set(calls.map((c) => c.method))],
    of: (method) => calls.filter((c) => c.method === method),
    answer: (method, answer) => {
      table[method] = answer as Answer;
    },
    emit: (name, payload) => pm.emit(name, payload),
    settle: () => pm.settle(),
  };
}
