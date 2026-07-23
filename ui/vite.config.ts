import react from '@vitejs/plugin-react';
import { defaultClientConditions, defineConfig } from 'vite';
import { WORKER_BUNDLE_RESOLVE_CONDITIONS } from '@mdreader/markdown/worker-conditions';

// https://vite.dev/config/
// Tauri drives this dev server: it expects a fixed port and relies on Vite not
// clearing the screen so Rust build errors stay visible. Mobile host/HMR wiring
// arrives with the mobile targets in P6.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  // The markdown parse worker (`@mdreader/markdown/worker`) bundles the unified
  // pipeline, whose transitive deps ship `browser` export variants that touch
  // `document` at module load (decode-named-character-reference,
  // hast-util-from-html-isomorphic, …). A dedicated Web Worker has no
  // `document`, so a chunk built with Vite's default `browser` conditions throws
  // `ReferenceError: document is not defined` on load, breaking every document
  // routed off-thread (>= 64 KiB). Prepending the DOM-free `worker` condition
  // (source of truth in @mdreader/markdown) resolves those deps to their
  // worker-safe variants; the main-thread variants stay functionally identical.
  resolve: { conditions: [...WORKER_BUNDLE_RESOLVE_CONDITIONS, ...defaultClientConditions] },
  server: {
    port: 1420,
    strictPort: true,
  },
});
