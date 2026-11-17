// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Map's layout Worker: all it does is answer `handle` (../workerProtocol.ts) for each message. The
// page builds it from this file's text with spawnWorker (the frame cannot fetch a worker's file); the
// type-check pass beside this folder gives it the WebWorker lib and no DOM, so a stray `window` here
// is a compile error and not a ReferenceError in the frame. DedicatedWorkerGlobalScope is declared only
// by that lib, so the cast below also fails under the DOM config: the proof the right lib is in force.
import { handle, transferOf } from "../workerProtocol";

const scope = self as unknown as DedicatedWorkerGlobalScope;

scope.onmessage = (event: MessageEvent<unknown>) => {
  const reply = handle(event.data);
  scope.postMessage(reply, transferOf(reply));
};
