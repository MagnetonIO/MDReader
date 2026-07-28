/**
 * Milestone PT — WebdriverIO + Appium config (iOS leg, appium-xcuitest-driver).
 *
 * Runs the shared `FLOW` against the iOS Simulator, switching into the app's
 * WEBVIEW context (see `specs/mobile.e2e.ts`). The Appium server is spawned by
 * `@wdio/appium-service` so this is one self-contained `wdio run`.
 *
 * `MDREADER_IOS_APP` must point to a built simulator `.app`; capability
 * construction rejects missing, malformed, production-identity, or empty
 * bundles before Appium starts.
 */

import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { iosCapabilitiesFor } from '../capabilities.ts';
import { startMobileFixtureServer, type MobileFixtureServer } from '../mobile-fixture.ts';

const HERE = dirname(fileURLToPath(import.meta.url));
const E2E_ROOT = resolve(HERE, '..');
let fixture: MobileFixtureServer | undefined;

export const config: WebdriverIO.Config = {
  runner: 'local',
  specs: [join(E2E_ROOT, 'specs', 'mobile.e2e.ts')],
  maxInstances: 1,
  capabilities: [iosCapabilitiesFor()],
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
    process.env.MDREADER_E2E_CONNECTED_ORIGIN = fixture.origin;
  },
  onComplete: async () => {
    await fixture?.close();
    fixture = undefined;
    delete process.env.MDREADER_E2E_CONNECTED_ORIGIN;
  },
  // Spawn a local Appium server. The pinned xcuitest driver is installed with
  // this package's ordinary dependencies.
  services: [['appium', { args: { address: '127.0.0.1', port: 4723 }, command: 'appium' }]],
  port: 4723,
  path: '/',
};
