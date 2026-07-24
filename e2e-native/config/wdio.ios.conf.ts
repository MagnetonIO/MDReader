/**
 * Milestone PT — WebdriverIO + Appium config (iOS leg, appium-xcuitest-driver).
 *
 * Runs the shared `FLOW` against the iOS Simulator, switching into the app's
 * WEBVIEW context (see `specs/mobile.e2e.ts`). The Appium server is spawned by
 * `@wdio/appium-service` so this is one self-contained `wdio run`.
 *
 * BLOCKED on P6: there is no iOS app build yet. Point `MDREADER_IOS_APP` at the
 * built simulator `.app` (or `MDREADER_IOS_BUNDLE_ID` at an installed build) once
 * the P6 mobile shell lands and this leg runs unchanged. CI-wired (see tauri.yml).
 */

import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { iosCapabilities } from '../capabilities.ts';

const HERE = dirname(fileURLToPath(import.meta.url));
const E2E_ROOT = resolve(HERE, '..');

export const config: WebdriverIO.Config = {
  runner: 'local',
  specs: [join(E2E_ROOT, 'specs', 'mobile.e2e.ts')],
  maxInstances: 1,
  capabilities: [iosCapabilities],
  logLevel: 'info',
  bail: 0,
  waitforTimeout: 15_000,
  connectionRetryTimeout: 180_000,
  connectionRetryCount: 3,
  framework: 'mocha',
  reporters: ['spec'],
  mochaOpts: { ui: 'bdd', timeout: 240_000 },
  // Spawn a local Appium server for the session. The xcuitest driver must be
  // installed: `appium driver install xcuitest`.
  services: [['appium', { args: { address: '127.0.0.1', port: 4723 }, command: 'appium' }]],
  port: 4723,
  path: '/',
};
