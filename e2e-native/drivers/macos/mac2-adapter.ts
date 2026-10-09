/**
 * Milestone PT — the macOS platform adapter (appium-mac2-driver / XCUITest).
 *
 * This is the macOS leg of the UNIFIED harness. It drives the NATIVE shell through
 * a real Appium (mac2) WebDriver session — the things the old AppleScript + Core-
 * Graphics smoke did out-of-band, now over the WebDriver protocol:
 *
 *   - focus            → `macos: activateApp`
 *   - keystrokes/typing → `macos: keys` (XCUIKeyModifierFlags.command = 1<<4)
 *   - titlebar/content drag → `macos: clickAndDrag` (a genuine tracked gesture:
 *                          the window server sees a real drag, not a reposition)
 *   - resize           → `macos: clickAndDrag` on the bottom-right resize corner
 *   - red-X close      → `macos: click` on the window's close button, which the
 *                          session locates by XCUITest's standard
 *                          `_XCUI:CloseWindow` identifier (the close traffic light,
 *                          not minimize/zoom)
 *   - window geometry  → the XCUITest window element's rect
 *
 * For the WEBVIEW it reads WebKit's Accessibility tree via the compiled `axquery`
 * helper (WKWebView has no WebDriver bridge, and XCUITest's source doesn't surface
 * `AXDOMClassList`), against the SAME app process the session drives (by pid). The
 * before/after `screencapture` frame oracle corroborates each interaction step.
 *
 * Clicking INSIDE the webview goes through {@link Mac2Adapter.reveal}, because an
 * AX rectangle is not a click target: WebKit reports where an element is laid
 * out even when its scroll container has it out of view, and clicking that point
 * hits whatever is painted there (for a sheet, the backdrop — which closes it).
 * So the point is hit-tested first, and a control that is out of view is
 * scrolled into view the way a user would before it is clicked at all.
 *
 * READ-PATH FAULTS (RED proof the AX reads fail closed) are owned here, matching
 * the bespoke smoke:
 *   - `jump-closure-error`  — the palette-closure read spawns a MISSING axquery
 *                             binary → the read throws (spawn error).
 *   - `jump-ax-read-error`  — the closure read points axquery at an INVALID pid so
 *                             `AXUIElementCopyAttributeValue` returns a real error
 *                             and the Swift helper exits 3 → the read throws,
 *                             instead of a swallowed nil-and-print-0.
 *   - `frame-oracle`        — the AFTER capture grabs a wrong-size rect → a frame
 *                             ANOMALY → the step FAILS, never "fully changed".
 *
 * Node built-ins + a live WebdriverIO (Appium) session. No AppleScript, so it does
 * not need the separate "Automation"/Apple-Events TCC grant — only Accessibility
 * (for axquery) and Screen-Recording (for the capture oracle), which XCUITest and
 * the AX API already require.
 */

import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { inflateSync } from 'node:zlib';

import type { Chord } from '../../flows.ts';
import { AxNotHittable, makeAx, type Ax, type AxPoint, type AxSelector } from '../ax-helper.ts';
import { faulted, type DragOrigin, type FrameToken, type PlatformDriver, type Rect } from '../types.ts';

/** Fraction of sampled frame bytes that must change between BEFORE and AFTER for
 *  an interaction to count as "the action produced a visible result". Measured on
 *  this app: two identical frames differ ~0.1%; the smallest real transition
 *  (welcome → document) is ~1.9%. 0.5% sits well above noise and below the
 *  smallest real change. Same threshold the bespoke smoke used. */
const FRAME_DIFF_MIN = 0.005;

/** XCUIKeyModifierFlags.command (1 << 4). Verified: this is the value mac2's
 *  `macos: keys` needs for Cmd chords (the CGEvent maskCommand 1<<20 does NOT
 *  fire the app's shortcuts). */
const CMD = 16;

/** XCUIKeyModifierFlags.shift (1 << 1) — the `Mod+Shift+…` semantic chords. */
const SHIFT = 2;

/** How many scroll gestures {@link Mac2Adapter.reveal} will spend bringing a
 *  control into view before it gives up and FAILS the step. Six wheel notches of
 *  {@link SCROLL_NOTCH} cover ~360pt of overflow — more than any of the app's
 *  sheets can hide — and a bounded loop can never become a silent hang. */
const REVEAL_TRIES = 6;

/** Points of content one reveal gesture scrolls. Measured on this app: mac2's
 *  `deltaY` is 1pt per unit, so a notch of 60 moves a sheet by ~60pt. */
const SCROLL_NOTCH = 60;

/** A capture anomaly (blank / wrong-size / undecodable frame) — distinct from a
 *  genuine below-threshold diff. Retried; if it persists the step FAILS. */
class FrameAnomaly extends Error {}

export interface Mac2AdapterOptions {
  /** The live WebdriverIO (Appium mac2) session. */
  browser: WebdriverIO.Browser;
  /** The app's bundle identifier (for `macos: activateApp`). */
  bundleId: string;
  /** The running app's pid — the axquery reads + liveness checks target it. */
  pid: number;
  /** Path to the compiled `axquery` binary. */
  axBin: string;
  /** Path to the compiled `keypost` binary — the chord-delivery fallback. */
  keyBin: string;
  /** Directory to write frame-oracle captures into. */
  artifacts: string;
}

/** The shared `Chord` vocabulary in `keypost`'s notation. */
function keypostChord(chord: Chord): string {
  if (chord === 'Enter') return 'enter';
  if (chord === 'Escape') return 'escape';
  return chord.toLowerCase().replace('mod', 'cmd');
}

export class Mac2Adapter implements PlatformDriver {
  readonly name = 'macOS (appium-mac2-driver / XCUITest)';
  readonly windowOps = true;
  readonly frameOracle = true;
  private readonly b: WebdriverIO.Browser;
  private readonly bundleId: string;
  private readonly pid: number;
  private readonly axBin: string;
  private readonly keyBin: string;
  private readonly artifacts: string;
  private readonly ax: Ax;
  private shotSeq = 0;

  constructor(o: Mac2AdapterOptions) {
    this.b = o.browser;
    this.bundleId = o.bundleId;
    this.pid = o.pid;
    this.axBin = o.axBin;
    this.keyBin = o.keyBin;
    this.artifacts = o.artifacts;
    this.ax = makeAx(o.axBin, o.pid);
  }

  // ---- lifecycle / focus ----

  async focus(): Promise<void> {
    await this.b.execute('macos: activateApp', { bundleId: this.bundleId });
    await sleep(500);
  }

  async dispose(): Promise<void> {
    try {
      await this.b.deleteSession();
    } catch {
      /* session already torn down */
    }
  }

  // ---- WEBVIEW reads (fail closed via the axquery wrapper) ----

  async shellMounted(): Promise<boolean> {
    return this.isWindowPresent();
  }

  async countClass(cssClass: string): Promise<number> {
    return this.ax.count(cssClass);
  }

  async statusText(): Promise<string> {
    return this.ax.text('statusbar');
  }

  async headings(): Promise<string> {
    return this.ax.headings();
  }

  async readerText(): Promise<string> {
    // `.shell__main` is the surfaced AX ancestor of the generic `.mdreader` scroll
    // container macOS collapses — scoping heading/provenance reads to it keeps the
    // sidebar/status bar from supplying a false match.
    return this.ax.text('shell__main');
  }

  async paletteContainerCount(): Promise<number> {
    // READ-PATH FAULTS live here (see the module doc-comment). Each must THROW,
    // never read as "container gone / count 0".
    const INVALID_PID = 999_999; // above macOS PID_MAX (99999): never a live app
    const reader = faulted('jump-closure-error')
      ? makeAx(`${this.axBin}__missing`, this.pid) // spawn a missing binary → throws
      : faulted('jump-ax-read-error')
        ? makeAx(this.axBin, INVALID_PID) // genuine AX error → Swift exits 3 → throws
        : this.ax;
    return reader.count('mdsearch');
  }

  // ---- input (mac2 `macos: keys`) ----

  async chord(chord: Chord): Promise<void> {
    const keys =
      chord === 'Enter'
        ? [{ key: '\r' }]
        : chord === 'Escape'
          ? [{ key: String.fromCharCode(27) }]
          : [
              {
                key: chord.split('+').pop()!.toLowerCase(),
                modifierFlags: chord.includes('Shift') ? CMD | SHIFT : CMD,
              },
            ];
    await this.b.execute('macos: keys', { keys });
  }

  async chordDirect(chord: Chord): Promise<void> {
    // `execFileSync` throws on a nonzero exit, so an unmappable chord FAILS the
    // step instead of quietly posting nothing (keypost exits 2).
    execFileSync(this.keyBin, [String(this.pid), keypostChord(chord)]);
    await sleep(400);
  }

  async focusInput(cssClass: string): Promise<void> {
    // The palette input owns `autoFocus`. Wait for the focus WebKit actually
    // publishes instead of clicking an AX screen coordinate: mac2 and AX use
    // different coordinate spaces for web content, and the old click could
    // land on the Settings button behind the transient palette.
    for (let attempt = 0; attempt < 50; attempt++) {
      if (this.ax.focused(cssClass)) {
        return;
      }
      await sleep(100);
    }
    throw new Error(`.${cssClass} did not receive WebKit keyboard focus`);
  }

  async inputValue(cssClass: string): Promise<string> {
    return this.ax.value(cssClass);
  }

  async type(text: string): Promise<void> {
    await this.b.execute('macos: keys', { keys: [...text] });
  }

  async typeDirect(text: string): Promise<void> {
    execFileSync(this.keyBin, [String(this.pid), '--text', text]);
    await sleep(150);
  }

  // ---- current reader controls: read + click inside the webview ----

  async scopedText(cssClass: string): Promise<string> {
    return this.ax.text(cssClass);
  }

  async clickIn(cssClass: string, contains?: string): Promise<void> {
    const point = await this.reveal(cssClass, contains);
    await this.b.execute('macos: click', point);
    await sleep(350);
  }

  /**
   * A point that genuinely hits the named control, scrolling the surface it
   * lives on until it does.
   *
   * WebKit publishes an element's LAID-OUT rectangle whether or not it is
   * inside its clipping ancestor, so a control scrolled below the fold reports
   * a perfectly plausible frame that belongs, on screen, to whatever is painted
   * there — for a modal, the backdrop, whose click dismisses the sheet. That
   * failure is silent and it blames the wrong thing: the run reads "the lens is
   * not in force" when what happened is that the picker was closed out from
   * under it. So the click point comes from `ax.point`, which hit-tests before
   * it answers, and this loop treats "not hittable" as an instruction to reveal
   * the control the way a user would — point at the surface and scroll — rather
   * than as permission to click anyway.
   *
   * FAILS CLOSED at both ends: a control that does not exist throws out of
   * `ax.point` (axquery exit 4) on the first call, and one that never becomes
   * hittable throws here naming what it is and where it is laid out. Neither is
   * ever answered with a click at a guessed point.
   */
  private async reveal(selector: AxSelector, contains?: string): Promise<AxPoint> {
    // Hit-testing only means anything against the frontmost app.
    await this.focus();
    let lastReason = '';
    for (let attempt = 0; attempt <= REVEAL_TRIES; attempt++) {
      try {
        return this.ax.point(selector, 0, contains);
      } catch (err) {
        if (!(err instanceof AxNotHittable)) throw err;
        lastReason = err.message;
      }
      // Where it is laid out says which way to scroll: a control below the
      // window's middle is below the fold, one above it is above.
      const rect = this.ax.frame(selector, 0, contains);
      const window = await this.windowRect();
      const down = rect.y + rect.h / 2 > window.y + window.h / 2;
      await this.b.execute('macos: scroll', {
        // Point INTO the surface that is hiding it — same column as the
        // control, vertically in the middle of the window, which is inside the
        // sheet/rail/reader doing the clipping.
        x: Math.round(Math.min(Math.max(rect.x + rect.w / 2, window.x + 8), window.x + window.w - 8)),
        y: Math.round(window.y + window.h / 2),
        deltaX: 0,
        deltaY: down ? -SCROLL_NOTCH : SCROLL_NOTCH,
      });
      await sleep(250);
    }
    throw new Error(
      `could not bring ${selector}${contains === undefined ? '' : ` containing ${JSON.stringify(contains)}`} into view ` +
        `after ${REVEAL_TRIES} scroll gestures — refusing to click a point that does not resolve to it. (${lastReason})`,
    );
  }

  // ---- NATIVE window (mac2 execute-methods + XCUITest geometry) ----

  async isProcessAlive(): Promise<boolean> {
    try {
      process.kill(this.pid, 0);
      return true;
    } catch {
      return false;
    }
  }

  async isWindowPresent(): Promise<boolean> {
    if (!(await this.isProcessAlive())) return false;
    try {
      const win = await this.b.$('//XCUIElementTypeWindow');
      if (!(await win.isExisting())) return false;
      const loc = await win.getLocation();
      const size = await win.getSize();
      const webdriver = {
        x: Math.round(loc.x),
        y: Math.round(loc.y),
        w: Math.round(size.width),
        h: Math.round(size.height),
      };
      const owned = this.ax.appFrame();
      return (['x', 'y', 'w', 'h'] as const).every(
        (field) => Math.abs(webdriver[field] - owned[field]) <= 2,
      );
    } catch {
      return false;
    }
  }

  async windowRect(): Promise<Rect> {
    const win = await this.b.$('//XCUIElementTypeWindow');
    const loc = await win.getLocation();
    const size = await win.getSize();
    return { x: Math.round(loc.x), y: Math.round(loc.y), w: Math.round(size.width), h: Math.round(size.height) };
  }

  async dragBy(origin: DragOrigin, dx: number, dy: number): Promise<void> {
    const r = await this.windowRect();
    // Titlebar: mid-top, clear of the traffic lights. Content: lower-middle.
    const gx = r.x + Math.round(r.w / 2);
    const gy = origin === 'titlebar' ? r.y + 12 : r.y + Math.round(r.h * 0.6);
    await this.b.execute('macos: clickAndDrag', {
      startX: gx,
      startY: gy,
      endX: gx + dx,
      endY: gy + dy,
      duration: 0.6,
    });
  }

  async resizeBy(dw: number, dh: number): Promise<Rect> {
    const r = await this.windowRect();
    // Grab the bottom-right resize corner and drag it out by (dw,dh).
    const cx = r.x + r.w - 2;
    const cy = r.y + r.h - 2;
    await this.b.execute('macos: clickAndDrag', {
      startX: cx,
      startY: cy,
      endX: cx + dw,
      endY: cy + dh,
      duration: 0.6,
    });
    await sleep(400);
    return this.windowRect();
  }

  async clickClose(): Promise<void> {
    const { x, y } = await this.closeButtonTarget();
    await this.b.execute('macos: click', { x, y });
  }

  /** Center of THE window close button (the red traffic light), located via
   *  XCUITest's standard `_XCUI:CloseWindow` identifier — the same control
   *  `window.buttons[XCUIIdentifierCloseWindow]` resolves to. This targets the
   *  close control specifically (NOT the minimize `_XCUI:MinimizeWindow` or the
   *  fullscreen/zoom `_XCUI:FullScreenWindow` sibling), so we can only ever click
   *  close.
   *
   *  FAIL CLOSED: the geometric fallback fires ONLY when the element GENUINELY
   *  isn't found (`isExisting()` is false — no `_XCUI:CloseWindow` match). If the
   *  element IS found but its location/size query then fails, that is a real error
   *  and PROPAGATES — we never mask it by silently clicking an estimated point
   *  (which could hit the wrong control). A lookup that errors outright likewise
   *  propagates. Only the clean "not found" path is soft, and it logs. */
  private async closeButtonTarget(): Promise<{ x: number; y: number }> {
    const closeBtn = await this.b.$('//XCUIElementTypeButton[@identifier="_XCUI:CloseWindow"]');
    if (await closeBtn.isExisting()) {
      // Element found — a query failure here is a REAL error; let it throw.
      const loc = await closeBtn.getLocation();
      const size = await closeBtn.getSize();
      return { x: Math.round(loc.x + size.width / 2), y: Math.round(loc.y + size.height / 2) };
    }
    const r = await this.windowRect();
    console.warn(
      `mac2: window close button (_XCUI:CloseWindow) not found via XCUITest — ` +
        `falling back to the top-left geometric estimate at (${r.x + 16},${r.y + 16}).`,
    );
    return { x: r.x + 16, y: r.y + 16 };
  }

  // ---- frame oracle (screencapture before/after) ----

  async captureFrame(): Promise<FrameToken> {
    await this.focus();
    const r = await this.windowRect();
    return readFileSync(this.captureWindow(r, this.shot('before')));
  }

  async assertFrameChanged(before: FrameToken, label: string, tries = 4): Promise<number> {
    let lastAnomaly = '';
    for (let attempt = 1; attempt <= tries; attempt++) {
      await this.focus();
      const r = await this.windowRect();
      // FAULT "frame-oracle": grab a deliberately wrong-size (8x8) rect for every
      // attempt, exercising the "mismatch → FAIL, not fully-changed" path.
      const after = faulted('frame-oracle')
        ? readFileSync(this.capture({ x: 0, y: 0, w: 8, h: 8 }, this.shot('after')))
        : readFileSync(this.captureWindow(r, this.shot('after')));
      try {
        const frac = frameDiffFraction(before, after);
        if (frac < FRAME_DIFF_MIN) {
          throw new Error(
            `no visible change for ${label} (diff ${(frac * 100).toFixed(1)}% < ` +
              `${(FRAME_DIFF_MIN * 100).toFixed(1)}%) — the action rendered nothing.`,
          );
        }
        return frac;
      } catch (err) {
        if (!(err instanceof FrameAnomaly)) throw err; // genuine below-threshold → fail now
        lastAnomaly = err.message;
        await sleep(400);
      }
    }
    throw new Error(
      `frame oracle could not obtain comparable captures for ${label} after ${tries} tries ` +
        `(last: ${lastAnomaly}) — refusing to treat a blank/wrong-size frame as a change.`,
    );
  }

  private shot(tag: string): string {
    return join(this.artifacts, `frame-${String(this.shotSeq++).padStart(2, '0')}-${tag}.png`);
  }

  private capture(r: Rect, path: string): string {
    execFileSync('screencapture', ['-x', '-R', `${r.x},${r.y},${r.w},${r.h}`, path]);
    return path;
  }

  private captureWindow(r: Rect, path: string): string {
    return this.capture(r, path);
  }
}

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

/** A PNG's inflated (filtered) scanline bytes — the raw pixel-ish stream. */
function rawScanlines(png: Buffer): Buffer {
  if (png.length < 8 || png.readUInt32BE(0) !== 0x89504e47) throw new Error('not a PNG');
  let off = 8;
  const idat: Buffer[] = [];
  while (off + 8 <= png.length) {
    const len = png.readUInt32BE(off);
    const type = png.toString('ascii', off + 4, off + 8);
    const start = off + 8;
    if (type === 'IDAT') idat.push(png.subarray(start, start + len));
    if (type === 'IEND') break;
    off = start + len + 4;
  }
  return inflateSync(Buffer.concat(idat));
}

/** Fraction of sampled bytes that differ between two same-window frames. A size or
 *  format mismatch is NOT "fully changed" — it is a capture ANOMALY and THROWS. */
function frameDiffFraction(beforePng: Buffer, afterPng: Buffer): number {
  let a: Buffer;
  let b: Buffer;
  try {
    a = rawScanlines(beforePng);
    b = rawScanlines(afterPng);
  } catch (err) {
    throw new FrameAnomaly(`undecodable frame: ${err instanceof Error ? err.message : String(err)}`);
  }
  if (a.length !== b.length) {
    throw new FrameAnomaly(`frame size mismatch (${a.length} vs ${b.length} decoded bytes)`);
  }
  const stride = Math.max(1, Math.floor(a.length / 200_000));
  let n = 0;
  let changed = 0;
  for (let i = 0; i < a.length; i += stride) {
    n++;
    if (Math.abs(a[i]! - b[i]!) > 8) changed++;
  }
  return n === 0 ? 0 : changed / n;
}
