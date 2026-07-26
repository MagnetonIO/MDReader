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
 *   - owns only the app process it launches; it never searches for or terminates
 *     another MDReader process;
 *   - registers the bundle with LaunchServices so the session can attach by id;
 *   - wipes the plugin-store + engine index and RE-SEEDS the fixture workspace;
 *   - compiles the `axquery` helper;
 *   - launches the app with `MDREADER_DEV_HASH_EMBED=1` (offline, instant index);
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
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { remote } from 'webdriverio';

import { E2E_PRODUCT_NAME, FLOW_CONFIG } from '../flows.ts';
import { assertE2EBundle } from '../bundle.ts';
import { appConfigDir, clearSeed, e2eProfile, seedWorkspace, type E2EProfile } from '../seed.ts';
import { buildSwiftHelper } from '../drivers/ax-helper.ts';
import { Mac2Adapter } from '../drivers/macos/mac2-adapter.ts';
import { consoleLog, runFlow } from '../drivers/runner.ts';

const HERE = dirname(fileURLToPath(import.meta.url));
const E2E_ROOT = resolve(HERE, '..');
const REPO_ROOT = resolve(E2E_ROOT, '..', '..', '..');
const APPIUM_HOME = join(E2E_ROOT, '.appium');
const APPIUM_PORT = Number(process.env.APPIUM_PORT ?? 4723);
const APPIUM_BASE = `http://127.0.0.1:${APPIUM_PORT}`;

const FAULT = process.env.MDREADER_SMOKE_FAULT ?? '';

/**
 * The reviewer the seeded overlay claims are signed by, and the schema whose
 * claims are reviewed.
 *
 * The P7 semantic steps read an interpretation that is populated, and an
 * overlay fact only reaches the graph when a human approves it — the engine
 * rejects an unapproved one on every write path. Performing ~30 approvals
 * through the Inspector before each run would make the interesting steps
 * hostage to the longest one, so the review state is seeded the same way the
 * workspace grant is: out of band, through the REAL approval call with a real
 * reviewer name (`examples/seed_reviewed_workspace.rs`). Nothing about the
 * product's approval gate is relaxed, and the flow still drives the Inspector
 * itself — over claims that have been ruled on, which is what the graph the
 * Constraint Explorer checks is made of.
 */
const REVIEWER = 'matthew.long';
const REVIEWED_SCHEMA = 'architecture_overlay';

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
  const app = resolve(get('--app') ?? join(REPO_ROOT, 'target/release/bundle/macos', `${E2E_PRODUCT_NAME}.app`));
  return {
    app,
    binary: join(app, 'Contents/MacOS/mdreader-app'),
    corpus: resolve(get('--corpus') ?? join(REPO_ROOT, FLOW_CONFIG.corpusRelPath)),
    artifacts: resolve(get('--artifacts') ?? join(REPO_ROOT, 'target/e2e-artifacts/macos')),
    profile: e2eProfile(),
    keep: argv.includes('--keep'),
  };
}

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

function quietExec(cmd: string, args: string[]): void {
  try {
    execFileSync(cmd, args, { stdio: 'ignore' });
  } catch {
    /* best-effort */
  }
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

  console.log('── MDReader macOS GUI smoke (unified Appium/mac2 harness) ──');
  console.log(`app:       ${opts.app}`);
  console.log(`corpus:    ${opts.corpus}`);
  console.log(`artifacts: ${opts.artifacts}`);
  if (FAULT) console.log(`FAULT INJECTION ACTIVE: "${FAULT}" — expect a RED failure at that step.`);

  if (!existsSync(opts.binary)) {
    console.error(`\nFAIL: app binary not found: ${opts.binary}`);
    console.error('Build it first: pnpm --filter @mdreader/app tauri build --bundles app --config src-tauri/tauri.e2e.conf.json');
    return 1;
  }
  try {
    assertE2EBundle(opts.app);
  } catch (err) {
    console.error(`\nFAIL: ${err instanceof Error ? err.message : String(err)}`);
    return 1;
  }
  if (!existsSync(opts.corpus)) {
    console.error(`\nFAIL: fixture corpus not found: ${opts.corpus}`);
    return 1;
  }

  mkdirSync(opts.artifacts, { recursive: true });

  // Register the bundle so the mac2 session can attach to it by bundle id.
  quietExec('/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister', ['-f', opts.app]);

  // Fresh state: wipe the plugin-store + engine index, then re-seed the fixture
  // workspace so the app re-grants + re-indexes from scratch on launch.
  await clearSeed(opts.profile);
  const seedPath = await seedWorkspace(opts.profile, opts.corpus);
  console.log(`seeded:    ${seedPath}`);

  // Seed the REVIEW state into the very workspace the app is about to open, so
  // the semantic surfaces have an interpretation to show and a law to break.
  // Fails the run outright if it cannot: a GUI pass over an empty graph would
  // prove less than it claims.
  try {
    const seeded = execFileSync(
      'cargo',
      [
        'run', '--quiet',
        '-p', 'mdreader-ingest',
        '--example', 'seed_reviewed_workspace',
        '--no-default-features',
        '--',
        opts.corpus,
        appConfigDir(opts.profile),
        REVIEWER,
        REVIEWED_SCHEMA,
      ],
      { cwd: REPO_ROOT, encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] },
    );
    for (const line of seeded.trimEnd().split('\n')) console.log(`review:    ${line}`);
  } catch (err) {
    console.error(`\nFAIL: could not seed the reviewed workspace: ${err instanceof Error ? err.message : String(err)}`);
    return 1;
  }

  const axBin = buildSwiftHelper(opts.artifacts, 'axquery');
  console.log(`axquery:   ${axBin}`);
  // The chord-delivery fallback, used only when a shortcut demonstrably never
  // reached the app because another application owns it system-wide. See
  // `drivers/macos/keypost.swift` and `runner.ts`'s `openSurface`.
  const keyBin = buildSwiftHelper(opts.artifacts, 'keypost');
  console.log(`keypost:   ${keyBin}\n`);

  const appium = await ensureAppium();

  // Launch the real binary (offline instant index), then attach the session to it.
  const child = spawn(opts.binary, [], {
    env: { ...process.env, MDREADER_DEV_HASH_EMBED: '1' },
    stdio: 'ignore',
    detached: false,
  });
  if (child.pid === undefined) {
    console.error('FAIL: could not spawn the app');
    appium?.kill();
    return 1;
  }

  let result;
  try {
    const browser = await remote({
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
        platformName: 'mac',
        'appium:automationName': 'mac2',
        'appium:bundleId': opts.profile.identifier,
        'appium:noReset': true,
        // Same reasoning on the server side of the handshake.
        'appium:newCommandTimeout': 600,
        'appium:showServerLogs': true,
      } as WebdriverIO.Capabilities,
    });

    const adapter = new Mac2Adapter({
      browser,
      bundleId: opts.profile.identifier,
      pid: child.pid,
      axBin,
      keyBin,
      artifacts: opts.artifacts,
    });
    try {
      result = await runFlow(adapter, consoleLog);
    } finally {
      await adapter.dispose();
    }
  } finally {
    // The close step already ended the app on success; otherwise tidy up unless
    // asked to keep it. Then release the Appium server if we own it.
    if (!opts.keep && child.exitCode === null && !child.killed) child.kill('SIGTERM');
    appium?.kill();
  }

  const summary = { ok: result.ok, failedAt: result.failedAt, artifacts: opts.artifacts, results: result.results };
  writeFileSync(join(opts.artifacts, 'result.json'), JSON.stringify(summary, null, 2));

  console.log('───────────────────────────────────────────────────────────');
  if (result.ok) {
    console.log('RESULT: PASS — all flow steps green (drag + red-X close asserted via mac2).');
    return 0;
  }
  console.log(`RESULT: FAIL at step "${result.failedAt}".`);
  return 1;
}

main()
  .then((code) => process.exit(code))
  .catch((err) => {
    console.error('macOS smoke runner crashed:', err);
    process.exit(1);
  });
