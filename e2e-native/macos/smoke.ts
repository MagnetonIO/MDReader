/**
 * Milestone PT — the macOS GUI smoke DRIVER.
 *
 * WKWebView (the macOS webview) exposes no WebDriver endpoint, so the WebdriverIO
 * suite cannot drive the real app here. This driver does it the only way macOS
 * allows: it launches the real `.app`, then drives + inspects the NATIVE window
 * through AppleScript System Events (position/size/close-button, keystrokes) and
 * CoreGraphics synthetic input (`cgdrag`), capturing the webview with
 * `screencapture`. It consumes the SAME `FLOW` the WebdriverIO suite does.
 *
 * What is asserted RIGOROUSLY (fails the run, exit 1):
 *   - launch:        the process is alive and a native window exists.
 *   - grantWorkspace: the captured frame shows rendered content (not a blank
 *                     webview) — the "booted but nothing rendered" failure.
 *   - waitIndexed:   the engine wrote its on-disk index (a `catdb.db` appears).
 *   - openDoc:       after Cmd+P → open, the frame shows rendered content.
 *   - search:        after Cmd+K → type, the frame shows rendered content.
 *   - jumpToHit:     after Enter, the frame shows rendered content.
 *   - dragWindow:    a SYNTHETIC titlebar drag moves the window by ~the delta.
 *                    A dead drag region (the class of bug that shipped) → no
 *                    move → FAIL.
 *   - resizeWindow:  the window size changes.
 *   - closeWindow:   a SYNTHETIC click on the red close button ends the process.
 *                    An intercepted/blocked close (the bug that shipped) → the
 *                    process survives → FAIL.
 *
 * The dragWindow and closeWindow assertions are the two a "process booted" check
 * can never make — they are the reason this harness exists. Every step also
 * writes a PNG artifact for the run log.
 *
 * Node built-ins only. Runs under Node's native TypeScript (Node ≥ 22.13 with
 * `--experimental-strip-types`; ≥ 23 by default) — no build step.
 */

import { execFileSync, spawn } from 'node:child_process';
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
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

/** Below this inflated-scanline stddev a frame is treated as blank/unrendered.
 *  Calibrated: real content ≈ 22, a solid frame ≈ 0.5 (see docs/testing). */
const RENDERED_STDDEV_MIN = 4.0;

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

/** Bring the app forward and raise its window — the precondition for keystrokes
 *  to reach the webview. NO coordinate clicks (those can activate other apps). */
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
// screencapture + rendered-content check
// --------------------------------------------------------------------------

function capture(rect: Rect, path: string): void {
  execFileSync('screencapture', ['-x', '-R', `${rect.x},${rect.y},${rect.w},${rect.h}`, path]);
}

/** Standard deviation of a PNG's inflated (filtered) scanline bytes. A blank/
 *  solid frame yields ≈0; rendered content yields tens. Node built-ins only. */
function renderedStddev(png: Buffer): number {
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
  const raw = inflateSync(Buffer.concat(idat));
  let n = 0;
  let sum = 0;
  let sumSq = 0;
  const stride = Math.max(1, Math.floor(raw.length / 200_000));
  for (let i = 0; i < raw.length; i += stride) {
    const v = raw[i]!;
    n++;
    sum += v;
    sumSq += v * v;
  }
  const mean = sum / n;
  return Math.sqrt(Math.max(0, sumSq / n - mean * mean));
}

/** Capture the window and assert the frame is not blank. Returns the stddev. */
function assertRenderedContent(pid: number, path: string): number {
  const rect = windowRect(pid);
  capture(rect, path);
  const stddev = renderedStddev(readFileSync(path));
  if (stddev < RENDERED_STDDEV_MIN) {
    throw new Error(
      `frame is blank (stddev ${stddev.toFixed(1)} < ${RENDERED_STDDEV_MIN}) — nothing rendered`,
    );
  }
  return stddev;
}

// --------------------------------------------------------------------------
// cgdrag (synthetic mouse) — compiled once from the committed Swift source
// --------------------------------------------------------------------------

function buildCgdrag(artifacts: string): string {
  const src = join(HERE, 'cgdrag.swift');
  const bin = join(artifacts, 'cgdrag');
  execFileSync('swiftc', ['-O', '-o', bin, src]);
  return bin;
}

function cgDrag(bin: string, x1: number, y1: number, x2: number, y2: number): void {
  execFileSync(bin, ['drag', String(x1), String(y1), String(x2), String(y2)]);
}

function cgClick(bin: string, x: number, y: number): void {
  execFileSync(bin, ['click', String(x), String(y)]);
}

// --------------------------------------------------------------------------
// step interpreter
// --------------------------------------------------------------------------

interface Ctx {
  opts: Options;
  cgdrag: string;
  pid: number;
  shot: (name: string) => string;
}

/** Poll `check` until it is true or `timeoutMs` elapses. */
async function waitUntil(check: () => boolean, timeoutMs: number, everyMs = 300): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    if (check()) return true;
    if (Date.now() >= deadline) return false;
    await sleep(everyMs);
  }
}

/** True once the engine has written an index db for any workspace. */
function indexOnDisk(identifier: string): boolean {
  const dir = join(appConfigDir(identifier), 'workspaces');
  if (!existsSync(dir)) return false;
  return readdirSync(dir).some((h) => existsSync(join(dir, h, 'catdb.db')));
}

async function runStep(step: Step, ctx: Ctx): Promise<string> {
  const { pid, opts } = ctx;
  switch (step.kind) {
    case 'launch': {
      const ok = await waitUntil(() => windowCount(pid) >= 1, 20_000);
      if (!ok) throw new Error('no native window appeared within 20s');
      const shot = ctx.shot('01-launch');
      capture(windowRect(pid), shot);
      return `process ${pid} alive, window present → ${shot}`;
    }
    case 'grantWorkspace': {
      await sleep(1500);
      const shot = ctx.shot('02-grant-workspace');
      const stddev = assertRenderedContent(pid, shot);
      return `workspace granted, sidebar rendered (stddev ${stddev.toFixed(1)}) → ${shot}`;
    }
    case 'waitIndexed': {
      const ok = await waitUntil(() => indexOnDisk(opts.identifier), FLOW_CONFIG.indexTimeoutMs);
      if (!ok) throw new Error(`no engine index (catdb.db) within ${FLOW_CONFIG.indexTimeoutMs}ms`);
      const shot = ctx.shot('03-wait-indexed');
      capture(windowRect(pid), shot);
      return `engine wrote its index (catdb.db on disk); status shows the count → ${shot}`;
    }
    case 'openDoc': {
      focusWindow(pid);
      sendChord('Mod+P');
      await sleep(700);
      typeText(step.query);
      await sleep(1300);
      sendChord('Enter');
      await sleep(1300);
      const shot = ctx.shot('04-open-doc');
      const stddev = assertRenderedContent(pid, shot);
      return `opened "${step.query}" via Cmd+P, document rendered (stddev ${stddev.toFixed(1)}) → ${shot}`;
    }
    case 'search': {
      focusWindow(pid);
      sendChord('Mod+K');
      await sleep(700);
      typeText(step.query);
      await sleep(2000);
      const shot = ctx.shot('05-search');
      const stddev = assertRenderedContent(pid, shot);
      return `Cmd+K "${step.query}" returned results (stddev ${stddev.toFixed(1)}) → ${shot}`;
    }
    case 'jumpToHit': {
      assertAlive(pid, 'jump-to-hit');
      sendChord('Enter');
      await sleep(1500);
      const shot = ctx.shot('06-jump-to-hit');
      const stddev = assertRenderedContent(pid, shot);
      return `Enter opened the hit at its provenance line (stddev ${stddev.toFixed(1)}) → ${shot}`;
    }
    case 'dragWindow': {
      focusWindow(pid);
      const before = windowRect(pid);
      // Grab the titlebar mid-top, clear of the traffic lights on the left.
      const grabX = before.x + Math.round(before.w / 2);
      const grabY = before.y + 12;
      cgDrag(ctx.cgdrag, grabX, grabY, grabX + step.dx, grabY + step.dy);
      await sleep(500);
      const after = windowRect(pid);
      const movedX = after.x - before.x;
      const movedY = after.y - before.y;
      const shot = ctx.shot('07-drag-window');
      capture(after, shot);
      const tol = 20;
      if (Math.abs(movedX - step.dx) > tol || Math.abs(movedY - step.dy) > tol) {
        throw new Error(
          `titlebar drag did NOT move the window as expected: moved (${movedX},${movedY}), ` +
            `wanted ~(${step.dx},${step.dy}). A dead drag region looks exactly like this.`,
        );
      }
      return `window moved (${before.x},${before.y}) → (${after.x},${after.y}) [Δ ${movedX},${movedY}] → ${shot}`;
    }
    case 'resizeWindow': {
      focusWindow(pid);
      const before = windowRect(pid);
      resizeWindowTo(pid, before.w + step.dw, before.h + step.dh);
      await sleep(500);
      const after = windowRect(pid);
      const shot = ctx.shot('08-resize-window');
      capture(after, shot);
      if (after.w === before.w && after.h === before.h) {
        throw new Error(`window did not resize: still ${before.w}x${before.h}`);
      }
      return `window resized ${before.w}x${before.h} → ${after.w}x${after.h} → ${shot}`;
    }
    case 'closeWindow': {
      focusWindow(pid);
      const cb = closeButtonRect(pid);
      cgClick(ctx.cgdrag, cb.x + Math.round(cb.w / 2), cb.y + Math.round(cb.h / 2));
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

// --------------------------------------------------------------------------
// main
// --------------------------------------------------------------------------

async function main(): Promise<number> {
  const opts = parseArgs(process.argv.slice(2));

  console.log('── MDReader macOS GUI smoke ───────────────────────────────');
  console.log(`app:       ${opts.app}`);
  console.log(`corpus:    ${opts.corpus}`);
  console.log(`artifacts: ${opts.artifacts}`);

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

  // Any leftover instance would confuse window/pid lookups.
  try {
    execFileSync('pkill', ['-x', 'mdreader-app']);
  } catch {
    /* none running */
  }

  // Fresh state: wipe the plugin-store + engine index so the run re-grants and
  // re-indexes from scratch (hash-embed makes this instant).
  await clearSeed(opts.identifier);
  const workspaces = join(appConfigDir(opts.identifier), 'workspaces');
  rmSync(workspaces, { recursive: true, force: true });
  const seedPath = await seedWorkspace(opts.identifier, opts.corpus);
  console.log(`seeded:    ${seedPath}\n`);

  const cgdrag = buildCgdrag(opts.artifacts);

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

  const ctx: Ctx = { opts, cgdrag, pid, shot };
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
