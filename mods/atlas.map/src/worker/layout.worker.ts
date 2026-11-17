// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Map's layout Worker, filled in by the engine slice. It exists now so the WebWorker type-check
// pass (tsconfig.json beside it) has a file to check. DedicatedWorkerGlobalScope is declared only by
// lib WebWorker, so this line fails under the DOM config: it is the proof the right lib is in force.
export type WorkerScope = DedicatedWorkerGlobalScope;
