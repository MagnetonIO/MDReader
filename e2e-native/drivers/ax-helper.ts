/**
 * Milestone PT — the compiled Accessibility (AX) read bridge for the macOS leg.
 *
 * WKWebView exposes no WebDriver bridge, and appium-mac2-driver's XCUITest source
 * surfaces only a FIXED attribute set (label/value/title/frame/…) — never the
 * private `AXDOMClassList` that carries a DOM element's CSS class list. So the
 * class-based reads that the shared `flows.ts` is expressed in (count
 * `.filetree__row`, read the `statusbar` subtree, list headings, read the
 * `.shell__main` reader region, assert the `.mdsearch` container is gone) go
 * through the raw AX API. This module compiles the committed Swift helper
 * (`macos/axquery.swift`) once and wraps it with the SAME fail-closed contract the
 * bespoke smoke used: a helper spawn failure, a nonzero Swift exit (a genuine
 * `AXUIElementCopyAttributeValue` error), or unparseable output THROWS — it is
 * never coerced to "" / 0. Only a successful query that genuinely found nothing
 * yields "" / 0.
 *
 * The mac2 adapter runs these against the SAME app process its WebDriver session
 * drives (located by pid), so the native XCUITest layer and the AX read layer see
 * one running app. Node built-ins only.
 */

import { execFileSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));

/** Compile `<name>.swift` (from `drivers/macos/`) to `<artifacts>/<name>` and
 *  return the binary path. */
export function buildSwiftHelper(artifacts: string, name: string): string {
  const src = join(HERE, 'macos', `${name}.swift`);
  const bin = join(artifacts, name);
  execFileSync('swiftc', ['-O', '-o', bin, src]);
  return bin;
}

/** The fail-closed AX read surface over a compiled `axquery` binary + a pid. */
export interface Ax {
  /** Number of elements whose `AXDOMClassList` contains `cssClass`. */
  count(cssClass: string): number;
  /** Text of every element, or of the subtree(s) scoped to `cssClass`. */
  text(cssClass?: string): string;
  /** Text of every rendered heading (AXHeading). */
  headings(): string;
}

/**
 * Bind the compiled `axquery` binary to a target `pid`. Every read FAILS CLOSED:
 * `execFileSync` throws on a spawn failure or nonzero exit (the Swift helper exits
 * 3 on a real AX error rather than printing a false `0`), and a malformed count
 * throws rather than being coerced. A deliberately-broken `bin`/`pid` (the mac2
 * adapter's read-path fault probes) therefore turns the step RED, never green.
 */
export function makeAx(bin: string, pid: number): Ax {
  const run = (sub: string, extra: string[] = []): string =>
    execFileSync(bin, [sub, String(pid), ...extra], { encoding: 'utf8' }).trim();
  return {
    count: (cssClass) => {
      const out = run('count', [cssClass]);
      const n = Number.parseInt(out, 10);
      if (!/^\d+$/.test(out) || !Number.isSafeInteger(n)) {
        throw new Error(`axquery count(${cssClass}) returned unparseable output: ${JSON.stringify(out)}`);
      }
      return n;
    },
    text: (cssClass) => run('text', cssClass ? [cssClass] : []),
    headings: () => run('headings'),
  };
}
