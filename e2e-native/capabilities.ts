/**
 * Milestone PT — the per-driver Appium/WebDriver capability sets for the unified
 * harness. One place every leg's capabilities live, so the runner + adapters stay
 * transport-agnostic and the CI configs just import the right set.
 *
 * Each set names the Appium driver (automationName) that speaks the WebDriver
 * protocol for that target:
 *   - macOS   → Mac2      (appium-mac2-driver, XCUITest)   — RUNS locally
 *   - Windows → Windows   (appium-windows-driver, WinAppDriver + WebView2)
 *   - Linux   → tauri-driver (WebKitGTK webview)           — via tauri:options
 *   - iOS     → XCUITest  (appium-xcuitest-driver, Simulator)
 *   - Android → UiAutomator2 (appium-uiautomator2-driver, emulator)
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
 * Linux/Windows desktop — driven through `tauri-driver`, which bridges W3C
 * WebDriver to the platform webview driver (WebKitWebDriver on Linux, msedgedriver
 * / WebView2 on Windows). `tauri:options.application` is the binary to launch.
 * CI-validated-pending (cannot run on the macOS dev box).
 */
export const desktopCapabilities: WebdriverIO.Capabilities = {
  'tauri:options': { application: desktopAppBinary() },
  // A W3C-valid but webview-agnostic browserName; tauri-driver ignores it.
  browserName: 'wry',
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
