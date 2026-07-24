/**
 * Milestone PT — the macOS GUI smoke DRIVER.
 *
 * WKWebView (the macOS webview) exposes no WebDriver endpoint, so the WebdriverIO
 * suite cannot drive the real app here. This driver does it the only way macOS
 * allows: it launches the real `.app`, then drives + inspects the NATIVE window
 * through AppleScript System Events (keystrokes, position/size/close-button) and
 * CoreGraphics synthetic input (`cgdrag`), and — the part that makes its DOM
 * assertions RIGOROUS rather than pixel-guessing — it reads the webview's
 * Accessibility tree with the compiled `axquery` helper. WebKit publishes each
 * DOM element's CSS class list on `AXDOMClassList`, so this driver enforces the
 * SAME semantic contract the WebdriverIO leg does (the shared `FLOW`): it counts
 * `.filetree__row` rows, parses the status-bar text against the same regex,
 * matches a rendered heading, counts palette result rows, and proves the reader
 * scrolled to a hit's provenance line.
 *
 * What is asserted, and why a "process booted" check could never catch it:
 *   - launch:        a native window exists (the process is alive).
 *   - grantWorkspace: the file tree is populated to >= minFiles rows (AX count of
 *                     `.filetree__row`) — not "the screen changed".
 *   - waitIndexed:   the STATUS BAR reads /Indexed N files/ (the same regex the
 *                     WebdriverIO leg uses), AND the engine wrote its index to the
 *                     workspace-scoped path `workspaces/<blake3(canonical root)>/
 *                     catdb.db` (a stale db elsewhere cannot satisfy it). The
 *                     status-bar signal is the gate; the db is corroboration.
 *   - openDoc:       after Cmd+P → open, a rendered HEADING matches the expected
 *                     pattern (AX heading read) AND the frame changed (before/
 *                     after diff), not merely "a non-blank frame".
 *   - search:        after Cmd+K → type, >= 1 palette result row is present (AX
 *                     count of `.mdsearch__item`).
 *   - jumpToHit:     after Enter, the search dialog container (`.mdsearch`) is
 *                     positively ABSENT (closure is not inferred from a row
 *                     count), the jumped-to document's heading is shown, and its
 *                     provenance line — text unique to that document — is in the
 *                     reader region (`.shell__main`). The jump really landed.
 *   - dragWindow:    a SYNTHETIC titlebar drag moves the window by ~the delta,
 *                     AND a NEGATIVE-CONTROL drag started in the CONTENT area does
 *                     NOT move it. Together they prove the titlebar is genuinely
 *                     and *locally* draggable — a dead drag region, or a
 *                     whole-window drag region, fails one or the other.
 *   - resizeWindow:  the window size changes.
 *   - closeWindow:   a SYNTHETIC click on the red close button ends the process.
 *                    An intercepted/blocked close (the bug that shipped) → the
 *                    process survives → FAIL.
 *
 * Fault injection (RED proof): set `MDREADER_SMOKE_FAULT=<id>` to break the real
 * behaviour a step exercises, or to break the harness's own read path, and
 * confirm the driver goes RED there. `<id>` is a step id (e.g. `drag-window`
 * drags the whole window on the negative control; `close-window` skips the red-X
 * click) OR one of the FAIL-OPEN probes: `jump-closure-error` (the palette-
 * closure read spawns a MISSING helper — the TS makeAx path throws),
 * `jump-ax-read-error` (the closure read hits a GENUINE AXUIElementCopyAttribute-
 * Value error via an invalid pid — the Swift helper exits nonzero instead of
 * printing a false 0), and `frame-oracle` (feeds a wrong-size frame to the visual
 * oracle — must FAIL, never "fully changed"). Each must FAIL the step, never be
 * read as "palette gone / count 0 / fully changed". It proves each assertion is
 * load-bearing and fails CLOSED. Inert unless the env var is set.
 *
 * Node built-ins only. Runs under Node's native TypeScript (Node >= 22.13 with
 * `--experimental-strip-types`; >= 23 strips by default) — no build step.
 */

import { execFileSync, spawn } from 'node:child_process';
import {
  existsSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { inflateSync } from 'node:zlib';

import { FLOW, FLOW_CONFIG, type Step } from '../flows.ts';
import { appConfigDir, clearSeed, seedWorkspace } from '../seed.ts';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, '..', '..', '..', '..');

/** Fraction of sampled frame bytes that must change between the BEFORE and AFTER
 *  capture of an interaction step for it to count as "the action produced a
 *  visible result". Only the content pane changes (the sidebar/titlebar/status
 *  bar are identical across the transition), so the fraction is modest. Measured:
 *  two identical frames differ ~0.1% (compression/cursor noise); the smallest
 *  real transition (welcome → document) is ~1.9%, and document → palette /
 *  palette → reader are ~6%. 0.5% sits ~4.5x above the noise floor and ~3.8x
 *  below the smallest real change — it rejects "nothing moved" without flaking. */
const FRAME_DIFF_MIN = 0.005;

/** The step id whose real behaviour to break, for RED evidence. Empty in normal
 *  runs. See the module doc-comment. */
const FAULT = process.env.MDREADER_SMOKE_FAULT ?? '';
const faulted = (id: string): boolean => FAULT === id;

interface Options {
  app: string;
  binary: string;
  corpus: string;
  artifacts: string;
  identifier: string;
  keep: boolean;
}

function parseArgs(argv: string[]): Options {
  const get = (flag: string): string | undefined => {
    const i = argv.indexOf(flag);
    return i >= 0 ? argv[i + 1] : undefined;
  };
  const app = resolve(
    get('--app') ?? join(REPO_ROOT, 'target/release/bundle/macos/MDReader.app'),
  );
  return {
    app,
    binary: join(app, 'Contents/MacOS/mdreader-app'),
    corpus: resolve(get('--corpus') ?? join(REPO_ROOT, FLOW_CONFIG.corpusRelPath)),
    artifacts: resolve(get('--artifacts') ?? join(REPO_ROOT, 'target/e2e-artifacts/macos')),
    identifier: FLOW_CONFIG.bundleIdentifier,
    keep: argv.includes('--keep'),
  };
}

// --------------------------------------------------------------------------
// BLAKE3 (single chunk) — to locate the engine's per-workspace index dir.
//
// The engine writes its db to `<app-data-dir>/workspaces/<blake3(canonical
// root)>/catdb.db` (see engine.rs `db_path_under`). Reproducing that hash lets
// the driver scope its "the index exists" check to THIS workspace, so a stale db
// left by another workspace can never satisfy it. macOS PATH_MAX is 1024, so any
// canonical path fits in a single BLAKE3 chunk — a single-chunk implementation is
// exact here. Verified against the official empty-input vector and the real dir
// the app creates for the fixture corpus.
// --------------------------------------------------------------------------

const B3_IV = [
  0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];
const B3_MSG = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];
const B3_CHUNK_START = 1;
const B3_CHUNK_END = 2;
const B3_ROOT = 8;

const rotr = (x: number, n: number): number => ((x >>> n) | (x << (32 - n))) >>> 0;

function b3g(v: number[], a: number, b: number, c: number, d: number, mx: number, my: number): void {
  v[a] = (v[a]! + v[b]! + mx) >>> 0;
  v[d] = rotr(v[d]! ^ v[a]!, 16);
  v[c] = (v[c]! + v[d]!) >>> 0;
  v[b] = rotr(v[b]! ^ v[c]!, 12);
  v[a] = (v[a]! + v[b]! + my) >>> 0;
  v[d] = rotr(v[d]! ^ v[a]!, 8);
  v[c] = (v[c]! + v[d]!) >>> 0;
  v[b] = rotr(v[b]! ^ v[c]!, 7);
}

function b3round(v: number[], m: number[]): void {
  b3g(v, 0, 4, 8, 12, m[0]!, m[1]!);
  b3g(v, 1, 5, 9, 13, m[2]!, m[3]!);
  b3g(v, 2, 6, 10, 14, m[4]!, m[5]!);
  b3g(v, 3, 7, 11, 15, m[6]!, m[7]!);
  b3g(v, 0, 5, 10, 15, m[8]!, m[9]!);
  b3g(v, 1, 6, 11, 12, m[10]!, m[11]!);
  b3g(v, 2, 7, 8, 13, m[12]!, m[13]!);
  b3g(v, 3, 4, 9, 14, m[14]!, m[15]!);
}

function b3compress(cv: number[], block: number[], blockLen: number, flags: number): number[] {
  const v = [
    cv[0]!, cv[1]!, cv[2]!, cv[3]!, cv[4]!, cv[5]!, cv[6]!, cv[7]!,
    B3_IV[0]!, B3_IV[1]!, B3_IV[2]!, B3_IV[3]!,
    0, 0, blockLen >>> 0, flags >>> 0,
  ];
  let m = block.slice();
  for (let r = 0; r < 7; r++) {
    b3round(v, m);
    if (r < 6) {
      const nm = new Array<number>(16);
      for (let i = 0; i < 16; i++) nm[i] = m[B3_MSG[i]!]!;
      m = nm;
    }
  }
  const out = new Array<number>(8);
  for (let i = 0; i < 8; i++) out[i] = (v[i]! ^ v[i + 8]!) >>> 0;
  return out;
}

/** Lowercase hex BLAKE3-256 of `buf` (single chunk; buf.length must be <= 1024). */
function blake3hex(buf: Buffer): string {
  if (buf.length > 1024) throw new Error('blake3hex: single-chunk only (input > 1024 bytes)');
  const nBlocks = Math.max(1, Math.ceil(buf.length / 64));
  let cv = B3_IV.slice();
  for (let bi = 0; bi < nBlocks; bi++) {
    const start = bi * 64;
    const blockLen = Math.min(64, buf.length - start);
    const words = new Array<number>(16).fill(0);
    for (let w = 0; w < 16; w++) {
      const o = start + w * 4;
      words[w] =
        (((buf[o] ?? 0) | ((buf[o + 1] ?? 0) << 8) | ((buf[o + 2] ?? 0) << 16) | ((buf[o + 3] ?? 0) << 24)) >>>
          0);
    }
    let flags = 0;
    if (bi === 0) flags |= B3_CHUNK_START;
    if (bi === nBlocks - 1) flags |= B3_CHUNK_END | B3_ROOT; // single chunk ⇒ also the root
    cv = b3compress(cv, words, blockLen, flags);
  }
  let hex = '';
  for (let i = 0; i < 8; i++) {
    const le = cv[i]!;
    hex += [le & 0xff, (le >>> 8) & 0xff, (le >>> 16) & 0xff, (le >>> 24) & 0xff]
      .map((b) => b.toString(16).padStart(2, '0'))
      .join('');
  }
  return hex;
}

// --------------------------------------------------------------------------
// osascript / System Events helpers
// --------------------------------------------------------------------------

/** Run one AppleScript (multi-line allowed) and return its trimmed stdout.
 *  stderr is captured (not inherited) so an expected failure — e.g. probing a
 *  process that just exited — stays silent; callers that care catch the throw. */
function osa(script: string): string {
  return execFileSync('osascript', ['-e', script], {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  }).trim();
}

/** The `tell (first process whose unix id is PID)` preamble. */
function proc(pid: number): string {
  return `first process whose unix id is ${pid}`;
}

/** Number of windows the process currently owns (0 if it has none / is gone). */
function windowCount(pid: number): number {
  try {
    return Number.parseInt(
      osa(`tell application "System Events" to tell (${proc(pid)})
        return (count of windows) as string
      end tell`),
      10,
    );
  } catch {
    return 0;
  }
}

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

function windowRect(pid: number): Rect {
  const out = osa(`tell application "System Events" to tell (${proc(pid)})
    set b to position of window 1
    set s to size of window 1
    return ((item 1 of b) as string) & "," & ((item 2 of b) as string) & "," & ((item 1 of s) as string) & "," & ((item 2 of s) as string)
  end tell`);
  const [x, y, w, h] = out.split(',').map(Number);
  return { x: x!, y: y!, w: w!, h: h! };
}

/** The red close button's rect, via its AX subrole. */
function closeButtonRect(pid: number): Rect {
  const out = osa(`tell application "System Events" to tell (${proc(pid)})
    set cb to (first button of window 1 whose subrole is "AXCloseButton")
    set p to position of cb
    set z to size of cb
    return ((item 1 of p) as string) & "," & ((item 2 of p) as string) & "," & ((item 1 of z) as string) & "," & ((item 2 of z) as string)
  end tell`);
  const [x, y, w, h] = out.split(',').map(Number);
  return { x: x!, y: y!, w: w!, h: h! };
}

function resizeWindowTo(pid: number, w: number, h: number): void {
  osa(`tell application "System Events" to tell (${proc(pid)})
    set size of window 1 to {${w}, ${h}}
  end tell`);
}

/** Throw a clear error if the app is no longer running — turns a raw osascript
 *  "-1719 invalid index" into an actionable message when the app exits early. */
function assertAlive(pid: number, where: string): void {
  if (!isAlive(pid)) {
    throw new Error(`the app exited unexpectedly before "${where}" (pid ${pid} is gone)`);
  }
}

/** Bring the app forward and raise its window — the precondition both for
 *  keystrokes to reach the webview AND for an unoccluded screencapture. NO
 *  coordinate clicks (those can activate other apps). */
function focusWindow(pid: number): void {
  assertAlive(pid, 'focus');
  osa(`tell application "System Events"
    set frontmost of (${proc(pid)}) to true
    delay 0.4
    tell (${proc(pid)}) to perform action "AXRaise" of window 1
    delay 0.3
  end tell`);
}

/** Send a shell chord (`Mod+K` etc.) to the focused window via System Events. */
function sendChord(chord: string): void {
  if (chord === 'Enter') {
    osa('tell application "System Events" to key code 36');
    return;
  }
  if (chord === 'Escape') {
    osa('tell application "System Events" to key code 53');
    return;
  }
  const parts = chord.split('+');
  const key = parts[parts.length - 1]!.toLowerCase();
  // `Mod` is Cmd on macOS.
  const useCommand = parts.includes('Mod');
  const using = useCommand ? ' using command down' : '';
  osa(`tell application "System Events" to keystroke "${key}"${using}`);
}

function typeText(text: string): void {
  osa(`tell application "System Events" to keystroke "${text.replace(/"/g, '\\"')}"`);
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

// --------------------------------------------------------------------------
// axquery (Accessibility reads) — compiled once from the committed Swift source
// --------------------------------------------------------------------------

function buildSwift(artifacts: string, name: string): string {
  const src = join(HERE, `${name}.swift`);
  const bin = join(artifacts, name);
  execFileSync('swiftc', ['-O', '-o', bin, src]);
  return bin;
}

interface Ax {
  /** Number of DOM elements whose CSS class list contains `cssClass`. */
  count(cssClass: string): number;
  /** Text of every element, or of the subtree(s) scoped to `cssClass`. */
  text(cssClass?: string): string;
  /** Text of every rendered heading (AXHeading). */
  headings(): string;
}

function makeAx(bin: string, pid: number): Ax {
  // axquery's arg order is `<subcommand> <pid> [cssClass]`. FAIL CLOSED: a helper
  // invocation error, a nonzero swift exit, or unparseable output must THROW —
  // never be coerced to an empty string or a count of 0. Otherwise "0" would be
  // ambiguous (genuine-zero vs helper-broke) and a broken query could false-pass
  // a "== 0"/absence check. Only a SUCCESSFUL query that genuinely found nothing
  // yields "" / 0.
  const run = (sub: string, extra: string[] = []): string => {
    // No try/catch: execFileSync throws on spawn failure or nonzero exit, and we
    // want that to propagate and fail the step.
    return execFileSync(bin, [sub, String(pid), ...extra], { encoding: 'utf8' }).trim();
  };
  return {
    count: (cssClass) => {
      const out = run('count', [cssClass]);
      const n = Number.parseInt(out, 10);
      // Reject anything that isn't exactly a non-negative integer (e.g. "",
      // "NaN", "3\n7", garbage) so a malformed read can't masquerade as a count.
      if (!/^\d+$/.test(out) || !Number.isSafeInteger(n)) {
        throw new Error(
          `axquery count(${cssClass}) returned unparseable output: ${JSON.stringify(out)}`,
        );
      }
      return n;
    },
    text: (cssClass) => run('text', cssClass ? [cssClass] : []),
    headings: () => run('headings'),
  };
}

// --------------------------------------------------------------------------
// screencapture + before/after frame diff
// --------------------------------------------------------------------------

function capture(rect: Rect, path: string): void {
  execFileSync('screencapture', ['-x', '-R', `${rect.x},${rect.y},${rect.w},${rect.h}`, path]);
}

/** Capture an arbitrary screen rect to `path` and return the path (no focus/raise
 *  — used only by the oracle fault to inject a deliberately wrong-size frame). */
function captureRect(rect: Rect, path: string): string {
  capture(rect, path);
  return path;
}

/** RAISE + focus the target window, then capture ITS identified bounds. The
 *  raise/focus happens immediately before the grab so a background window or an
 *  occluder over a fixed screen rectangle can't supply false variance — the frame
 *  is always the front-most MDReader window. Returns the file path. */
function captureWindow(pid: number, path: string): string {
  focusWindow(pid); // frontmost + AXRaise of window 1 (asserts the app is alive)
  capture(windowRect(pid), path);
  return path;
}

/** A PNG's inflated (filtered) scanline bytes — the raw pixel-ish stream, filter
 *  bytes included. Used only to compare two frames of the same window. */
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

/** A capture anomaly (blank / wrong-size / undecodable frame) — distinct from a
 *  genuine below-threshold diff. Retried a bounded number of times; if it
 *  persists, the step FAILS. Never silently treated as "fully changed". */
class FrameAnomaly extends Error {}

/** Fraction of sampled bytes that differ between two same-window frames. A size
 *  or format mismatch is NOT "fully changed" — it is a capture anomaly (a blank
 *  or wrong-size grab) and THROWS, so it can never pass the lower bound. This is
 *  a BEFORE/AFTER diff — proof the action changed the pixels — not a single-frame
 *  "is it blank" guess. */
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

/** Perform the AFTER capture, diff it against `before`, and assert a real visible
 *  change (>= FRAME_DIFF_MIN). A capture ANOMALY (size/format mismatch, blank or
 *  wrong-size grab) is retried up to `tries` times, then FAILS — it is never
 *  accepted as change. A genuine below-threshold diff fails immediately. Returns
 *  the change fraction. Secondary sanity check behind the AX oracle. */
async function assertFrameChanged(
  pid: number,
  before: Buffer,
  shotPath: string,
  label: string,
  tries = 4,
): Promise<number> {
  let lastAnomaly = '';
  for (let attempt = 1; attempt <= tries; attempt++) {
    // FAULT: substitute a wrong-size grab (a tiny 8x8 screen rect) for every
    // attempt, exercising the "mismatch → FAIL, not fully-changed" path.
    const after = faulted('frame-oracle')
      ? readFileSync(captureRect({ x: 0, y: 0, w: 8, h: 8 }, shotPath))
      : readFileSync(captureWindow(pid, shotPath));
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
      if (!(err instanceof FrameAnomaly)) throw err; // real below-threshold → fail now
      lastAnomaly = err.message;
      await sleep(400);
    }
  }
  throw new Error(
    `frame oracle could not obtain comparable captures for ${label} after ${tries} tries ` +
      `(last: ${lastAnomaly}) — refusing to treat a blank/wrong-size frame as a change.`,
  );
}

// --------------------------------------------------------------------------
// cgdrag (synthetic mouse) — compiled once from the committed Swift source
// --------------------------------------------------------------------------

function cgDrag(bin: string, x1: number, y1: number, x2: number, y2: number): void {
  execFileSync(bin, ['drag', String(x1), String(y1), String(x2), String(y2)]);
}

function cgClick(bin: string, x: number, y: number): void {
  execFileSync(bin, ['click', String(x), String(y)]);
}

/** One synthetic drag that GRABS at (grabX,grabY) and pulls by (dx,dy). */
function cgDragBy(bin: string, grabX: number, grabY: number, dx: number, dy: number): void {
  cgDrag(bin, grabX, grabY, grabX + dx, grabY + dy);
}

// --------------------------------------------------------------------------
// step interpreter
// --------------------------------------------------------------------------

interface Ctx {
  opts: Options;
  cgdrag: string;
  axBin: string;
  pid: number;
  shot: (name: string) => string;
  /** Absolute path of the workspace-scoped index db for the seeded corpus. */
  indexDb: string;
}

/** Poll `check` until it is true or `timeoutMs` elapses. */
async function waitUntil(
  check: () => boolean | Promise<boolean>,
  timeoutMs: number,
  everyMs = 300,
): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    if (await check()) return true;
    if (Date.now() >= deadline) return false;
    await sleep(everyMs);
  }
}

async function runStep(step: Step, ctx: Ctx): Promise<string> {
  const { pid, opts } = ctx;
  const ax = makeAx(ctx.axBin, pid);
  switch (step.kind) {
    case 'launch': {
      const ok = await waitUntil(() => windowCount(pid) >= 1, 20_000);
      if (!ok) throw new Error('no native window appeared within 20s');
      const shot = ctx.shot('01-launch');
      capture(windowRect(pid), shot);
      return `process ${pid} alive, window present → ${shot}`;
    }

    case 'grantWorkspace': {
      // SAME contract as the WebdriverIO leg: the file tree is populated to
      // >= minFiles rows. Counted from the AX tree (`.filetree__row`), the exact
      // elements `[role="treeitem"]` maps to — not "the screen changed".
      const need = faulted(step.id) ? Number.MAX_SAFE_INTEGER : step.minFiles;
      let rows = 0;
      const ok = await waitUntil(() => {
        rows = ax.count('filetree__row');
        return rows >= need;
      }, 15_000);
      const shot = ctx.shot('02-grant-workspace');
      captureWindow(pid, shot);
      if (!ok) {
        throw new Error(
          `file tree not populated: ${rows} rows < required ${need} — the seeded ` +
            `workspace was not granted/listed.`,
        );
      }
      return `workspace granted — file tree populated with ${rows} rows (>= ${need}) → ${shot}`;
    }

    case 'waitIndexed': {
      // GATE on the status-bar signal — the same regex the WebdriverIO leg uses —
      // read from the status bar's AX subtree. Corroborated by the engine's
      // WORKSPACE-SCOPED index db (a stale db elsewhere cannot satisfy it).
      const re = new RegExp(faulted(step.id) ? 'Indexed 9999999 files' : step.pattern);
      let statusText = '';
      const statusOk = await waitUntil(() => {
        statusText = ax.text('statusbar');
        return re.test(statusText);
      }, FLOW_CONFIG.indexTimeoutMs);
      if (!statusOk) {
        throw new Error(
          `status bar never matched /${re.source}/ within ${FLOW_CONFIG.indexTimeoutMs}ms ` +
            `(last read: ${JSON.stringify(statusText.replace(/\n/g, ' ⏎ ').slice(0, 160))})`,
        );
      }
      if (!existsSync(ctx.indexDb)) {
        throw new Error(
          `status reported indexed but the workspace-scoped index db is absent: ${ctx.indexDb}`,
        );
      }
      const shot = ctx.shot('03-wait-indexed');
      captureWindow(pid, shot);
      const line = statusText.split('\n').find((l) => re.test(l)) ?? statusText;
      return `status bar reads "${line.trim()}"; index db present at the workspace-scoped path → ${shot}`;
    }

    case 'openDoc': {
      // captureWindow raises+focuses the window; that focus also lets the ensuing
      // keystrokes reach the webview.
      const before = readFileSync(captureWindow(pid, ctx.shot('04-open-doc-before')));
      sendChord('Mod+P');
      await sleep(700);
      typeText(step.query);
      await sleep(1300);
      sendChord('Enter');
      // SAME contract as the WebdriverIO leg: a rendered heading matches. On
      // fault, demand a heading the doc does not have.
      const re = new RegExp(faulted(step.id) ? 'No Such Heading 12345' : step.headingPattern);
      let headings = '';
      const ok = await waitUntil(() => {
        headings = ax.headings();
        return re.test(headings);
      }, 15_000);
      if (!ok) {
        throw new Error(
          `opened "${step.query}" but no rendered heading matched /${re.source}/ ` +
            `(headings: ${JSON.stringify(headings.replace(/\n/g, ' | ').slice(0, 160))})`,
        );
      }
      const shot = ctx.shot('04-open-doc');
      const frac = await assertFrameChanged(pid, before, shot, `openDoc "${step.query}"`);
      return `opened "${step.query}" — heading /${step.headingPattern}/ rendered, frame changed ${(frac * 100).toFixed(0)}% → ${shot}`;
    }

    case 'search': {
      const before = readFileSync(captureWindow(pid, ctx.shot('05-search-before')));
      sendChord('Mod+K');
      await sleep(700);
      typeText(step.query);
      // SAME contract as the WebdriverIO leg: >= 1 result row (`.mdsearch__item`,
      // the elements cmdk gives role="option"). On fault, count a class no row
      // carries → 0 rows, standing in for "the search returned nothing".
      const rowClass = faulted(step.id) ? 'mdsearch__item__ABSENT' : 'mdsearch__item';
      let rows = 0;
      const ok = await waitUntil(() => {
        rows = ax.count(rowClass);
        return rows >= 1;
      }, 15_000);
      if (!ok) throw new Error(`Cmd+K "${step.query}" returned no result rows`);
      const shot = ctx.shot('05-search');
      const frac = await assertFrameChanged(pid, before, shot, `search "${step.query}"`);
      return `Cmd+K "${step.query}" → ${rows} result row(s), palette drawn (frame changed ${(frac * 100).toFixed(0)}%) → ${shot}`;
    }

    case 'jumpToHit': {
      const before = readFileSync(captureWindow(pid, ctx.shot('06-jump-to-hit-before')));
      // Faults that each break a DIFFERENT layer of the palette-closure check —
      // all must FAIL the step, never be read as "palette gone":
      //   - "jump-to-hit":       skip Enter → the palette stays open (the app-
      //                          behaviour fault).
      //   - "jump-closure-error": press Enter, but spawn a MISSING axquery binary
      //                          → the TS makeAx path throws (spawn error).
      //   - "jump-ax-read-error": press Enter, but point axquery at an INVALID pid
      //                          (> macOS PID_MAX) so AXUIElementCopyAttributeValue
      //                          returns a real error (kAXErrorCannotComplete) and
      //                          the helper EXITS NONZERO → the in-helper AX-read-
      //                          failure path throws. This is the class a swallowed
      //                          nil-and-print-0 would have masked.
      if (!faulted(step.id)) sendChord('Enter');
      const INVALID_PID = 999_999; // above macOS PID_MAX (99999): never a live app
      const closureAx = faulted('jump-closure-error')
        ? makeAx(`${ctx.axBin}__missing`, pid)
        : faulted('jump-ax-read-error')
          ? makeAx(ctx.axBin, INVALID_PID)
          : ax;
      const headRe = new RegExp(step.headingPattern);
      const provRe = new RegExp(step.provenancePattern);
      let palettePresent = 1;
      let readerText = '';
      // POSITIVE CLOSURE: the search dialog's own container (`.mdsearch`) must be
      // ABSENT — closure is asserted directly, NOT inferred from a row count of 0
      // (which a fail-open zero could fake). Heading AND provenance are read from
      // the reader region only (`.shell__main`, the surfaced ancestor of the
      // generic `.mdreader` scroll container that macOS AX collapses), so the
      // sidebar/status bar can't supply a false match — matching the WebdriverIO
      // leg's scoped `.mdreader` reads. `count` FAILS CLOSED on a helper error.
      const ok = await waitUntil(() => {
        palettePresent = closureAx.count('mdsearch');
        readerText = ax.text('shell__main');
        return palettePresent === 0 && headRe.test(readerText) && provRe.test(readerText);
      }, 15_000);
      if (!ok) {
        throw new Error(
          `jump did not land on the provenance target: palette container ` +
            `${palettePresent === 0 ? 'gone' : 'STILL PRESENT'}, ` +
            `heading /${headRe.source}/ ${headRe.test(readerText) ? 'shown' : 'MISSING'}, ` +
            `provenance line /${provRe.source}/ ${provRe.test(readerText) ? 'shown' : 'MISSING'} ` +
            `(reader region).`,
        );
      }
      const shot = ctx.shot('06-jump-to-hit');
      const frac = await assertFrameChanged(pid, before, shot, 'jumpToHit');
      return `Enter landed on the hit — palette gone, heading /${step.headingPattern}/ + provenance /${step.provenancePattern}/ in reader region (frame changed ${(frac * 100).toFixed(0)}%) → ${shot}`;
    }

    case 'dragWindow': {
      focusWindow(pid);
      // (1) POSITIVE: a genuine drag STARTED ON the titlebar moves the window by
      // ~the delta. Grab the titlebar mid-top, clear of the traffic lights.
      const r0 = windowRect(pid);
      const tbX = r0.x + Math.round(r0.w / 2);
      const tbY = r0.y + 12;
      cgDragBy(ctx.cgdrag, tbX, tbY, step.dx, step.dy);
      await sleep(400);
      const moved = windowRect(pid);
      const posMovedX = moved.x - r0.x;
      const posMovedY = moved.y - r0.y;
      const tol = 20;
      if (Math.abs(posMovedX - step.dx) > tol || Math.abs(posMovedY - step.dy) > tol) {
        throw new Error(
          `titlebar drag did NOT move the window as expected: moved (${posMovedX},${posMovedY}), ` +
            `wanted ~(${step.dx},${step.dy}). A dead drag region looks exactly like this.`,
        );
      }
      // (2) NEGATIVE CONTROL: the SAME drag started in the CONTENT area must NOT
      // move the window (the native titlebar is the only drag surface). On fault,
      // grab the titlebar instead — a whole-window drag region would look like
      // this, and this assertion must catch it.
      const cx = moved.x + Math.round(moved.w / 2);
      const cy = faulted(step.id) ? moved.y + 12 : moved.y + Math.round(moved.h * 0.6);
      cgDragBy(ctx.cgdrag, cx, cy, step.dx, step.dy);
      await sleep(400);
      const settled = windowRect(pid);
      const negMovedX = settled.x - moved.x;
      const negMovedY = settled.y - moved.y;
      const shot = ctx.shot('07-drag-window');
      captureWindow(pid, shot);
      const negTol = 6;
      if (Math.abs(negMovedX) > negTol || Math.abs(negMovedY) > negTol) {
        throw new Error(
          `NEGATIVE CONTROL FAILED: a content-area drag moved the window ` +
            `(${negMovedX},${negMovedY}) — the window is draggable outside the titlebar, so the ` +
            `positive drag proves nothing about titlebar draggability.`,
        );
      }
      return (
        `titlebar drag moved the window Δ(${posMovedX},${posMovedY}) ~ (${step.dx},${step.dy}); ` +
        `content-area drag moved it Δ(${negMovedX},${negMovedY}) ~ (0,0) → ${shot}`
      );
    }

    case 'resizeWindow': {
      focusWindow(pid);
      const before = windowRect(pid);
      // On fault, skip the resize → size unchanged → the assertion must fire.
      if (!faulted(step.id)) resizeWindowTo(pid, before.w + step.dw, before.h + step.dh);
      await sleep(500);
      const after = windowRect(pid);
      const shot = ctx.shot('08-resize-window');
      captureWindow(pid, shot);
      if (after.w === before.w && after.h === before.h) {
        throw new Error(`window did not resize: still ${before.w}x${before.h}`);
      }
      return `window resized ${before.w}x${before.h} → ${after.w}x${after.h} → ${shot}`;
    }

    case 'closeWindow': {
      focusWindow(pid);
      const cb = closeButtonRect(pid);
      // On fault, skip the click → the process survives → the assertion must fire
      // (this is the dead-titlebar / intercepted-close class of bug).
      if (!faulted(step.id)) cgClick(ctx.cgdrag, cb.x + Math.round(cb.w / 2), cb.y + Math.round(cb.h / 2));
      const gone = await waitUntil(() => windowCount(pid) === 0 && !isAlive(pid), 5000);
      if (!gone) {
        throw new Error(
          'red-X click did NOT end the app — the process is still alive. ' +
            'An intercepted/blocked native close looks exactly like this.',
        );
      }
      return `red-X closed the app — process ${pid} is gone`;
    }

    default: {
      const _exhaustive: never = step;
      throw new Error(`unhandled step ${JSON.stringify(_exhaustive)}`);
    }
  }
}

function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

/** True while any `mdreader-app` process is running (a real check, not a swallowed
 *  pkill exit code). */
function anyInstanceRunning(): boolean {
  try {
    const out = execFileSync('pgrep', ['-x', 'mdreader-app'], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    }).trim();
    return out.length > 0;
  } catch {
    // pgrep exits 1 (throws) when nothing matches — that means none running.
    return false;
  }
}

/** Ensure NO stray `mdreader-app` survives before we launch our own — a leftover
 *  instance owns a window/pid and an index that would contaminate this run.
 *  Escalates TERM → KILL and VERIFIES the app is actually gone; refuses to
 *  proceed otherwise (rather than swallowing the pkill exit code). */
async function ensureNoStrayInstance(): Promise<void> {
  if (!anyInstanceRunning()) return;
  try {
    execFileSync('pkill', ['-x', 'mdreader-app']);
  } catch {
    /* pkill exits 1 if the process died between the check and here — fine */
  }
  if (await waitUntil(() => !anyInstanceRunning(), 5000)) return;
  // Still alive after a polite TERM — escalate.
  try {
    execFileSync('pkill', ['-9', '-x', 'mdreader-app']);
  } catch {
    /* raced to exit */
  }
  if (!(await waitUntil(() => !anyInstanceRunning(), 5000))) {
    throw new Error(
      'a prior mdreader-app instance is still running and could not be killed — refusing to ' +
        'proceed (its window/pid/index would contaminate the run).',
    );
  }
}

// --------------------------------------------------------------------------
// main
// --------------------------------------------------------------------------

async function main(): Promise<number> {
  const opts = parseArgs(process.argv.slice(2));

  console.log('── MDReader macOS GUI smoke ───────────────────────────────');
  console.log(`app:       ${opts.app}`);
  console.log(`corpus:    ${opts.corpus}`);
  console.log(`artifacts: ${opts.artifacts}`);
  if (FAULT) console.log(`FAULT INJECTION ACTIVE: "${FAULT}" — expect a RED failure at that step.`);

  if (!existsSync(opts.binary)) {
    console.error(`\nFAIL: app binary not found: ${opts.binary}`);
    console.error('Build it first: pnpm --filter @mdreader/app tauri build --bundles app');
    return 1;
  }
  if (!existsSync(opts.corpus)) {
    console.error(`\nFAIL: fixture corpus not found: ${opts.corpus}`);
    return 1;
  }

  mkdirSync(opts.artifacts, { recursive: true });
  const shot = (name: string): string => join(opts.artifacts, `${name}.png`);

  // Any leftover instance would confuse window/pid lookups AND its stale index
  // could satisfy a laxly-scoped check — verify it is actually gone first.
  await ensureNoStrayInstance();

  // Fresh state: wipe the plugin-store + engine index so the run re-grants and
  // re-indexes from scratch (hash-embed makes this instant).
  await clearSeed(opts.identifier);
  const workspaces = join(appConfigDir(opts.identifier), 'workspaces');
  rmSync(workspaces, { recursive: true, force: true });
  const seedPath = await seedWorkspace(opts.identifier, opts.corpus);
  console.log(`seeded:    ${seedPath}`);

  // The engine writes its index to workspaces/<blake3(canonical root)>/catdb.db.
  // Reproduce that path so waitIndexed's db check is scoped to THIS workspace.
  const canonicalCorpus = realpathSync(opts.corpus);
  const indexDb = join(workspaces, blake3hex(Buffer.from(canonicalCorpus, 'utf8')), 'catdb.db');
  console.log(`index db:  ${indexDb}\n`);

  const cgdrag = buildSwift(opts.artifacts, 'cgdrag');
  const axBin = buildSwift(opts.artifacts, 'axquery');

  // Launch the real binary with the offline hash embedder so indexing is instant
  // and deterministic (no ONNX model download).
  const child = spawn(opts.binary, [], {
    env: { ...process.env, MDREADER_DEV_HASH_EMBED: '1' },
    stdio: 'ignore',
    detached: false,
  });
  const pid = child.pid;
  if (pid === undefined) {
    console.error('FAIL: could not spawn the app');
    return 1;
  }

  const ctx: Ctx = { opts, cgdrag, axBin, pid, shot, indexDb };
  let failedAt: string | null = null;

  try {
    for (const step of FLOW) {
      const t0 = Date.now();
      try {
        const detail = await runStep(step, ctx);
        const ms = Date.now() - t0;
        const tag = step.channel === 'window' ? 'WINDOW' : 'dom';
        console.log(`PASS  [${tag}] ${step.title}`);
        console.log(`        ${detail}  (${ms}ms)`);
      } catch (err) {
        failedAt = step.id;
        console.error(`FAIL  ${step.title}`);
        console.error(`        ${err instanceof Error ? err.message : String(err)}`);
        break;
      }
    }
  } finally {
    // Close-step success already ended the process; otherwise tidy up unless
    // the caller asked to keep the window for inspection.
    if (failedAt !== null && !opts.keep && isAlive(pid)) {
      try {
        process.kill(pid);
      } catch {
        /* already gone */
      }
    }
  }

  const summary = { ok: failedAt === null, failedAt, artifacts: opts.artifacts };
  writeFileSync(join(opts.artifacts, 'result.json'), JSON.stringify(summary, null, 2));

  console.log('───────────────────────────────────────────────────────────');
  if (failedAt === null) {
    console.log('RESULT: PASS — all flow steps green (drag + red-X close asserted).');
    return 0;
  }
  console.log(`RESULT: FAIL at step "${failedAt}".`);
  return 1;
}

main()
  .then((code) => process.exit(code))
  .catch((err) => {
    console.error('smoke driver crashed:', err);
    process.exit(1);
  });
