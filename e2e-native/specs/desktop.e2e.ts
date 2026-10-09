/**
 * Milestone PT — the desktop leg of the UNIFIED harness (WebdriverIO + tauri-driver).
 *
 * Drives the SAME shared `FLOW` the macOS mac2 leg drives, but off the real app's
 * webview DOM on Linux (WebKitGTK) and Windows (WebView2) via `tauri-driver`. The
 * webview-channel steps are asserted rigorously against the live DOM; the native-
 * window steps (drag / resize / red-X close) are registered PENDING here — a
 * webview WebDriver session drives the document, not the OS chrome, so those live
 * in the macOS mac2 leg.
 *
 * STATUS: authored + wired into CI (tauri.yml, ubuntu + windows), CI-validated-
 * pending — the macOS dev box cannot run tauri-driver.
 */

import { browser, expect } from '@wdio/globals';

import { WINDOW_STEPS } from '../flows.ts';
import { runFlow } from '../drivers/runner.ts';
import { desktopWorkerWitness } from '../desktop-fixture.ts';
import { WebviewAdapter } from '../drivers/webview-adapter.ts';

describe('MDReader desktop — unified FLOW over the webview DOM (tauri-driver)', () => {
  it('drives grant → text index → open → search → exact native receipt against the live DOM', async () => {
    const adapter = new WebviewAdapter({
      browser,
      name: `${process.platform} desktop (tauri-driver webview)`,
      windowPendingReason:
        'a webview WebDriver session drives the DOM, not the OS window chrome; asserted by the macOS mac2 leg',
    });
    const result = await runFlow(adapter, undefined, desktopWorkerWitness());
    // The runner logs each step PASS/PEND; surface the first failure clearly.
    if (!result.ok) throw new Error(`unified FLOW failed at step "${result.failedAt}"`);
    expect(result.ok).toBe(true);
  });

  // Make the coverage boundary explicit in the report: the native-window steps are
  // the macOS mac2 leg's rigorous domain, pending on a webview session.
  for (const step of WINDOW_STEPS) {
    if (step.kind === 'launch') continue;
    it.skip(
      `${step.title} — asserted by the macOS mac2 leg (native window not drivable via a webview WebDriver session)`,
      () => undefined,
    );
  }
});
