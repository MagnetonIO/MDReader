/**
 * Export conditions the markdown parse WORKER bundle must resolve WITH, prepended
 * ahead of a bundler's default browser conditions.
 *
 * `parseMarkdown` (the unified pipeline) pulls transitive dependencies that ship a
 * `browser` export variant which references `document` at module-evaluation time:
 *
 *  - `decode-named-character-reference` -> `index.dom.js`
 *    (`const element = document.createElement('i')` at module top level), reached
 *    unconditionally via `remark-parse`/micromark — so it breaks EVERY document,
 *    with or without special content; and
 *  - `hast-util-from-html-isomorphic` -> `browser.js`
 *    (`new DOMParser()` / `document.createElement('template')`), reached via
 *    `rehype-katex` when a document contains math.
 *
 * A dedicated Web Worker has NO `document`, so a worker chunk built with a
 * bundler's default `browser` conditions throws `ReferenceError: document is not
 * defined` ("Can't find variable: document" in WKWebView/JavaScriptCore) the
 * moment it loads — which is why documents at or above {@link WORKER_BYTE_THRESHOLD}
 * (routed off-thread) failed to open while smaller, main-thread parses rendered
 * fine.
 *
 * Every one of those packages ALSO publishes a `worker` export condition that maps
 * to a DOM-free variant, and each lists `worker` ahead of `browser` in its export
 * map, so making the resolver see `worker` as active picks the DOM-free variant
 * (the main-thread variants are functionally identical — output parity holds).
 *
 * Single source of truth: BOTH `apps/mdreader/ui/vite.config.ts` (the shipped
 * worker/client bundle) and the `worker` vitest project (`vitest.config.ts`)
 * build their resolve conditions by calling {@link workerBundleResolveConditions}
 * — neither re-inlines the spread — so the app and the regression guard can never
 * drift. Dropping `'worker'` here reintroduces the bug in both places.
 *
 * Guards: `resolve-conditions.test.ts` pins the helper's output AND loads/evaluates
 * the shipped `vite.config.ts` (via Vite's config loader), asserting its effective
 * `resolve.conditions` deep-equals the helper output — so a hand-edit to the app
 * config that drifts from this helper is caught semantically, not by text match;
 * `worker-dom-safety.test.ts` proves the worker path parses a >64 KiB doc with no
 * `document`; and `dom-worker-parity.test.ts` proves the DOM (`browser`) variants
 * and these `worker` variants serialize identically, entity-decoding and KaTeX
 * paths included.
 */
export const WORKER_BUNDLE_RESOLVE_CONDITIONS: readonly string[] = ['worker'];

/**
 * The exact `resolve.conditions` the markdown worker/client bundle must use:
 * {@link WORKER_BUNDLE_RESOLVE_CONDITIONS} prepended ahead of the bundler's own
 * default client conditions (passed in so this shipped module never imports the
 * bundler). Consumed verbatim by `vite.config.ts` (`defaultClientConditions`
 * from Vite) and the `worker` vitest project, and asserted by
 * `resolve-conditions.test.ts`.
 */
export function workerBundleResolveConditions(defaultClientConditions: readonly string[]): string[] {
  return [...WORKER_BUNDLE_RESOLVE_CONDITIONS, ...defaultClientConditions];
}
