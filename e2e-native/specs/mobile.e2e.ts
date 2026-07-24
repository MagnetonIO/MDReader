/**
 * Milestone PT — the mobile leg of the UNIFIED harness (Appium: iOS + Android).
 *
 * Drives the SAME shared `FLOW` off the mobile app's WEBVIEW context: iOS via
 * appium-xcuitest-driver (Simulator), Android via appium-uiautomator2-driver
 * (emulator). The adapter switches into the WEBVIEW context, then reads the same
 * DOM class names the desktop legs read — one behavioural spec, phone to laptop.
 *
 * BLOCKED on P6: the app currently has NO iOS/Android build (no mobile app shell),
 * so there is nothing for Appium to install/launch yet. This spec + its capability
 * sets + CI wiring are ready so the leg RUNS the moment the P6 shell lands — at
 * which point the only missing pieces are the built `.app`/`.apk` (supplied via
 * the `MDREADER_IOS_APP` / `MDREADER_ANDROID_APP` env the capabilities read).
 *
 * The native-window steps are inherently N/A on mobile (no draggable OS window),
 * so they register PENDING with a mobile-specific reason.
 */

import { browser, expect } from '@wdio/globals';

import { WINDOW_STEPS } from '../flows.ts';
import { runFlow } from '../drivers/runner.ts';
import { WebviewAdapter } from '../drivers/webview-adapter.ts';

/** Switch the Appium session into the app's WEBVIEW context (the HTML UI). */
async function switchToWebview(b: WebdriverIO.Browser): Promise<void> {
  // Poll: the webview context can take a moment to appear after launch.
  const deadline = Date.now() + 30_000;
  for (;;) {
    const contexts = (await b.getContexts()) as unknown as string[];
    const webview = contexts.find((c) => String(c).toUpperCase().includes('WEBVIEW'));
    if (webview) {
      await b.switchContext(webview);
      return;
    }
    if (Date.now() >= deadline) {
      throw new Error(`no WEBVIEW context appeared (contexts: ${JSON.stringify(contexts)})`);
    }
    await new Promise((r) => setTimeout(r, 500));
  }
}

describe('MDReader mobile — unified FLOW over the webview context (Appium)', () => {
  it('drives grant → index → open → search → provenance-jump in the WEBVIEW context', async () => {
    const adapter = new WebviewAdapter({
      browser,
      name: `${(browser.capabilities as Record<string, unknown>)['platformName'] ?? 'mobile'} (Appium webview)`,
      windowPendingReason: 'mobile has no draggable OS window; the native-window steps are desktop-only',
      activate: switchToWebview,
    });
    const result = await runFlow(adapter);
    if (!result.ok) throw new Error(`unified FLOW failed at step "${result.failedAt}"`);
    expect(result.ok).toBe(true);
  });

  for (const step of WINDOW_STEPS) {
    if (step.kind === 'launch') continue;
    it.skip(`${step.title} — N/A on mobile (no draggable OS window)`, () => undefined);
  }
});
