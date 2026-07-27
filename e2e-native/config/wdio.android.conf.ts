/**
 * Milestone PT — WebdriverIO + Appium config (Android leg, appium-uiautomator2-driver).
 *
 * Runs the shared `FLOW` against an Android emulator, switching into the app's
 * WEBVIEW (WebView_<pkg>) context (see `specs/mobile.e2e.ts`). The Appium server is
 * spawned by `@wdio/appium-service`.
 *
 * `MDREADER_ANDROID_APP` must point to a non-empty built `.apk`; capability
 * construction rejects an absent placeholder before Appium starts.
 */

import { execFileSync } from 'node:child_process';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { androidCapabilitiesFor } from '../capabilities.ts';
import {
  MOBILE_FIXTURE_PORT,
  startMobileFixtureServer,
  type MobileFixtureServer,
} from '../mobile-fixture.ts';

const HERE = dirname(fileURLToPath(import.meta.url));
const E2E_ROOT = resolve(HERE, '..');
let fixture: MobileFixtureServer | undefined;

export const config: WebdriverIO.Config = {
  runner: 'local',
  specs: [join(E2E_ROOT, 'specs', 'mobile.e2e.ts')],
  maxInstances: 1,
  capabilities: [androidCapabilitiesFor()],
  logLevel: 'info',
  bail: 0,
  waitforTimeout: 15_000,
  connectionRetryTimeout: 180_000,
  connectionRetryCount: 3,
  framework: 'mocha',
  reporters: ['spec'],
  mochaOpts: { ui: 'bdd', timeout: 240_000 },
  onPrepare: async () => {
    fixture = await startMobileFixtureServer();
    try {
      execFileSync('adb', [
        'reverse',
        `tcp:${MOBILE_FIXTURE_PORT}`,
        `tcp:${MOBILE_FIXTURE_PORT}`,
      ]);
    } catch (error) {
      await fixture.close();
      fixture = undefined;
      throw error;
    }
  },
  onComplete: async () => {
    try {
      execFileSync('adb', ['reverse', '--remove', `tcp:${MOBILE_FIXTURE_PORT}`]);
    } finally {
      await fixture?.close();
      fixture = undefined;
    }
  },
  services: [['appium', { args: { address: '127.0.0.1', port: 4723 }, command: 'appium' }]],
  port: 4723,
  path: '/',
};
