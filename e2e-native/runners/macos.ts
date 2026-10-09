/**
 * Milestone PT — the macOS acceptance entry for the UNIFIED harness.
 *
 * This is the leg that RUNS locally on a Mac and is the acceptance evidence. It
 * launches the real, freshly-built `.app` against the seeded fixture workspace,
 * opens an appium-mac2-driver (XCUITest) WebDriver session, and drives the shared
 * `FLOW` through {@link Mac2Adapter} — native shell over the WebDriver protocol,
 * WKWebView read through the compiled `axquery` AX helper.
 *
 * It owns the fiddly macOS session lifecycle so `scripts/macos-smoke.sh` is a
 * one-liner:
 *   - validates the exact isolated bundle and its embedded source revision;
 *   - creates a new private disposable profile and seeds its public folder request;
 *   - compiles the `axquery` helper;
 *   - starts the exact packaged SQLite service in supported no-model mode;
 *   - binds AX reads and cleanup to the executable mac2 actually loaded;
 *   - connects a WebdriverIO session to a local Appium server (spawning one if
 *     none is already listening), runs the flow, and tears everything down.
 *
 * FAULT INJECTION: set `MDREADER_SMOKE_FAULT=<step-id>` (or a read-path probe id)
 * and the run must go RED at that step. See `../drivers/runner.ts` and
 * `../drivers/macos/mac2-adapter.ts`.
 *
 * Runs under Node's native TypeScript (Node >= 22.13 with `--experimental-strip-
 * types`; >= 23 strips by default). Node built-ins + the local `webdriverio`.
 */

import { execFileSync, spawn, type ChildProcess } from 'node:child_process';
import { existsSync, realpathSync } from 'node:fs';
import { realpath } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { remote } from 'webdriverio';

import { E2E_PROCESS_NAME, E2E_PRODUCT_NAME, FLOW_CONFIG } from '../flows.ts';
import {
  macosCapabilitiesFor,
  validateDesktopAppBinary,
} from '../capabilities.ts';
import {
  createE2EProfile,
  E2E_LOCAL_SOURCE_ID,
  withNativeRunArtifacts,
  type E2EProfile,
} from '../seed.ts';
import {
  resolveOwnedRunOutcome,
  terminateExclusiveExecutableOwner,
  terminateOwnedProcess,
  waitForExclusiveExecutableOwner,
  type ExecutableProcessOwner,
} from '../lifecycle.ts';
import { buildSwiftHelper } from '../drivers/ax-helper.ts';
import { Mac2Adapter } from '../drivers/macos/mac2-adapter.ts';
import { consoleLog, runFlow } from '../drivers/runner.ts';
import { startDesktopFixture } from '../desktop-fixture.ts';
import { E2E_ROOT, REPO_ROOT } from '../repository.mjs';

const APPIUM_HOME = join(E2E_ROOT, '.appium');
const APPIUM_PORT = Number(process.env.APPIUM_PORT ?? 4723);
const APPIUM_BASE = `http://127.0.0.1:${APPIUM_PORT}`;

const FAULT = process.env.MDREADER_SMOKE_FAULT ?? '';

interface Options {
  app: string;
  binary: string;
  corpus: string;
  artifacts: string;
  profile: E2EProfile;
  keep: boolean;
}

function parseArgs(argv: string[]): Options {
  const get = (flag: string): string | undefined => {
    const i = argv.indexOf(flag);
    return i >= 0 ? argv[i + 1] : undefined;
  };
  const app = resolve(get('--app') ?? join(REPO_ROOT, 'target/release/bundle/macos/MDReader E2E.app'));
  return {
    app,
    binary: join(app, 'Contents/MacOS', E2E_PROCESS_NAME),
    corpus: resolve(get('--corpus') ?? join(REPO_ROOT, FLOW_CONFIG.corpusRelPath)),
    artifacts: resolve(get('--artifacts') ?? join(REPO_ROOT, 'target/e2e-artifacts/macos')),
    profile: createE2EProfile(),
    keep: argv.includes('--keep'),
  };
}

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

function bundleIdentifier(app: string): string {
  return execFileSync('/usr/libexec/PlistBuddy', ['-c', 'Print :CFBundleIdentifier', join(app, 'Contents', 'Info.plist')], {
    encoding: 'utf8',
  }).trim();
}

function assertE2EBundle(app: string, profile: E2EProfile): void {
  const actual = bundleIdentifier(app);
  if (actual !== profile.identifier) {
    throw new Error(`refusing to seed or launch ${app}: expected E2E bundle ${profile.identifier}, found ${actual || '(missing)'}`);
  }
}

function runningE2EProcessIds(): number[] {
  try {
    return execFileSync('/usr/bin/pgrep', ['-x', E2E_PROCESS_NAME], {
      encoding: 'utf8',
    })
      .trim()
      .split(/\s+/)
      .filter(Boolean)
      .map(Number)
      .filter(Number.isSafeInteger);
  } catch (error) {
    if ((error as { status?: number }).status === 1) return [];
    throw error;
  }
}

function runningE2EProcessOwners(): ExecutableProcessOwner[] {
  return runningE2EProcessIds().flatMap((pid) => {
    let output: string;
    try {
      output = execFileSync(
        '/usr/sbin/lsof',
        ['-a', '-p', String(pid), '-d', 'txt', '-Fn'],
        { encoding: 'utf8' },
      );
    } catch (error) {
      if ((error as { status?: number }).status === 1) return [];
      throw error;
    }
    const executable = output
      .split(/\r?\n/)
      .find((line) => line.startsWith('n'))
      ?.slice(1);
    if (!executable) {
      throw new Error(`could not resolve the loaded executable for E2E pid ${pid}`);
    }
    return [{ pid, executable: realpathSync(executable) }];
  });
}

async function appiumReachable(): Promise<boolean> {
  try {
    const res = await fetch(`${APPIUM_BASE}/status`, { signal: AbortSignal.timeout(2000) });
    return res.ok;
  } catch {
    return false;
  }
}

/** Ensure an Appium server is listening. If one already is, use it (and don't own
 *  it). Otherwise spawn one against the local APPIUM_HOME and return its handle. */
async function ensureAppium(): Promise<ChildProcess | undefined> {
  if (await appiumReachable()) return undefined;
  const child = spawn('npx', ['appium', 'server', '-p', String(APPIUM_PORT), '--base-path', '/', '--log-level', 'error'], {
    cwd: E2E_ROOT,
    env: { ...process.env, APPIUM_HOME },
    stdio: 'ignore',
  });
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (await appiumReachable()) return child;
    await sleep(500);
  }
  child.kill();
  throw new Error('Appium server did not become ready within 30s');
}

async function main(): Promise<number> {
  const opts = parseArgs(process.argv.slice(2));

  console.log(`── ${E2E_PRODUCT_NAME} macOS GUI smoke (unified Appium/mac2 harness) ──`);
  console.log(`app:       ${opts.app}`);
  console.log(`corpus:    ${opts.corpus}`);
  console.log(`artifacts: ${opts.artifacts}`);
  if (FAULT) console.log(`FAULT INJECTION ACTIVE: "${FAULT}" — expect a RED failure at that step.`);

  try {
    assertE2EBundle(opts.app, opts.profile);
    validateDesktopAppBinary(opts.binary);
  } catch (err) {
    console.error(`\nFAIL: ${err instanceof Error ? err.message : String(err)}`);
    return 1;
  }
  if (!existsSync(opts.binary)) {
    console.error(`\nFAIL: app binary not found: ${opts.binary}`);
    console.error('Build it first: pnpm --dir e2e-native run build:e2e');
    return 1;
  }
  if (!existsSync(opts.corpus)) {
    console.error(`\nFAIL: fixture corpus not found: ${opts.corpus}`);
    return 1;
  }
  const existing = runningE2EProcessIds();
  if (existing.length > 0) {
    console.error(
      `\nFAIL: refusing to run while another ${E2E_PROCESS_NAME} process owns the E2E bundle: ` +
        existing.join(', '),
    );
    return 1;
  }
  const corpusRoot = await realpath(opts.corpus);
  const expectedExecutable = realpathSync(opts.binary);

  const summary = await withNativeRunArtifacts(opts.artifacts, corpusRoot, async (artifacts) => {
    console.log(`run:       ${artifacts}`);

    const axBin = buildSwiftHelper(artifacts, 'axquery');
    console.log(`axquery:   ${axBin}`);
    // The chord-delivery fallback, used only when a shortcut demonstrably never
    // reached the app because another application owns it system-wide. See
    // `drivers/macos/keypost.swift` and `runner.ts`'s `openSurface`.
    const keyBin = buildSwiftHelper(artifacts, 'keypost');
    console.log(`keypost:   ${keyBin}\n`);

    const fixture = await startDesktopFixture(opts.binary, corpusRoot);
    opts.profile = fixture.profile;
    let appium: ChildProcess | undefined;
    let adapter: Mac2Adapter | undefined;
    let browser: Awaited<ReturnType<typeof remote>> | undefined;
    let sessionPid: number | undefined;
    let sessionBoundToExactApp = false;
    let outcome: Awaited<ReturnType<typeof runFlow>> | undefined;
    let primaryError: unknown;
    let cleanupError: unknown;
    try {
      appium = await ensureAppium();

      // mac2 must launch the exact reviewed bundle path. Launching the binary
      // separately and attaching only by bundle identifier is ambiguous when
      // LaunchServices knows another worktree build with the same E2E identity;
      // XCTest may replace that process while creating its application session.
      const capabilities = macosCapabilitiesFor(
        opts.app,
        opts.binary,
        {
          MDREADER_E2E_PROFILE_ROOT: opts.profile.dataDir,
          MDREADER_E2E_CORPUS_ROOT: corpusRoot,
          MDREADER_E2E_SOURCE_ID: E2E_LOCAL_SOURCE_ID,
        },
      );
      browser = await remote({
        hostname: '127.0.0.1',
        port: APPIUM_PORT,
        path: '/',
        logLevel: 'error',
        // mac2 builds WebDriverAgentMac with xcodebuild the first time it runs on a
        // machine (and after an Xcode upgrade wipes DerivedData). That is minutes,
        // not seconds, and WebdriverIO's default session timeout aborts the POST
        // while the build is still going — which surfaces as an opaque
        // "operation was aborted due to timeout" and no run at all. Warm runs are
        // still ~3s; this only has to be longer than a cold build.
        connectionRetryTimeout: 600_000,
        capabilities: {
          ...capabilities,
          // Same reasoning on the server side of the handshake.
          'appium:newCommandTimeout': 600,
          'appium:showServerLogs': true,
        } as WebdriverIO.Capabilities,
      });

      const pid = await waitForExclusiveExecutableOwner(
        expectedExecutable,
        () => runningE2EProcessOwners().filter((owner) => owner.pid !== fixture.pid),
        () => sleep(100),
      );
      sessionPid = pid;
      adapter = new Mac2Adapter({
        browser,
        bundleId: opts.profile.identifier,
        pid,
        axBin,
        keyBin,
        artifacts,
      });
      const windowDeadline = Date.now() + 20_000;
      let exactWindow = false;
      while (!exactWindow && Date.now() < windowDeadline) {
        exactWindow = await adapter.isWindowPresent();
        if (!exactWindow) await sleep(250);
      }
      if (!exactWindow) {
        throw new Error(
          `WebDriver window did not match the AX window owned by mac2 pid ${pid}`,
        );
      }
      sessionBoundToExactApp = true;
      outcome = await runFlow(adapter, consoleLog, fixture);
    } catch (error) {
      primaryError = error;
    } finally {
      const keepExactApp = opts.keep && sessionBoundToExactApp;
      if (browser && !keepExactApp) {
        try {
          await browser.execute('macos: terminateApp', { path: opts.app });
        } catch (error) {
          cleanupError ??= error;
        }
      }
      try {
        if (adapter) await adapter.dispose();
        else await browser?.deleteSession();
      } catch (error) {
        cleanupError ??= error;
      }
      if (!keepExactApp && sessionPid !== undefined) {
        try {
          await terminateExclusiveExecutableOwner(
            expectedExecutable,
            sessionPid,
            () => runningE2EProcessOwners().filter((owner) => owner.pid !== fixture.pid),
            (pid, signal) => process.kill(pid, signal),
            () => sleep(100),
          );
        } catch (error) {
          cleanupError ??= error;
        }
      }
      try {
        await terminateOwnedProcess(appium);
      } catch (error) {
        cleanupError ??= error;
      }
      try {
        if (!keepExactApp) await fixture.close();
        else console.log(`Retained explicit --keep E2E profile: ${fixture.profile.dataDir}`);
      } catch (error) {
        cleanupError ??= error;
      }
    }
    return resolveOwnedRunOutcome(primaryError, cleanupError, outcome);
  });

  console.log('───────────────────────────────────────────────────────────');
  if (summary.ok) {
    console.log(
      'RESULT: PASS — all flow steps green; source bytes, mtimes, and membership unchanged.',
    );
    return 0;
  }
  if (!summary.sourceIntegrity.unchanged) {
    console.log(
      `RESULT: FAIL — source integrity changed: ${summary.sourceIntegrity.changed.join(', ') || '(unknown)'}.`,
    );
    return 1;
  }
  if (summary.error) console.log(`RESULT: FAIL — ${summary.error}.`);
  else console.log(`RESULT: FAIL at step "${summary.failedAt}".`);
  return 1;
}

main()
  .then((code) => process.exit(code))
  .catch((err) => {
    console.error('macOS smoke runner crashed:', err);
    process.exit(1);
  });
