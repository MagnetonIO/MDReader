/**
 * Milestone PT — WebdriverIO + tauri-driver config for the desktop GUI suite.
 *
 * This drives the REAL built MDReader app through `tauri-driver`, which bridges
 * W3C WebDriver to the platform webview driver:
 *   - Linux:   WebKitWebDriver (WebKitGTK / `libwebkit2gtk-4.1`)
 *   - Windows: msedgedriver (WebView2)
 * macOS is intentionally absent: WKWebView ships no WebDriver, so the Mac path
 * is the AppleScript smoke driver (`macos/smoke.ts`) instead.
 *
 * HONEST STATUS: this suite is authored + wired into CI (tauri.yml, ubuntu +
 * windows legs) but has NOT been executed on the macOS dev box (it cannot run
 * there). It is CI-validated-pending. The specs consume the same `FLOW` the
 * macOS smoke does, so the two harnesses can never describe different behaviour.
 *
 * The app is launched with `MDREADER_DEV_HASH_EMBED=1` (offline, instant index)
 * and a pre-seeded `session.json` granting the fixture corpus — the same seeding
 * the macOS driver uses (`seed.ts`), so no OS folder-picker is ever driven.
 */

import { spawn, type ChildProcess } from 'node:child_process';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { FLOW_CONFIG } from './flows.ts';
import { clearSeed, seedWorkspace } from './seed.ts';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, '..', '..', '..');

/** The built app executable for the current OS (raw binary, not the bundle). */
function appBinary(): string {
  const base = join(REPO_ROOT, 'target', 'release', 'mdreader-app');
  return process.platform === 'win32' ? `${base}.exe` : base;
}

const CORPUS = join(REPO_ROOT, FLOW_CONFIG.corpusRelPath);

let tauriDriver: ChildProcess | undefined;

export const config: WebdriverIO.Config = {
  runner: 'local',
  // tauri-driver listens here and proxies to the native webdriver.
  hostname: '127.0.0.1',
  port: 4444,
  path: '/',
  specs: [join(HERE, 'specs', '*.e2e.ts')],
  maxInstances: 1,
  capabilities: [
    {
      // The Tauri automation capability; `application` is the binary to launch.
      'tauri:options': {
        application: appBinary(),
      },
      // A W3C-valid but webview-agnostic browserName; tauri-driver ignores it.
      browserName: 'wry',
    } as WebdriverIO.Capabilities,
  ],
  logLevel: 'info',
  bail: 0,
  waitforTimeout: 15_000,
  connectionRetryTimeout: 120_000,
  connectionRetryCount: 3,
  framework: 'mocha',
  reporters: ['spec'],
  mochaOpts: {
    ui: 'bdd',
    timeout: 120_000,
  },

  // Seed the workspace so the app grants the fixture corpus on launch (no OS
  // folder-picker). Runs once before the session starts.
  onPrepare: async () => {
    await clearSeed(FLOW_CONFIG.bundleIdentifier);
    await seedWorkspace(FLOW_CONFIG.bundleIdentifier, CORPUS);
  },

  // tauri-driver must be running before the WebDriver session connects. It
  // inherits our env, so the app it launches sees MDREADER_DEV_HASH_EMBED.
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
