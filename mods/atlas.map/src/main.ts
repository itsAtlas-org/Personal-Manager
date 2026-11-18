// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Map's page: the one line that makes the real connection and the real worker, and starts it.
// What starts is boot.ts, which a test drives with stand-ins; this file has no logic to test.
import "./style.css";
import workerSource from "inline-worker:./worker/layout.worker.ts";
import { connectHost } from "../../../src/kit/host";
import { spawnWorker } from "../../../src/kit/spawnWorker";
import { boot } from "./boot";

void boot({
  doc: document,
  win: window,
  connect: connectHost,
  spawn: () => spawnWorker(workerSource),
});
