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
import { lstatSync, readFileSync } from 'node:fs';
import { dirname, isAbsolute, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { E2E_BUNDLE_IDENTIFIER, E2E_PROCESS_NAME, FLOW_CONFIG } from './flows.ts';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, '..', '..', '..');

/** The built desktop app binary for the current OS (raw binary, not the bundle) —
 *  what tauri-driver launches. */
export function desktopAppBinary(): string {
  const base = join(REPO_ROOT, 'target', 'release', E2E_PROCESS_NAME);
  return process.platform === 'win32' ? `${base}.exe` : base;
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
export const desktopCapabilities: WebdriverIO.Capabilities = {
  'tauri:options': { application: desktopAppBinary() },
  // A W3C-valid but webview-agnostic browserName; tauri-driver ignores it.
  browserName: 'wry',
} as WebdriverIO.Capabilities;

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

function validateIosApp(artifact: string | undefined): string {
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
  return app;
}

function validateAndroidApk(artifact: string | undefined): string {
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
  return apk;
}

/** iOS — appium-xcuitest-driver against a validated unsigned Simulator build. */
export function iosCapabilitiesFor(
  artifact: string | undefined = process.env.MDREADER_IOS_APP,
): WebdriverIO.Capabilities {
  const platformVersion = process.env.MDREADER_IOS_VERSION;
  return {
    platformName: 'iOS',
    'appium:automationName': 'XCUITest',
    'appium:deviceName': process.env.MDREADER_IOS_DEVICE ?? 'iPhone 17',
    ...(platformVersion ? { 'appium:platformVersion': platformVersion } : {}),
    'appium:app': validateIosApp(artifact),
    'appium:bundleId': E2E_BUNDLE_IDENTIFIER,
    'appium:autoWebview': false,
    'appium:includeSafariInWebviews': false,
  } as WebdriverIO.Capabilities;
}

/** Android — appium-uiautomator2-driver against a validated debug APK. */
export function androidCapabilitiesFor(
  artifact: string | undefined = process.env.MDREADER_ANDROID_APP,
): WebdriverIO.Capabilities {
  return {
    platformName: 'Android',
    'appium:automationName': 'UiAutomator2',
    'appium:deviceName': process.env.MDREADER_ANDROID_DEVICE ?? 'emulator-5554',
    'appium:app': validateAndroidApk(artifact),
    'appium:appPackage': E2E_BUNDLE_IDENTIFIER,
    'appium:autoWebview': false,
    'appium:ensureWebviewsHavePages': true,
    'appium:nativeWebScreenshot': true,
  } as WebdriverIO.Capabilities;
}
