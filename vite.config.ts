// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],

  // Scan only PM's own page for dependencies to pre-bundle. Vite's default is every *.html under
  // the root, which wanders into mods/*/src, tests/mod-fixtures/*/src and tests/sealed-frame/*,
  // pages their own pipelines build (with `inline-worker:` imports this scan cannot resolve). The
  // scan then failed, pre-bundling ran late, and the forced reload could leave the window white.
  // The calendar pop-out and the briefing window are query strings on this one page.
  optimizeDeps: { entries: ["index.html"] },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
