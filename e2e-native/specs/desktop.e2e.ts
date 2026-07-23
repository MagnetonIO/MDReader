/**
 * Milestone PT — the desktop GUI suite, driven by WebdriverIO + tauri-driver
 * against the REAL built app on Linux (WebKitGTK) and Windows (WebView2).
 *
 * It walks the SAME `FLOW` the macOS smoke driver walks (`../flows.ts`), so the
 * two harnesses can never diverge on what "working" means. Each webview
 * (`channel: 'dom'`) step is asserted rigorously against the live DOM. The
 * native-window (`channel: 'window'`) steps — drag / resize / red-X close —
 * are registered as PENDING here with an explicit reason: a W3C WebDriver
 * session drives the webview document, not the OS window chrome, so those
 * assertions live in the macOS AppleScript driver (`../macos/smoke.ts`), which
 * CAN move and close the real window.
 *
 * STATUS: CI-validated-pending — authored + wired into tauri.yml, not yet run on
 * a machine with tauri-driver (the macOS dev box cannot run it).
 */

import { browser, $, $$, expect } from '@wdio/globals';
import { Key } from 'webdriverio';

import { DOM_STEPS, SELECTORS, WINDOW_STEPS, type Step } from '../flows.ts';

/** The primary modifier the app listens for off-macOS (detectMac === false). */
const MOD = Key.Ctrl;

/** Map a shell chord to a WebdriverIO key sequence. */
function chordKeys(chord: string): string[] {
  if (chord === 'Enter') return [Key.Enter];
  if (chord === 'Escape') return [Key.Escape];
  const parts = chord.split('+');
  const key = parts[parts.length - 1]!.toLowerCase();
  return parts.includes('Mod') ? [MOD, key] : [key];
}

async function press(chord: string): Promise<void> {
  await browser.keys(chordKeys(chord));
}

describe('MDReader desktop — core flow (WebdriverIO + tauri-driver)', () => {
  it('renders the shell once the app has launched', async () => {
    // The workspace was seeded (see wdio.conf onPrepare), so the shell mounts
    // straight into the granted corpus.
    await $(SELECTORS.workspaceTree.css!).waitForExist({ timeout: 30_000 });
  });

  for (const step of DOM_STEPS) {
    it(step.title, async () => {
      await runDomStep(step);
    });
  }

  // The native-window assertions cannot be made through a webview WebDriver
  // session — they are covered by the macOS smoke driver. Register them as
  // pending so the report shows the coverage boundary explicitly.
  for (const step of WINDOW_STEPS) {
    it.skip(
      `${step.title} — asserted by the macOS smoke driver (native window not drivable via WebDriver)`,
      () => undefined,
    );
  }
});

async function runDomStep(step: Step): Promise<void> {
  switch (step.kind) {
    case 'grantWorkspace': {
      await $(SELECTORS.workspaceTree.css!).waitForDisplayed({ timeout: 30_000 });
      const rows = await $$(SELECTORS.treeItem.css!);
      expect(rows.length).toBeGreaterThanOrEqual(step.minFiles);
      return;
    }
    case 'waitIndexed': {
      const status = $(SELECTORS.statusBar.css!);
      const re = new RegExp(step.pattern);
      await browser.waitUntil(async () => re.test(await status.getText()), {
        timeout: 30_000,
        timeoutMsg: `status bar never matched /${step.pattern}/`,
      });
      return;
    }
    case 'openDoc': {
      await press('Mod+P');
      await $(SELECTORS.paletteInput.css!).waitForDisplayed();
      await browser.keys(step.query.split(''));
      await press('Enter');
      const reader = $(SELECTORS.reader.css!);
      await reader.waitForExist({ timeout: 15_000 });
      const re = new RegExp(step.headingPattern);
      await browser.waitUntil(async () => re.test(await $(SELECTORS.reader.css!).getText()), {
        timeout: 15_000,
        timeoutMsg: `opened document never showed a heading matching /${step.headingPattern}/`,
      });
      return;
    }
    case 'search': {
      await press('Mod+K');
      await $(SELECTORS.paletteInput.css!).waitForDisplayed();
      await browser.keys(step.query.split(''));
      // At least one hit must come back.
      await browser.waitUntil(async () => (await $$(SELECTORS.paletteOption.css!)).length > 0, {
        timeout: 15_000,
        timeoutMsg: `Cmd+K "${step.query}" returned no hits`,
      });
      return;
    }
    case 'jumpToHit': {
      await press('Enter');
      // The palette closes and the hit's document renders at its provenance line.
      await $(SELECTORS.paletteDialog.css!).waitForExist({ reverse: true, timeout: 15_000 });
      const blocks = $(SELECTORS.readerBlock.css!);
      await blocks.waitForExist({ timeout: 15_000 });
      return;
    }
    default:
      throw new Error(`runDomStep received a non-dom step: ${step.kind}`);
  }
}
