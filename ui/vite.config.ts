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
  // worker-safe variants.
  //
  // SCOPE: Vite has no worker-only resolve.conditions (only worker.format /
  // .plugins / .rollupOptions), so this applies to the MAIN/client bundle too.
  // That is intentional and safe: for every affected dep the `worker` variant
  // serializes byte-identically to the `browser` variant — proven for the whole
  // pipeline by the DOM↔worker parity guard (packages/markdown
  // dom-worker-parity.test.ts), which fails if any dep's `worker` export ever
  // diverges in output. The only non-pipeline app dep with a `worker` condition
  // is react-dom, and only on its `./server`/`./static` subpaths (unused by the
  // client) where `worker` maps to the same file as `browser`.
  resolve: { conditions: [...WORKER_BUNDLE_RESOLVE_CONDITIONS, ...defaultClientConditions] },
  server: {
    port: 1420,
    strictPort: true,
  },
});
