/**
 * Milestone PT — the per-driver Appium/WebDriver capability sets for the unified
 * harness. One place every leg's capabilities live, so the runner + adapters stay
 * transport-agnostic and the CI configs just import the right set.
 *
 * Each set names the driver that speaks the WebDriver protocol for that target:
 *   - macOS   → Mac2  (appium-mac2-driver, XCUITest) — drives native shell + AX
 *               webview reads. RUNS locally.
 *   - Linux   → tauri-driver (WebKitGTK webview)  — webview DOM only.
 *   - Windows → tauri-driver (WebView2)           — webview DOM only.
 *   - iOS     → XCUITest  (appium-xcuitest-driver, Simulator) — webview context.
 *   - Android → UiAutomator2 (appium-uiautomator2-driver, emulator) — webview context.
 *
 * A Windows NATIVE-shell leg (appium-windows-driver / WinAppDriver, for
 * drag/resize/close) is a FOLLOW-UP: {@link windowsNativeCapabilities} exists as a
 * capability stub, but no adapter drives the Windows OS chrome yet — today the
 * native-window steps are covered only by the macOS mac2 leg.
 *
 * Node built-ins only; no runtime deps.
 */

import { execFileSync } from 'node:child_process';
import {
  closeSync,
  lstatSync,
  openSync,
  readFileSync,
  readSync,
} from 'node:fs';
import { dirname, isAbsolute, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { scanAndroidArchive } from './apk-manifest-scanner.mjs';
import { E2E_BUNDLE_IDENTIFIER, E2E_PROCESS_NAME, FLOW_CONFIG } from './flows.ts';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, '..', '..', '..');
const BUILD_MANIFEST_PREFIX = Buffer.from('MDREADER_E2E_BUILD_MANIFEST:');
const BUILD_MANIFEST_SUFFIX = Buffer.from(':MDREADER_E2E_BUILD_MANIFEST_END');
const MAX_ANDROID_NATIVE_LIBRARY_BYTES = 512 * 1024 * 1024;
const MAX_ANDROID_TOTAL_NATIVE_LIBRARY_BYTES = 1024 * 1024 * 1024;
const MAX_ANDROID_NATIVE_LIBRARIES = 64;
const MAX_ANDROID_NON_NATIVE_EXECUTABLE_BYTES = 516 * 1024 * 1024;
const ANDROID_APP_LIBRARY = 'lib/x86_64/libmdreader_app_lib.so';

interface E2eBuildManifest {
  schema: 1;
  source_commit: string;
  bundle_identifier: string;
  profile: 'e2e-build';
}

function expectedSourceCommit(): string {
  const configured = process.env.MDREADER_SOURCE_COMMIT;
  const commit =
    configured ??
    execFileSync('git', ['rev-parse', 'HEAD'], {
      cwd: REPO_ROOT,
      encoding: 'utf8',
    }).trim();
  if (!/^[0-9a-f]{40}$/.test(commit)) {
    throw new Error('expected source commit must be a 40-character lowercase Git commit');
  }
  return commit;
}

export function validateEmbeddedBuildManifest(
  bytes: Buffer,
  expectedCommit: string,
): E2eBuildManifest {
  if (!/^[0-9a-f]{40}$/.test(expectedCommit)) {
    throw new Error('expected source commit must be a 40-character lowercase Git commit');
  }
  const start = bytes.indexOf(BUILD_MANIFEST_PREFIX);
  if (start < 0) throw new Error('artifact has no embedded E2E build manifest');
  const jsonStart = start + BUILD_MANIFEST_PREFIX.length;
  const end = bytes.indexOf(BUILD_MANIFEST_SUFFIX, jsonStart);
  if (end < 0 || bytes.indexOf(BUILD_MANIFEST_PREFIX, jsonStart) >= 0) {
    throw new Error('artifact has an ambiguous embedded E2E build manifest');
  }
  let manifest: unknown;
  try {
    manifest = JSON.parse(bytes.subarray(jsonStart, end).toString('utf8'));
  } catch {
    throw new Error('artifact has an invalid embedded E2E build manifest');
  }
  if (
    typeof manifest !== 'object' ||
    manifest === null ||
    (manifest as Record<string, unknown>).schema !== 1 ||
    (manifest as Record<string, unknown>).bundle_identifier !== E2E_BUNDLE_IDENTIFIER ||
    (manifest as Record<string, unknown>).profile !== 'e2e-build' ||
    (manifest as Record<string, unknown>).source_commit !== expectedCommit
  ) {
    throw new Error(
      `artifact build manifest does not match source commit ${expectedCommit}`,
    );
  }
  return manifest as E2eBuildManifest;
}

async function validateAndroidArchive(
  apk: string,
  expectedCommit: string,
  maximumNativeLibraryBytes: number,
  maximumTotalNativeLibraryBytes: number,
): Promise<void> {
  const scan = await scanAndroidArchive(apk, {
    maximumNative: maximumNativeLibraryBytes,
    maximumAggregateNative: maximumTotalNativeLibraryBytes,
    maximumLibraries: MAX_ANDROID_NATIVE_LIBRARIES,
  });
  if (scan.applicationId !== E2E_BUNDLE_IDENTIFIER) {
    throw new Error(
      `compiled AndroidManifest.xml package must be ${E2E_BUNDLE_IDENTIFIER}`,
    );
  }
  const appLibrary = scan.nativeLibraries.find(
    (library) => library.name === ANDROID_APP_LIBRARY,
  );
  if (appLibrary === undefined || appLibrary.embeddedManifest === null) {
    throw new Error(
      `MDReader app library ${ANDROID_APP_LIBRARY} has no embedded E2E build manifest`,
    );
  }
  validateEmbeddedBuildManifest(appLibrary.embeddedManifest, expectedCommit);
  if (
    scan.nativeLibraries.some(
      (library) =>
        library.name !== ANDROID_APP_LIBRARY &&
        library.embeddedManifest !== null,
    )
  ) {
    throw new Error(
      `only the MDReader app library ${ANDROID_APP_LIBRARY} may contain the E2E build manifest`,
    );
  }
}

/** The built desktop app binary for the current OS (raw binary, not the bundle) —
 *  what tauri-driver launches. */
export function desktopAppBinary(): string {
  const base = join(REPO_ROOT, 'target', 'release', E2E_PROCESS_NAME);
  return process.platform === 'win32' ? `${base}.exe` : base;
}

/** Validate the exact desktop executable before a runner may seed or launch it. */
export function validateDesktopAppBinary(
  artifact: string,
  sourceCommit = expectedSourceCommit(),
): string {
  if (!isAbsolute(artifact)) {
    throw new Error(`desktop E2E application must be an absolute path: ${artifact}`);
  }
  let metadata;
  try {
    metadata = lstatSync(artifact);
  } catch {
    throw new Error(`desktop E2E application does not exist: ${artifact}`);
  }
  if (!metadata.isFile() || metadata.size === 0) {
    throw new Error(`desktop E2E application is empty or invalid: ${artifact}`);
  }
  if (process.platform !== 'win32' && (metadata.mode & 0o111) === 0) {
    throw new Error(`desktop E2E application is not executable: ${artifact}`);
  }
  validateEmbeddedBuildManifest(readFileSync(artifact), sourceCommit);
  return artifact;
}

/**
 * macOS — appium-mac2-driver (XCUITest). Attaches to the running app by bundle id
 * (the harness pre-launches it with the hash-embed env + seeded workspace). Used
 * by `runners/macos.ts`, which drives this leg directly via `webdriverio.remote`.
 */
export const macosCapabilities: WebdriverIO.Capabilities = {
  platformName: 'mac',
  'appium:automationName': 'mac2',
  'appium:bundleId': FLOW_CONFIG.bundleIdentifier,
  'appium:noReset': true,
};

/**
 * Linux/Windows desktop WEBVIEW — driven through `tauri-driver`, which bridges W3C
 * WebDriver to the platform webview driver (WebKitWebDriver on Linux, msedgedriver
 * / WebView2 on Windows). This drives the webview DOM ONLY (the shared FLOW's DOM
 * steps + launch); the native-window steps are pending on these legs (the mac2 leg
 * covers them). `tauri:options.application` is the binary to launch.
 * CI-validated-pending (cannot run on the macOS dev box).
 */
export function desktopCapabilitiesFor(
  artifact = desktopAppBinary(),
  sourceCommit = expectedSourceCommit(),
): WebdriverIO.Capabilities {
  return {
    'tauri:options': {
      application: validateDesktopAppBinary(artifact, sourceCommit),
    },
    // A W3C-valid but webview-agnostic browserName; tauri-driver ignores it.
    browserName: 'wry',
  } as WebdriverIO.Capabilities;
}

/**
 * Windows NATIVE shell — appium-windows-driver (WinAppDriver). FOLLOW-UP stub: this
 * capability set is here so a future Windows native-window adapter (drag/resize/
 * close over the OS chrome) has a home, but NO adapter drives it yet — today the
 * native-window steps are covered only by the macOS mac2 leg. Not wired into any
 * wdio config until that adapter lands.
 */
export const windowsNativeCapabilities: WebdriverIO.Capabilities = {
  platformName: 'windows',
  'appium:automationName': 'Windows',
  // WinAppDriver identifies the app by its Application User Model ID or the exe path
  // once the native-shell adapter exists; the P0 build lands the app at target/.
  'appium:app': desktopAppBinary(),
} as WebdriverIO.Capabilities;

function requiredAbsoluteArtifact(
  variable: 'MDREADER_IOS_APP' | 'MDREADER_ANDROID_APP',
  artifact: string | undefined,
): string {
  if (!artifact) throw new Error(`${variable} must point to a built mobile artifact`);
  if (!isAbsolute(artifact)) throw new Error(`${variable} must be an absolute path: ${artifact}`);
  return artifact;
}

function xmlPlistValue(plist: string, key: string): string | undefined {
  const escaped = key.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return plist.match(new RegExp(`<key>\\s*${escaped}\\s*</key>\\s*<string>([^<]+)</string>`))?.[1];
}

function plistValue(path: string, key: string): string {
  const bytes = readFileSync(path);
  const text = bytes.toString('utf8');
  if (text.trimStart().startsWith('<?xml') || text.includes('<plist')) {
    const value = xmlPlistValue(text, key);
    if (!value) throw new Error(`Info.plist has no ${key}`);
    return value;
  }
  if (bytes.subarray(0, 6).toString('ascii') !== 'bplist') {
    throw new Error('Info.plist is not a supported property list');
  }
  if (process.platform !== 'darwin') {
    throw new Error('binary Info.plist validation requires macOS plutil');
  }
  return execFileSync('/usr/bin/plutil', ['-extract', key, 'raw', '-o', '-', path], {
    encoding: 'utf8',
  }).trim();
}

/** Validate the exact unsigned arm64 iOS Simulator bundle used by Appium. */
export function validateIosApp(
  artifact: string | undefined,
  sourceCommit = expectedSourceCommit(),
): string {
  const app = requiredAbsoluteArtifact('MDREADER_IOS_APP', artifact);
  let appMetadata;
  try {
    appMetadata = lstatSync(app);
  } catch {
    throw new Error(`MDREADER_IOS_APP does not exist: ${app}`);
  }
  if (!app.endsWith('.app') || !appMetadata.isDirectory()) {
    throw new Error(`MDREADER_IOS_APP must point to a simulator .app directory: ${app}`);
  }

  const plist = join(app, 'Info.plist');
  let bundleIdentifier: string;
  let executableName: string;
  try {
    bundleIdentifier = plistValue(plist, 'CFBundleIdentifier');
    executableName = plistValue(plist, 'CFBundleExecutable');
    if (plistValue(plist, 'CFBundlePackageType') !== 'APPL') throw new Error();
  } catch (error) {
    if (error instanceof Error && error.message.includes('Info.plist')) throw error;
    throw new Error(`MDREADER_IOS_APP has an invalid Info.plist: ${app}`);
  }
  if (bundleIdentifier !== E2E_BUNDLE_IDENTIFIER) {
    throw new Error(
      `MDREADER_IOS_APP must use ${E2E_BUNDLE_IDENTIFIER}; found ${bundleIdentifier}`,
    );
  }

  const executable = join(app, executableName);
  let executableMetadata;
  try {
    executableMetadata = lstatSync(executable);
  } catch {
    throw new Error(`MDREADER_IOS_APP executable is missing: ${executable}`);
  }
  if (!executableMetadata.isFile() || executableMetadata.size === 0) {
    throw new Error(`MDREADER_IOS_APP executable is empty or invalid: ${executable}`);
  }
  if ((executableMetadata.mode & 0o111) === 0) {
    throw new Error(`MDREADER_IOS_APP executable is not executable: ${executable}`);
  }
  try {
    const kind = execFileSync('/usr/bin/file', ['-b', executable], {
      encoding: 'utf8',
    });
    if (!kind.includes('Mach-O 64-bit executable') || !kind.includes('arm64')) {
      throw new Error(`expected an arm64 Mach-O executable; found ${kind.trim()}`);
    }
    const architectures = execFileSync('/usr/bin/lipo', ['-archs', executable], {
      encoding: 'utf8',
    })
      .trim()
      .split(/\s+/);
    if (architectures.length !== 1 || architectures[0] !== 'arm64') {
      throw new Error(`expected only the arm64 Simulator ABI; found ${architectures.join(' ')}`);
    }
    const build = execFileSync('/usr/bin/xcrun', ['vtool', '-show-build', executable], {
      encoding: 'utf8',
    });
    if (!/platform\s+IOSSIMULATOR\b/.test(build)) {
      throw new Error('Mach-O does not target the iOS Simulator platform');
    }
    validateEmbeddedBuildManifest(readFileSync(executable), sourceCommit);
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new Error(`MDREADER_IOS_APP executable is not a valid arm64 Simulator Mach-O: ${detail}`);
  }
  return app;
}

/** Validate the exact x86_64 Android package used by the emulator Appium leg. */
export async function validateAndroidApk(
  artifact: string | undefined,
  sourceCommit: string,
  maximumNativeLibraryBytes = MAX_ANDROID_NATIVE_LIBRARY_BYTES,
  maximumTotalNativeLibraryBytes = MAX_ANDROID_TOTAL_NATIVE_LIBRARY_BYTES,
): Promise<string> {
  if (!Number.isSafeInteger(maximumNativeLibraryBytes) || maximumNativeLibraryBytes <= 0) {
    throw new Error('Android native library safety bound must be a positive integer');
  }
  if (
    !Number.isSafeInteger(maximumTotalNativeLibraryBytes) ||
    maximumTotalNativeLibraryBytes <= 0 ||
    maximumTotalNativeLibraryBytes >
      Number.MAX_SAFE_INTEGER - MAX_ANDROID_NON_NATIVE_EXECUTABLE_BYTES
  ) {
    throw new Error('Android aggregate native library safety bound must be a positive integer');
  }
  const apk = requiredAbsoluteArtifact('MDREADER_ANDROID_APP', artifact);
  let metadata;
  try {
    metadata = lstatSync(apk);
  } catch {
    throw new Error(`MDREADER_ANDROID_APP does not exist: ${apk}`);
  }
  if (!apk.endsWith('.apk') || !metadata.isFile()) {
    throw new Error(`MDREADER_ANDROID_APP must point to an APK file: ${apk}`);
  }
  if (metadata.size === 0) throw new Error(`MDREADER_ANDROID_APP is empty: ${apk}`);
  const magic = Buffer.alloc(4);
  let handle: number | undefined;
  let magicBytes = 0;
  try {
    handle = openSync(apk, 'r');
    magicBytes = readSync(handle, magic, 0, magic.byteLength, 0);
  } finally {
    if (handle !== undefined) closeSync(handle);
  }
  if (
    magicBytes !== 4 ||
    magic[0] !== 0x50 ||
    magic[1] !== 0x4b ||
    !(
      (magic[2] === 0x03 && magic[3] === 0x04) ||
      (magic[2] === 0x05 && magic[3] === 0x06) ||
      (magic[2] === 0x07 && magic[3] === 0x08)
    )
  ) {
    throw new Error(`MDREADER_ANDROID_APP is not an APK/ZIP package: ${apk}`);
  }
  try {
    await validateAndroidArchive(
      apk,
      sourceCommit,
      maximumNativeLibraryBytes,
      maximumTotalNativeLibraryBytes,
    );
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new Error(`MDREADER_ANDROID_APP has an invalid manifest or x86_64 APK: ${detail}`);
  }
  return apk;
}

/** iOS — appium-xcuitest-driver against a validated unsigned Simulator build. */
export function iosCapabilitiesFor(
  artifact: string | undefined = process.env.MDREADER_IOS_APP,
  sourceCommit = expectedSourceCommit(),
): WebdriverIO.Capabilities {
  const platformVersion = process.env.MDREADER_IOS_VERSION;
  return {
    platformName: 'iOS',
    'appium:automationName': 'XCUITest',
    'appium:deviceName': process.env.MDREADER_IOS_DEVICE ?? 'iPhone 17',
    ...(platformVersion ? { 'appium:platformVersion': platformVersion } : {}),
    'appium:app': validateIosApp(artifact, sourceCommit),
    'appium:bundleId': E2E_BUNDLE_IDENTIFIER,
    'appium:additionalWebviewBundleIds': ['*'],
    'appium:autoWebview': false,
    'appium:includeSafariInWebviews': false,
    'appium:wdaLaunchTimeout': 180_000,
    'appium:wdaConnectionTimeout': 180_000,
  } as WebdriverIO.Capabilities;
}

/** Android — appium-uiautomator2-driver against a validated debug APK. */
export async function androidCapabilitiesFor(
  artifact: string | undefined = process.env.MDREADER_ANDROID_APP,
  sourceCommit = expectedSourceCommit(),
): Promise<WebdriverIO.Capabilities> {
  return {
    platformName: 'Android',
    'appium:automationName': 'UiAutomator2',
    'appium:deviceName': process.env.MDREADER_ANDROID_DEVICE ?? 'emulator-5554',
    'appium:app': await validateAndroidApk(artifact, sourceCommit),
    'appium:appPackage': E2E_BUNDLE_IDENTIFIER,
    'appium:autoWebview': false,
    'appium:ensureWebviewsHavePages': true,
    'appium:nativeWebScreenshot': true,
  } as WebdriverIO.Capabilities;
}
