// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The enhanced layout's speed. `vitest run` never runs this file (it only collects `*.test.ts`), and the
// page never imports it. It exists so the numbers the default cap (1,000 documents) and the 120 second
// deadline were set from can be made again, on the same input, on any machine. Run it in the capped scope:
//
//   node (V8, WebView2's engine):
//     ~/.cache/pm-capped.sh npx vitest bench --run mods/atlas.map/src/tsne.bench.ts
//
//   JavaScriptCore (WKWebView's and WebKitGTK's engine): `tsneBench.ts` has no imports beyond the layout
//   itself, so bundle it with a one-line entry that prints and run the bundle with the engine's own shell:
//     mkdir -p ~/.cache/pm-bench
//     printf 'import { measure } from "./tsneBench";\nfor (const n of [1000, 5000]) print(JSON.stringify(measure(n, 1024, () => Date.now())));\n' > mods/atlas.map/src/tsneBench.jsc.ts
//     npx rolldown mods/atlas.map/src/tsneBench.jsc.ts --format iife --file ~/.cache/pm-bench/tsne.jsc.js
//     ~/.cache/pm-capped.sh /usr/libexec/webkit2gtk-4.1/jsc ~/.cache/pm-bench/tsne.jsc.js
//   (and delete tsneBench.jsc.ts and ~/.cache/pm-bench afterwards: the entry is a throwaway.)

import { test } from "vitest";
import { measure } from "./tsneBench";

// A whole layout is seconds, not microseconds, so each size is one timed run, not a sampled loop.
for (const count of [1000, 5000]) {
  test(`the enhanced layout of ${count} documents of 1,024 dimensions`, async ({ bench }) => {
    await bench(`${count} documents`, () => {
      measure(count, 1024, () => performance.now());
    }).run({ iterations: 1, warmupIterations: 0, time: 0, warmupTime: 0 });
  }, 600_000);
}
