/**
 * Milestone PT — the UNIFIED flow interpreter.
 *
 * This is the one place the shared `FLOW` (`../flows.ts`) is turned into actions
 * and assertions. It calls ONLY the {@link PlatformDriver} contract, so the exact
 * same interpreter drives macOS (appium-mac2-driver + AX reads), Windows/Linux
 * (their Appium/tauri-driver webview sessions), and iOS/Android (Appium webview
 * contexts). What "working" means lives here once, not once per platform.
 *
 * It preserves the two disciplines the milestone was built on:
 *
 *   - FAULT INJECTION (RED proof). With `MDREADER_SMOKE_FAULT=<step-id>` set, the
 *     BEHAVIOURAL fault for that step breaks what the step exercises (require an
 *     impossible row count, demand a heading the doc lacks, skip the Enter that
 *     jumps, grab the titlebar on the negative-control drag, skip the resize/close)
 *     so the step must go RED exactly there. The driver layer owns the READ-PATH
 *     faults (a broken AX read must throw, a wrong-size frame must fail) — see the
 *     mac2 adapter. Together they prove every assertion is load-bearing.
 *   - FAIL CLOSED. Reads come back through the driver, which throws on a genuine
 *     read error; the interpreter never coerces a broken read into "count 0 /
 *     absent". Positive closure in `jumpToHit` asserts the palette CONTAINER is
 *     gone, never infers it from a zero row count.
 *
 * Node built-ins only.
 */

import { FLOW, FLOW_CONFIG, type Chord, type Step } from '../flows.ts';
import { faulted, type FlowResult, type PlatformDriver, type StepResult } from './types.ts';

/** A minimal logger the runner writes progress to (console by default). */
export interface RunLog {
  pass(line: string): void;
  detail(line: string): void;
  fail(title: string, message: string): void;
  pending(line: string): void;
}

export const consoleLog: RunLog = {
  pass: (l) => console.log(`PASS  ${l}`),
  detail: (l) => console.log(`        ${l}`),
  fail: (t, m) => {
    console.error(`FAIL  ${t}`);
    console.error(`        ${m}`);
  },
  pending: (l) => console.log(`PEND  ${l}`),
};

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

/** Poll `check` until true or `timeoutMs` elapses. */
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

/** Highest safe integer, used as the "impossible" row count for the grant fault. */
const IMPOSSIBLE = Number.MAX_SAFE_INTEGER;

/**
 * Run the whole shared FLOW against `driver`. Window-channel steps a driver can't
 * assert (webview-only sessions) are registered PENDING with the driver's reason;
 * everything else is asserted rigorously. Stops at the first failure.
 */
export async function runFlow(driver: PlatformDriver, log: RunLog = consoleLog): Promise<FlowResult> {
  const results: StepResult[] = [];
  let failedAt: string | null = null;

  for (const step of FLOW) {
    const t0 = Date.now();

    // A webview-only driver cannot drive the OS chrome. `launch` still has a
    // webview signal (the shell mounted), so only the true native-window steps
    // (drag/resize/close) go pending.
    if (step.channel === 'window' && step.kind !== 'launch' && !driver.windowOps) {
      const detail = driver.windowPendingReason ?? 'native window not drivable via this session';
      results.push({ id: step.id, title: step.title, channel: step.channel, status: 'pending', detail, ms: 0 });
      log.pending(`[window] ${step.title} — ${detail}`);
      continue;
    }

    try {
      const detail = await runStep(step, driver);
      const ms = Date.now() - t0;
      results.push({ id: step.id, title: step.title, channel: step.channel, status: 'pass', detail, ms });
      log.pass(`[${step.channel === 'window' ? 'WINDOW' : 'dom'}] ${step.title}`);
      log.detail(`${detail}  (${ms}ms)`);
    } catch (err) {
      const ms = Date.now() - t0;
      const message = err instanceof Error ? err.message : String(err);
      results.push({ id: step.id, title: step.title, channel: step.channel, status: 'fail', detail: message, ms });
      log.fail(step.title, message);
      failedAt = step.id;
      break;
    }
  }

  return { ok: failedAt === null, failedAt, results };
}

/** Drive + assert one step. Throws on failure (the runner turns that into FAIL). */
async function runStep(step: Step, d: PlatformDriver): Promise<string> {
  switch (step.kind) {
    case 'launch': {
      // The app is already launched by the harness entry (seeded workspace); the
      // step asserts it actually came up. Native-capable driver → a real window
      // exists; webview-only driver → the shell mounted.
      const check = d.windowOps ? () => d.isWindowPresent() : () => d.shellMounted();
      const ok = await waitUntil(check, 20_000);
      if (!ok) throw new Error('the app did not come up within 20s (no window / shell never mounted)');
      return d.windowOps ? 'native window present, process alive' : 'app shell mounted';
    }

    case 'grantWorkspace': {
      // Same contract on every leg: the file tree is populated to >= minFiles
      // rows (counted from the class the WebdriverIO `[role=treeitem]` selector
      // and the AX `AXDOMClassList` both resolve to). FAULT: demand an impossible
      // count so the step goes RED here.
      const need = faulted(step.id) ? IMPOSSIBLE : step.minFiles;
      let rows = 0;
      const ok = await waitUntil(async () => {
        rows = await d.countClass('filetree__row');
        return rows >= need;
      }, 15_000);
      if (!ok) {
        throw new Error(
          `file tree not populated: ${rows} rows < required ${need} — the seeded workspace was not granted/listed.`,
        );
      }
      return `workspace granted — file tree populated with ${rows} rows (>= ${need})`;
    }

    case 'waitIndexed': {
      // Gate on the status-bar "Indexed N files" signal — the SAME regex every
      // leg uses. FAULT: a pattern the real status can never match.
      const re = new RegExp(faulted(step.id) ? 'Indexed 9999999 files' : step.pattern);
      let statusText = '';
      const ok = await waitUntil(async () => {
        statusText = await d.statusText();
        return re.test(statusText);
      }, FLOW_CONFIG.indexTimeoutMs);
      if (!ok) {
        throw new Error(
          `status bar never matched /${re.source}/ within ${FLOW_CONFIG.indexTimeoutMs}ms ` +
            `(last read: ${JSON.stringify(statusText.replace(/\n/g, ' ⏎ ').slice(0, 160))})`,
        );
      }
      const line = statusText.split('\n').find((l) => re.test(l)) ?? statusText;
      return `status bar reads "${line.trim()}"`;
    }

    case 'openDoc': {
      const before = d.frameOracle && d.captureFrame ? await d.captureFrame() : null;
      await d.focus();
      await d.chord('Mod+P');
      await sleep(700);
      await d.type(step.query);
      await sleep(1200);
      await d.chord('Enter');
      // Same contract as every leg: a rendered heading matches. FAULT: demand a
      // heading the doc does not have.
      const re = new RegExp(faulted(step.id) ? 'No Such Heading 12345' : step.headingPattern);
      let headings = '';
      const ok = await waitUntil(async () => {
        headings = await d.headings();
        return re.test(headings);
      }, 15_000);
      if (!ok) {
        throw new Error(
          `opened "${step.query}" but no rendered heading matched /${re.source}/ ` +
            `(headings: ${JSON.stringify(headings.replace(/\n/g, ' | ').slice(0, 160))})`,
        );
      }
      const frac = before && d.assertFrameChanged ? await d.assertFrameChanged(before, `openDoc "${step.query}"`) : null;
      return `opened "${step.query}" — heading /${step.headingPattern}/ rendered${frameNote(frac)}`;
    }

    case 'search': {
      const before = d.frameOracle && d.captureFrame ? await d.captureFrame() : null;
      await d.focus();
      await d.chord('Mod+K');
      await sleep(700);
      await d.type(step.query);
      // Same contract: >= 1 result row. FAULT: count a class no row carries → 0
      // rows, standing in for "the search returned nothing".
      const rowClass = faulted(step.id) ? 'mdsearch__item__ABSENT' : 'mdsearch__item';
      let rows = 0;
      const ok = await waitUntil(async () => {
        rows = await d.countClass(rowClass);
        return rows >= 1;
      }, 15_000);
      if (!ok) throw new Error(`Cmd+K "${step.query}" returned no result rows`);
      const frac = before && d.assertFrameChanged ? await d.assertFrameChanged(before, `search "${step.query}"`) : null;
      return `Cmd+K "${step.query}" → ${rows} result row(s), palette drawn${frameNote(frac)}`;
    }

    case 'jumpToHit': {
      const before = d.frameOracle && d.captureFrame ? await d.captureFrame() : null;
      // FAULT "jump-to-hit": skip the Enter that jumps → the palette stays open →
      // closure assertion fires. (The mac2 adapter owns the READ-PATH faults
      // jump-closure-error / jump-ax-read-error inside paletteContainerCount.)
      if (!faulted(step.id)) await d.chord('Enter');
      const headRe = new RegExp(step.headingPattern);
      const provRe = new RegExp(step.provenancePattern);
      let palettePresent = 1;
      let readerText = '';
      // POSITIVE CLOSURE: the search dialog's own container must be ABSENT — read
      // directly, never inferred from a result-row count of 0. Heading AND the
      // provenance line (text unique to the target doc) must be in the reader
      // region only. `paletteContainerCount` fails closed on a read error.
      const ok = await waitUntil(async () => {
        palettePresent = await d.paletteContainerCount();
        readerText = await d.readerText();
        return palettePresent === 0 && headRe.test(readerText) && provRe.test(readerText);
      }, 15_000);
      if (!ok) {
        throw new Error(
          `jump did not land on the provenance target: palette container ` +
            `${palettePresent === 0 ? 'gone' : 'STILL PRESENT'}, ` +
            `heading /${headRe.source}/ ${headRe.test(readerText) ? 'shown' : 'MISSING'}, ` +
            `provenance line /${provRe.source}/ ${provRe.test(readerText) ? 'shown' : 'MISSING'} (reader region).`,
        );
      }
      const frac = before && d.assertFrameChanged ? await d.assertFrameChanged(before, 'jumpToHit') : null;
      return `Enter landed on the hit — palette gone, heading /${step.headingPattern}/ + provenance /${step.provenancePattern}/ in reader${frameNote(frac)}`;
    }

    case 'dragWindow': {
      await d.focus();
      // (1) POSITIVE: a genuine drag started ON the titlebar moves the window by
      // ~the delta.
      const r0 = await d.windowRect();
      await d.dragBy('titlebar', step.dx, step.dy);
      await sleep(400);
      const moved = await d.windowRect();
      const posX = moved.x - r0.x;
      const posY = moved.y - r0.y;
      const tol = 20;
      if (Math.abs(posX - step.dx) > tol || Math.abs(posY - step.dy) > tol) {
        throw new Error(
          `titlebar drag did NOT move the window as expected: moved (${posX},${posY}), ` +
            `wanted ~(${step.dx},${step.dy}). A dead drag region looks exactly like this.`,
        );
      }
      // (2) NEGATIVE CONTROL: the same drag started in the CONTENT area must NOT
      // move the window. FAULT "drag-window": grab the titlebar instead — a
      // whole-window drag region would look like this, and this must catch it.
      const negOrigin = faulted(step.id) ? 'titlebar' : 'content';
      await d.dragBy(negOrigin, step.dx, step.dy);
      await sleep(400);
      const settled = await d.windowRect();
      const negX = settled.x - moved.x;
      const negY = settled.y - moved.y;
      const negTol = 6;
      if (Math.abs(negX) > negTol || Math.abs(negY) > negTol) {
        throw new Error(
          `NEGATIVE CONTROL FAILED: a ${negOrigin}-area drag moved the window (${negX},${negY}) — the ` +
            `window is draggable outside the titlebar, so the positive drag proves nothing.`,
        );
      }
      return `titlebar drag moved the window Δ(${posX},${posY}) ~ (${step.dx},${step.dy}); content-area drag moved it Δ(${negX},${negY}) ~ (0,0)`;
    }

    case 'resizeWindow': {
      await d.focus();
      const before = await d.windowRect();
      // FAULT "resize-window": skip the resize → size unchanged → assertion fires.
      const after = faulted(step.id) ? before : await d.resizeBy(step.dw, step.dh);
      if (after.w === before.w && after.h === before.h) {
        throw new Error(`window did not resize: still ${before.w}x${before.h}`);
      }
      return `window resized ${before.w}x${before.h} → ${after.w}x${after.h}`;
    }

    case 'closeWindow': {
      await d.focus();
      // FAULT "close-window": skip the click → the process survives → assertion
      // fires (the intercepted/blocked-close class of bug).
      if (!faulted(step.id)) await d.clickClose();
      // Check process death FIRST and short-circuit: once the app is gone there is
      // no live window to query (a query against a dying app would just stall the
      // WebDriver session before erroring).
      const gone = await waitUntil(async () => !(await d.isProcessAlive()) && !(await d.isWindowPresent()), 8000);
      if (!gone) {
        throw new Error(
          'red-X click did NOT end the app — the process is still alive. ' +
            'An intercepted/blocked native close looks exactly like this.',
        );
      }
      return 'red-X closed the app — the process is gone';
    }

    default: {
      const _exhaustive: never = step;
      throw new Error(`unhandled step ${JSON.stringify(_exhaustive)}`);
    }
  }
}

/** Format the optional frame-oracle corroboration for a step's detail line. */
function frameNote(frac: number | null): string {
  return frac === null ? '' : `, frame changed ${(frac * 100).toFixed(0)}%`;
}

/** Re-exported so entry points can translate a `Chord` if they need to. */
export type { Chord };
