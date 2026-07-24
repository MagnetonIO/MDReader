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

import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { dirname } from 'node:path';

import { FLOW_CONFIG } from './flows.ts';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, '..', '..', '..');

/** The built desktop app binary for the current OS (raw binary, not the bundle) —
 *  what tauri-driver launches. */
export function desktopAppBinary(): string {
  const base = join(REPO_ROOT, 'target', 'release', 'mdreader-app');
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

/**
 * iOS — appium-xcuitest-driver against the Simulator. BLOCKED on the P6 mobile app
 * shell: there is no iOS build yet. `appium:app`/`bundleId` are placeholders the
 * P6 shell fills in; the adapter switches to the WEBVIEW context before driving
 * the DOM. Scaffolded + CI-wired so the harness runs the moment the shell lands.
 */
export const iosCapabilities: WebdriverIO.Capabilities = {
  platformName: 'iOS',
  'appium:automationName': 'XCUITest',
  'appium:deviceName': process.env.MDREADER_IOS_DEVICE ?? 'iPhone 16',
  'appium:platformVersion': process.env.MDREADER_IOS_VERSION ?? '18.2',
  // P6 supplies one of these (a built .app for the simulator, or the installed id):
  'appium:app': process.env.MDREADER_IOS_APP,
  'appium:bundleId': process.env.MDREADER_IOS_BUNDLE_ID ?? FLOW_CONFIG.bundleIdentifier,
  // Let the driver expose the webview context (we switch to it in the adapter).
  'appium:autoWebview': false,
  'appium:includeSafariInWebviews': false,
} as WebdriverIO.Capabilities;

/**
 * Android — appium-uiautomator2-driver against an emulator. BLOCKED on the P6
 * mobile app shell: there is no Android build yet. `appium:app` is a placeholder;
 * the adapter switches to the WEBVIEW context. Scaffolded + CI-wired.
 */
export const androidCapabilities: WebdriverIO.Capabilities = {
  platformName: 'Android',
  'appium:automationName': 'UiAutomator2',
  'appium:deviceName': process.env.MDREADER_ANDROID_DEVICE ?? 'emulator-5554',
  'appium:app': process.env.MDREADER_ANDROID_APP,
  'appium:appPackage': process.env.MDREADER_ANDROID_PACKAGE ?? 'dev.mdreader.app',
  // The Tauri/mobile webview is a Chrome-backed WebView; UiAutomator2 exposes it
  // as a WEBVIEW_<pkg> context the adapter switches into.
  'appium:autoWebview': false,
  'appium:ensureWebviewsHavePages': true,
  'appium:nativeWebScreenshot': true,
} as WebdriverIO.Capabilities;
