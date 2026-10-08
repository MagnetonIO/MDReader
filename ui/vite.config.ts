import react from '@vitejs/plugin-react';
import { defaultClientConditions, defineConfig } from 'vite';
import { workerBundleResolveConditions } from '@mdreader/markdown/worker-conditions';

const mobileDevHost = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
// Tauri drives this dev server: it expects a fixed port and relies on Vite not
// clearing the screen so Rust build errors stay visible. Mobile host/HMR wiring
// arrives with the mobile targets in P6.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  // The markdown parse worker (`@mdreader/markdown/worker`) bundles the unified
  // pipeline, whose transitive deps ship `browser` export variants that touch
  // `document` at module load (decode-named-character-reference,
  // hast-util-from-html-isomorphic, …). A dedicated Web Worker has no
  // `document`, so a chunk built with Vite's default `browser` conditions throws
  // `ReferenceError: document is not defined` on load, breaking every document
  // routed off-thread (>= 64 KiB). Prepending the DOM-free `worker` condition
  // resolves those deps to their worker-safe variants. The exact list is the
  // shared @mdreader/markdown helper — the single source of truth the worker
  // vitest project and the resolve-conditions pin test consume too, so this
  // config cannot silently drift from the guards.
  //
  // SCOPE: Vite has no worker-only resolve.conditions (only worker.format /
  // .plugins / .rollupOptions), so this applies to the MAIN/client bundle too.
  // That is intentional and safe: for every affected dep the `worker` variant
  // serializes byte-identically to the `browser` variant — proven for the
  // executed KaTeX and entity-decode paths by the DOM↔worker parity guard
  // (packages/markdown dom-worker-parity.test.ts), which fails if any dep's
  // `worker` export ever diverges in output. The only non-pipeline app dep with a
  // `worker` condition is react-dom, and only on its `./server`/`./static`
  // subpaths (unused by the client) where `worker` maps to the same file as
  // `browser`.
  resolve: {
    conditions: workerBundleResolveConditions(defaultClientConditions),
  },
  server: {
    host: mobileDevHost || false,
    port: 1420,
    strictPort: true,
    hmr: mobileDevHost
      ? {
          protocol: 'ws',
          host: mobileDevHost,
          port: 1421,
        }
      : undefined,
    // DEV ONLY, and e2e only: the same-origin seam the REAL-ENGINE Playwright
    // mode reaches a live `catdb-server` through (`src/e2e/real-engine.ts`).
    //
    // It has to be same-origin. catdb-server installs no CORS layer, and every
    // `/v1` route is a JSON POST — so a cross-origin call needs a preflight and
    // `OPTIONS /v1/…` answers 405. This is a transparent reverse proxy: the page
    // issues a real `fetch` and a real HTTP round trip reaches the real server.
    //
    // Nothing ships with it: `server.*` is ignored by `vite build`, and the only
    // page that ever calls this prefix is `e2e.html`, which is not a build input.
    // When no server is listening the proxy simply fails the request, which is
    // exactly what real-engine mode wants — see `answerFromRealEngine`.
    proxy: {
      '/__catdb': {
        target: `http://127.0.0.1:${process.env.MDREADER_E2E_CATDB_PORT ?? '8788'}`,
        rewrite: (path) => path.replace(/^\/__catdb/, ''),
      },
    },
  },
});
