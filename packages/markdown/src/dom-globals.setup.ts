// Test setup for the `dom` parity project: install a REAL DOM (jsdom) onto the
// globals BEFORE any pipeline module is imported, so the `browser` export
// variants of the pipeline's deps run their genuine DOM code paths —
// `decode-named-character-reference/index.dom.js` (`document.createElement('i')`)
// and `hast-util-from-html-isomorphic/browser.js` (`new DOMParser()` /
// `document.createElement('template')`). Kept in a Node environment (not the
// jsdom vitest environment) purely so `import.meta.url`-based corpus loading
// keeps working; the DOM globals below are what actually matter.
import { createRequire } from 'node:module';

interface JsdomWindow {
  [key: string]: unknown;
}
type JsdomCtor = new (html?: string, options?: { url?: string }) => { window: JsdomWindow };

// jsdom ships no types and `@types/jsdom` is not a dependency; require it untyped
// (this file is test-only setup and is never part of the published package).
const require = createRequire(import.meta.url);
const { JSDOM } = require('jsdom') as { JSDOM: JsdomCtor };

const dom = new JSDOM('<!doctype html><html><head></head><body></body></html>', {
  url: 'https://mdreader.test/',
});

const win = dom.window;
const g = globalThis as unknown as Record<string, unknown>;

// Assigned with defineProperty so getter-only Node globals don't reject a plain
// assignment. `window` is the JSDOM window; the rest are pulled off it.
const define = (key: string, value: unknown): void => {
  Object.defineProperty(g, key, { value, configurable: true, writable: true });
};
define('window', win);
for (const key of [
  'document',
  'DOMParser',
  'Node',
  'DocumentFragment',
  'HTMLElement',
  'HTMLTemplateElement',
  'Element',
  'Text',
  'Comment',
]) {
  define(key, win[key]);
}
