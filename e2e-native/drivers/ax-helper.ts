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
 * That last sentence needs one more guarantee to be true of a COUNT, because a
 * count of 0 is the one read an assertion consumes as an ABSENCE (`jumpToHit`
 * requires the `.mdsearch` container to be gone). "No element carries this class"
 * and "this process cannot see element classes at all" are different facts that
 * arrive as the same number, so axquery answers the second one with exit 5
 * (`AX_UNREADABLE`) instead of a 0 — see its EXIT CODES — and {@link makeAx}
 * retries it only for as long as a just-launched app can still be publishing its
 * web area, then THROWS.
 *
 * The same discipline covers the CLICK TARGETS. WebKit publishes an element's
 * laid-out rectangle whether or not it is inside its clipping ancestor, so the
 * frame of a control scrolled below a modal's fold is a perfectly plausible
 * point that belongs, on screen, to the backdrop behind it — and clicking it
 * dismisses the modal instead. {@link Ax.point} therefore hit-tests before it
 * answers, and reports "found, but not where it says it is" as
 * {@link AxNotHittable} (axquery exit 6) so the caller reveals the control and
 * asks again rather than clicking regardless.
 *
 * The mac2 adapter runs these against the SAME app process its WebDriver session
 * drives (located by pid), so the native XCUITest layer and the AX read layer see
 * one running app. Node built-ins only.
 */

import { execFileSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));

/** axquery's "that 0 is not an answer" exit: the count came out 0 while the
 *  class-list channel itself is not proven alive (no `AXWebArea`, or not one
 *  element in the app publishing a class list). See `macos/axquery.swift`. */
const EXIT_UNREADABLE = 5;

/** axquery's "it is not where it says it is" exit: the element was found and is
 *  laid out, but nothing at its rectangle hit-tests back to it — it is scrolled
 *  outside a clipping ancestor, or something is painted over it. See
 *  `macos/axquery.swift` and {@link AxNotHittable}. */
const EXIT_NOT_HITTABLE = 6;

/**
 * The located control exists but cannot be clicked WHERE IT IS LAID OUT.
 *
 * Its own class — not a generic Error — because it is the one failure a caller
 * can legitimately act on rather than report: reveal the control (scroll the
 * surface it lives on) and ask again. Every other read failure stays fatal.
 */
export class AxNotHittable extends Error {}

/** How long to keep re-reading through an `AX_UNREADABLE` before giving up. The
 *  webview's AX content root lands a beat AFTER the native window the `launch`
 *  step waits on, so the first read of a just-launched app can legitimately hit
 *  the not-yet-alive state. Bounded, then it THROWS — a read path that stays
 *  unreadable is a failure, never a "0". */
const UNREADABLE_TRIES = 8;
const UNREADABLE_WAIT_MS = 400;

/** WebKit can invalidate a node already yielded by an AX traversal
 * (`kAXErrorInvalidUIElement`) or return `kAXErrorCannotComplete` while it
 * republishes a changing accessibility subtree. A fresh helper invocation starts
 * from the application's current root again, so retry only those two transition
 * outcomes. Missing elements, invalid processes, denied access, and every other
 * read error still propagate on the first attempt. */
const TRANSIENT_AX_CODES = ['code=-25202', 'code=-25204'] as const;
const TRANSIENT_AX_TRIES = 41;
const TRANSIENT_AX_WAIT_MS = 250;

/** Block the calling (synchronous) read for `ms`. The whole AX surface is sync
 *  `execFileSync`, so the retry has to be too — Node built-ins only. */
function sleepSync(ms: number): void {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

/** Compile `<name>.swift` (from `drivers/macos/`) to `<artifacts>/<name>` and
 *  return the binary path. */
export function buildSwiftHelper(artifacts: string, name: string): string {
  const src = join(HERE, 'macos', `${name}.swift`);
  const bin = join(artifacts, name);
  execFileSync('swiftc', ['-O', '-o', bin, src]);
  return bin;
}

/** A screen rectangle in global, top-left-origin coordinates. */
export interface AxRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** A screen point in the same coordinate space as {@link AxRect}. */
export interface AxPoint {
  x: number;
  y: number;
}

/**
 * How an element is addressed. A bare string is a CSS class (what
 * `AXDOMClassList` carries). `text:<words>` addresses the innermost element
 * that RENDERS those words — the only way to reach the reading column's blocks,
 * which WebKit leaves out of the AX tree entirely because they are generic
 * `<div>`s whose children are all block-level.
 */
export type AxSelector = string;

/** The fail-closed AX read surface over a compiled `axquery` binary + a pid. */
export interface Ax {
  /** Number of elements whose `AXDOMClassList` contains `cssClass`. THROWS
   *  rather than returning 0 when the app publishes no class list to read at
   *  all — a 0 from this method always means "counted, and found none". */
  count(cssClass: string): number;
  /** Text of every element, or of the subtree(s) scoped to `cssClass`. */
  text(cssClass?: string): string;
  /** Text of every rendered heading (AXHeading). */
  headings(): string;
  /** Whether the single element carrying `cssClass` owns keyboard focus.
   * Throws when the selector is absent or ambiguous. */
  focused(cssClass: string): boolean;
  /** The matched element's AXValue only, excluding its accessible label and
   * descendants. Throws when the selector is absent or ambiguous. */
  value(cssClass: string): string;
  /** Exact native window frame owned by the pid this helper was bound to. */
  appFrame(): AxRect;
  /**
   * Screen rectangle of the `nth` (0-based, document order) element the
   * `selector` picks out, whose own subtree text contains `contains` (when
   * given). THROWS when there is no such element (axquery exits 4): a missing
   * control must never be answered with a rectangle nobody rendered.
   *
   * This is a LAYOUT rectangle and not a click target — see {@link point}.
   */
  frame(selector: AxSelector, nth: number, contains?: string): AxRect;
  /**
   * A point that HIT-TESTS to that same element — the only coordinate the
   * harness may click, addressed by what the control SAYS rather than by an
   * ordinal that shifts.
   *
   * WebKit publishes an element's laid-out frame even when it is scrolled
   * outside its clipping ancestor, so clicking a {@link frame} blind lands on
   * whatever is painted there instead. Throws {@link AxNotHittable} for exactly
   * that case (axquery exits 6) so the caller can reveal the control and ask
   * again; every other failure stays fatal.
   */
  point(selector: AxSelector, nth: number, contains?: string): AxPoint;
}

/**
 * Bind the compiled `axquery` binary to a target `pid`. Every read FAILS CLOSED:
 * `execFileSync` throws on a spawn failure or nonzero exit (the Swift helper exits
 * 3 on a real AX error rather than printing a false `0`), and a malformed count
 * throws rather than being coerced. A deliberately-broken `bin`/`pid` (the mac2
 * adapter's read-path fault probes) therefore turns the step RED, never green.
 */
export function makeAx(bin: string, pid: number): Ax {
  const run = (sub: string, extra: string[] = []): string => {
    for (let attempt = 1; ; attempt++) {
      try {
        return execFileSync(bin, [sub, String(pid), ...extra], {
          encoding: 'utf8',
        }).trim();
      } catch (error) {
        const stderr = String((error as { stderr?: string }).stderr ?? '');
        const retryable =
          (error as { status?: number }).status === 3 &&
          TRANSIENT_AX_CODES.some((code) => stderr.includes(code));
        if (!retryable || attempt >= TRANSIENT_AX_TRIES) throw error;
        sleepSync(TRANSIENT_AX_WAIT_MS);
      }
    }
  };
  const parseRect = (out: string, what: string): AxRect => {
    const parts = out.split(/\s+/).map((token) => Number.parseInt(token, 10));
    if (parts.length !== 4 || parts.some((n) => !Number.isSafeInteger(n))) {
      throw new Error(`axquery ${what} returned unparseable output: ${JSON.stringify(out)}`);
    }
    const [x, y, w, h] = parts as [number, number, number, number];
    if (w <= 0 || h <= 0) {
      throw new Error(`axquery ${what} is not a window rectangle: ${w}x${h} at (${x},${y})`);
    }
    return { x, y, w, h };
  };
  return {
    count: (cssClass) => {
      // A 0 is load-bearing (`jumpToHit` asserts the palette container is GONE by
      // reading exactly this number), so axquery refuses to print one it cannot
      // stand behind and exits 5 instead. Absorb that only for as long as a
      // just-launched app can legitimately still be publishing its web area —
      // then surface it, with the helper's own diagnosis attached.
      let out = '';
      for (let attempt = 1; ; attempt++) {
        try {
          out = run('count', [cssClass]);
          break;
        } catch (err) {
          const status = (err as { status?: number }).status;
          if (status !== EXIT_UNREADABLE) throw err;
          if (attempt >= UNREADABLE_TRIES) {
            const why = String((err as { stderr?: string }).stderr ?? '').trim();
            throw new Error(
              `axquery count(${cssClass}) could not be performed after ${attempt} tries: ${why || 'AX_UNREADABLE'} ` +
                '— the app publishes no readable class list, so this is NOT "0 / absent".',
            );
          }
          sleepSync(UNREADABLE_WAIT_MS);
        }
      }
      const n = Number.parseInt(out, 10);
      if (!/^\d+$/.test(out) || !Number.isSafeInteger(n)) {
        throw new Error(`axquery count(${cssClass}) returned unparseable output: ${JSON.stringify(out)}`);
      }
      return n;
    },
    text: (cssClass) => run('text', cssClass ? [cssClass] : []),
    headings: () => run('headings'),
    focused: (cssClass) => {
      const out = run('focused', [cssClass]);
      if (out !== '0' && out !== '1') {
        throw new Error(
          `axquery focused(${cssClass}) returned unparseable output: ${JSON.stringify(out)}`,
        );
      }
      return out === '1';
    },
    value: (cssClass) => run('value', [cssClass]),
    appFrame: () => parseRect(run('appframe'), 'appframe'),
    frame: (selector, nth, contains) => {
      // A nonzero exit (4 = no such element, 3 = a real AX error) throws out of
      // `execFileSync`, which is exactly what we want: the caller must not get
      // a point to click.
      const out = run('frame', locator(selector, nth, contains));
      const what = label(selector, nth, contains);
      return parseRect(out, `frame(${what})`);
    },
    point: (selector, nth, contains) => {
      const what = label(selector, nth, contains);
      let out: string;
      try {
        out = run('point', locator(selector, nth, contains));
      } catch (err) {
        // Exit 6 is the ONE outcome a caller may act on rather than report: the
        // control is real but not where it is laid out. Everything else — a
        // missing element (4), a genuine AX error (3), a spawn failure — stays
        // fatal and propagates.
        if ((err as { status?: number }).status !== EXIT_NOT_HITTABLE) throw err;
        throw new AxNotHittable(
          `axquery point(${what}): ` +
            (String((err as { stderr?: string }).stderr ?? '').trim() || 'AX_NOT_HITTABLE'),
        );
      }
      const parts = out.split(/\s+/).map((token) => Number.parseInt(token, 10));
      if (parts.length !== 2 || parts.some((n) => !Number.isSafeInteger(n))) {
        throw new Error(`axquery point(${what}) returned unparseable output: ${JSON.stringify(out)}`);
      }
      const [x, y] = parts as [number, number];
      return { x, y };
    },
  };
}

/** The argv tail both `frame` and `point` take. */
function locator(selector: AxSelector, nth: number, contains?: string): string[] {
  return contains === undefined ? [selector, String(nth)] : [selector, String(nth), contains];
}

/** How a lookup is named in an error message. */
function label(selector: AxSelector, nth: number, contains?: string): string {
  return `${selector}[${nth}]${contains === undefined ? '' : ` containing ${JSON.stringify(contains)}`}`;
}
