/**
 * Milestone PT — WebdriverIO + Appium config (Android leg, appium-uiautomator2-driver).
 *
 * Runs the shared `FLOW` against an Android emulator, switching into the app's
 * WEBVIEW (WebView_<pkg>) context (see `specs/mobile.e2e.ts`). The Appium server is
 * spawned by `@wdio/appium-service`.
 *
 * BLOCKED on P6: there is no Android app build yet. Point `MDREADER_ANDROID_APP` at
 * the built `.apk` once the P6 mobile shell lands and this leg runs unchanged. The
 * uiautomator2 driver must be installed: `appium driver install uiautomator2`.
 * CI-wired (see tauri.yml).
 */

import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { androidCapabilities } from '../capabilities.ts';

const HERE = dirname(fileURLToPath(import.meta.url));
const E2E_ROOT = resolve(HERE, '..');

export const config: WebdriverIO.Config = {
  runner: 'local',
  specs: [join(E2E_ROOT, 'specs', 'mobile.e2e.ts')],
  maxInstances: 1,
  capabilities: [androidCapabilities],
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
