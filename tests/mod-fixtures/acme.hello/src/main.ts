// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The fixture's page: a third-party mod as a developer would write one, small but touching every part
// of the toolchain: a stylesheet, a bundled npm package on the page (d3-quadtree), and a Worker whose
// own packages (d3-force and what it pulls in) only the nested worker build sees. It runs in the real
// frame (the live test of developer mode's file install) and says what happened on the page.
import "./style.css";
import { quadtree } from "d3-quadtree";
import workerSource from "inline-worker:./worker/count.ts";

const out = document.getElementById("out");
if (out) {
  const points: [number, number][] = [
    [0, 0],
    [1, 1],
    [2, 2],
  ];
  const held = quadtree<[number, number]>().addAll(points).size();
  // A plain string literal turned into a blob Worker: the frame cannot fetch a worker file.
  const url = URL.createObjectURL(new Blob([workerSource], { type: "text/javascript" }));
  const worker = new Worker(url);
  worker.onmessage = (event: MessageEvent<number>) => {
    out.textContent = `the page holds ${held} points, the worker simulated ${event.data} nodes`;
    worker.terminate();
    URL.revokeObjectURL(url);
  };
  worker.postMessage(points.length);
}
