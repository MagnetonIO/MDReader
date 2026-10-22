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
 * The app is launched with an owned native no-model service (real text index)
 * and a pre-seeded `session.json` granting the fixture corpus (no OS folder-picker).
 */

import { spawn, type ChildProcess } from 'node:child_process';
import { realpath } from 'node:fs/promises';
import { join } from 'node:path';
import { E2E_ROOT, REPO_ROOT } from '../repository.mjs';

import { FLOW_CONFIG } from '../flows.ts';
import { desktopCapabilitiesFor, desktopAppBinary } from '../capabilities.ts';
import { startDesktopFixture, type DesktopFixture } from '../desktop-fixture.ts';
import { E2E_LOCAL_SOURCE_ID } from '../seed.ts';
let readerFixture: DesktopFixture | undefined;
const CORPUS = join(REPO_ROOT, FLOW_CONFIG.corpusRelPath);

let tauriDriver: ChildProcess | undefined;

/** PROBE ONLY: launch the app directly for 12s and print whether it stays up,
 * its exit code and its stdout/stderr, to explain DevToolsActivePort. */
async function probeWindowsAppLaunch(): Promise<void> {
  const app = spawn(desktopAppBinary(), [], { env: process.env, stdio: ['ignore', 'pipe', 'pipe'] });
  let out = '';
  app.stdout?.on('data', (b: Buffer) => { out += b.toString('utf8'); });
  app.stderr?.on('data', (b: Buffer) => { out += b.toString('utf8'); });
  const exit = await new Promise<string>((resolve) => {
    const timer = setTimeout(() => resolve('still running after 12s'), 12_000);
    app.once('exit', (code, signal) => { clearTimeout(timer); resolve(`exited code=${code} signal=${signal}`); });
    app.once('error', (e) => { clearTimeout(timer); resolve(`spawn error ${e.message}`); });
  });
  console.log(`[probe] direct app launch: ${exit}; output tail: ${JSON.stringify(out.slice(-3000))}`);
  if (app.exitCode === null) { app.kill(); await new Promise((r) => setTimeout(r, 2000)); }
}

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
    readerFixture = await startDesktopFixture(desktopAppBinary(), corpusRoot);
    process.env.MDREADER_E2E_PROFILE_ROOT = readerFixture.profile.dataDir;
    process.env.MDREADER_E2E_SERVICE_PID = String(readerFixture.pid);
    process.env.MDREADER_E2E_CORPUS_ROOT = corpusRoot;
    process.env.MDREADER_E2E_SOURCE_ID = E2E_LOCAL_SOURCE_ID;
    if (process.platform === 'win32') await probeWindowsAppLaunch();
  },

  // tauri-driver must be running before the WebDriver session connects. It inherits
  // our explicit disposable profile environment.
  beforeSession: () => {
    tauriDriver = spawn('tauri-driver', [], {
      stdio: [null, process.stdout, process.stderr],
      env: process.env,
    });
  },

  afterSession: () => { tauriDriver?.kill(); },
  onComplete: async () => {
    await readerFixture?.close();
    readerFixture = undefined;
    delete process.env.MDREADER_E2E_PROFILE_ROOT;
    delete process.env.MDREADER_E2E_SERVICE_PID;
    delete process.env.MDREADER_E2E_CORPUS_ROOT;
    delete process.env.MDREADER_E2E_SOURCE_ID;
  },
};
