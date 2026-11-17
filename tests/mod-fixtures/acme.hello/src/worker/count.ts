// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The fixture's Worker: simulates as many nodes as it is told. Its packages (d3-force, which brings
// d3-dispatch and d3-timer) are bundled by the nested worker build only, which is why the build
// record has to include the worker's modules.
import { forceSimulation } from "d3-force";

// Typed by hand: the page project's DOM lib has no DedicatedWorkerGlobalScope.
const scope = self as unknown as {
  onmessage: ((event: MessageEvent<number>) => void) | null;
  postMessage(message: number): void;
};

scope.onmessage = (event) => {
  const nodes = Array.from({ length: event.data }, () => ({}));
  scope.postMessage(forceSimulation(nodes).stop().nodes().length);
};
