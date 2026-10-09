/**
 * Milestone PT — WebdriverIO + Appium config (Android leg, appium-uiautomator2-driver).
 *
 * Runs device-owned reader acceptance against an Android emulator, switching into the app's
 * WEBVIEW (WebView_<pkg>) context (see `specs/mobile.e2e.ts`). The Appium server is
 * spawned by `@wdio/appium-service`.
 *
 * `MDREADER_ANDROID_UDID` selects the exact owned emulator;
 * `MDREADER_ANDROID_APP` must point to a non-empty built `.apk`; capability
 * construction rejects an absent placeholder before Appium starts.
 */

import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { androidCapabilitiesFor } from '../capabilities.ts';

const HERE = dirname(fileURLToPath(import.meta.url));
const E2E_ROOT = resolve(HERE, '..');

export const config: WebdriverIO.Config = {
  runner: 'local',
  specs: [join(E2E_ROOT, 'specs', 'mobile.e2e.ts')],
  maxInstances: 1,
  capabilities: [await androidCapabilitiesFor()],
  logLevel: 'info',
  bail: 0,
  waitforTimeout: 15_000,
  connectionRetryTimeout: 180_000,
  connectionRetryCount: 3,
  framework: 'mocha',
  reporters: ['spec'],
  mochaOpts: { ui: 'bdd', timeout: 240_000 },
  services: [['appium', { args: { address: '127.0.0.1', port: 4723 }, command: 'appium' }]],
  port: 4723,
  path: '/',
};
