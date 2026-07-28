/**
 * Milestone PT — WebdriverIO + tauri-driver config (Linux/Windows desktop leg).
 *
 * Drives the REAL built MDReader app through `tauri-driver`, which bridges W3C
 * WebDriver to the platform webview driver:
 *   - Linux:   WebKitWebDriver (WebKitGTK / `libwebkit2gtk-4.1`)
 *   - Windows: msedgedriver (WebView2)
 * macOS is intentionally absent: WKWebView ships no WebDriver, so the Mac path is
 * the appium-mac2-driver leg (`runners/macos.ts`).
 *
 * The spec consumes the SAME shared `FLOW` the macOS leg does, so the two harnesses
 * can never describe different behaviour. CI-validated-pending (the macOS dev box
 * cannot run tauri-driver).
 *
 * The app is launched with `MDREADER_DEV_HASH_EMBED=1` (offline, instant index)
 * and a pre-seeded `session.json` granting the fixture corpus (no OS folder-picker).
 */

import { spawn, type ChildProcess } from 'node:child_process';
import { realpath } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { FLOW_CONFIG } from '../flows.ts';
import { desktopCapabilitiesFor } from '../capabilities.ts';
import {
  clearSeed,
  clearWorkspaceIndex,
  createE2EProfile,
  seedWorkspace,
} from '../seed.ts';

const HERE = dirname(fileURLToPath(import.meta.url));
const E2E_ROOT = resolve(HERE, '..');
const REPO_ROOT = resolve(E2E_ROOT, '..', '..', '..');
const CORPUS = join(REPO_ROOT, FLOW_CONFIG.corpusRelPath);
const E2E_PROFILE = createE2EProfile();

let tauriDriver: ChildProcess | undefined;

export const config: WebdriverIO.Config = {
  runner: 'local',
  hostname: '127.0.0.1',
  port: 4444,
  path: '/',
  specs: [join(E2E_ROOT, 'specs', 'desktop.e2e.ts')],
  maxInstances: 1,
  capabilities: [desktopCapabilitiesFor()],
  logLevel: 'info',
  bail: 0,
  waitforTimeout: 15_000,
  connectionRetryTimeout: 120_000,
  connectionRetryCount: 3,
  framework: 'mocha',
  reporters: ['spec'],
  mochaOpts: { ui: 'bdd', timeout: 180_000 },

  // Seed a restore request and pass the same canonical path to the isolated
  // backend, which captures the filesystem identity before reconciliation.
  onPrepare: async () => {
    const corpusRoot = await realpath(CORPUS);
    await clearSeed(E2E_PROFILE);
    await clearWorkspaceIndex(E2E_PROFILE);
    process.env.MDREADER_E2E_CORPUS_ROOT = corpusRoot;
    await seedWorkspace(E2E_PROFILE, corpusRoot);
  },

  // tauri-driver must be running before the WebDriver session connects. It inherits
  // our env, so the app it launches sees MDREADER_DEV_HASH_EMBED.
  beforeSession: () => {
    tauriDriver = spawn('tauri-driver', [], {
      stdio: [null, process.stdout, process.stderr],
      env: { ...process.env, MDREADER_DEV_HASH_EMBED: '1' },
    });
  },

  afterSession: () => {
    tauriDriver?.kill();
  },
};
