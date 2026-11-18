// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Vite's client types, so a mod's page can `import "./style.css"` (the same line src/ has).
/// <reference types="vite/client" />

// A mod's worker, built by mods/vite.config.ts into one string literal (the frame cannot fetch a
// worker's file): `import source from "inline-worker:./worker/layout.worker.ts"`.
declare module "inline-worker:*" {
  const source: string;
  export default source;
}
