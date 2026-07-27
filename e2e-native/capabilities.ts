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
  fstatSync,
  lstatSync,
  openSync,
  readFileSync,
  readSync,
} from 'node:fs';
import { dirname, isAbsolute, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { E2E_BUNDLE_IDENTIFIER, E2E_PROCESS_NAME, FLOW_CONFIG } from './flows.ts';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, '..', '..', '..');
const BUILD_MANIFEST_PREFIX = Buffer.from('MDREADER_E2E_BUILD_MANIFEST:');
const BUILD_MANIFEST_SUFFIX = Buffer.from(':MDREADER_E2E_BUILD_MANIFEST_END');
const MAX_ANDROID_NATIVE_LIBRARY_BYTES = 512 * 1024 * 1024;
const MAX_ANDROID_TOTAL_NATIVE_LIBRARY_BYTES = 1024 * 1024 * 1024;
const MAX_ANDROID_NATIVE_LIBRARIES = 64;
const MAX_ANDROID_CENTRAL_DIRECTORY_BYTES = 64 * 1024 * 1024;
const APK_MANIFEST_SCANNER = join(HERE, 'apk-manifest-scanner.mjs');

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

function readExactly(handle: number, bytes: Buffer, position: number): void {
  let offset = 0;
  while (offset < bytes.length) {
    const count = readSync(handle, bytes, offset, bytes.length - offset, position + offset);
    if (count === 0) throw new Error('APK ZIP metadata is truncated');
    offset += count;
  }
}

interface AndroidArchiveInspection {
  manifestCount: number;
  hasDex: boolean;
  nativeLibraries: Array<{ name: string; declaredSize: number }>;
}

function inspectAndroidArchive(apk: string): AndroidArchiveInspection {
  const handle = openSync(apk, 'r');
  try {
    const size = fstatSync(handle).size;
    const tailLength = Math.min(size, 65_557);
    const tail = Buffer.alloc(tailLength);
    readExactly(handle, tail, size - tailLength);
    let eocd = -1;
    for (let index = tail.length - 22; index >= 0; index -= 1) {
      if (
        tail.readUInt32LE(index) === 0x06054b50 &&
        index + 22 + tail.readUInt16LE(index + 20) === tail.length
      ) {
        eocd = index;
        break;
      }
    }
    if (eocd < 0) throw new Error('APK ZIP central directory is missing');
    const entries = tail.readUInt16LE(eocd + 10);
    const centralSize = tail.readUInt32LE(eocd + 12);
    const centralOffset = tail.readUInt32LE(eocd + 16);
    if (
      tail.readUInt16LE(eocd + 4) !== 0 ||
      tail.readUInt16LE(eocd + 6) !== 0 ||
      tail.readUInt16LE(eocd + 8) !== entries ||
      entries === 0xffff ||
      centralSize === 0xffffffff ||
      centralOffset === 0xffffffff
    ) {
      throw new Error('ZIP64 APK metadata is unsupported');
    }
    if (
      centralSize > MAX_ANDROID_CENTRAL_DIRECTORY_BYTES ||
      centralOffset + centralSize !== size - tailLength + eocd
    ) {
      throw new Error('APK ZIP central directory exceeds the artifact safety bound');
    }

    const header = Buffer.alloc(46);
    let offset = centralOffset;
    let manifestCount = 0;
    let hasDex = false;
    const nativeLibraries: AndroidArchiveInspection['nativeLibraries'] = [];
    const nativeLibraryNames = new Set<string>();
    for (let index = 0; index < entries; index += 1) {
      if (offset + header.length > centralOffset + centralSize) {
        throw new Error('APK ZIP central directory is truncated');
      }
      readExactly(handle, header, offset);
      if (header.readUInt32LE(0) !== 0x02014b50) {
        throw new Error('APK ZIP central directory is invalid');
      }
      const uncompressedSize = header.readUInt32LE(24);
      const nameLength = header.readUInt16LE(28);
      const extraLength = header.readUInt16LE(30);
      const commentLength = header.readUInt16LE(32);
      const recordLength = header.length + nameLength + extraLength + commentLength;
      if (offset + recordLength > centralOffset + centralSize || nameLength > 4096) {
        throw new Error('APK ZIP central directory entry exceeds the artifact safety bound');
      }
      const nameBytes = Buffer.alloc(nameLength);
      readExactly(handle, nameBytes, offset + header.length);
      const name = nameBytes.toString('utf8');
      if (name === 'AndroidManifest.xml') manifestCount += 1;
      if (/^classes(?:\d+)?\.dex$/.test(name)) hasDex = true;
      if (/^lib\/x86_64\/[^/]+\.so$/.test(name)) {
        if (nativeLibraryNames.has(name)) {
          throw new Error(`APK contains duplicate native library ${name}`);
        }
        if (uncompressedSize === 0xffffffff) {
          throw new Error('ZIP64 APK member is unsupported');
        }
        nativeLibraryNames.add(name);
        nativeLibraries.push({ name, declaredSize: uncompressedSize });
      }
      offset += recordLength;
    }
    if (offset !== centralOffset + centralSize) {
      throw new Error('APK ZIP central directory is invalid');
    }
    return { manifestCount, hasDex, nativeLibraries };
  } finally {
    closeSync(handle);
  }
}

function validateZipMemberBuildManifest(
  apk: string,
  member: string,
  expectedCommit: string,
  maximumBytes: number,
  remainingAggregateBytes: number,
): { manifest: E2eBuildManifest | null; streamedBytes: number } {
  const scanBytes = execFileSync(
    process.execPath,
    [
      APK_MANIFEST_SCANNER,
      apk,
      member,
      String(maximumBytes),
      String(remainingAggregateBytes),
    ],
    {
      encoding: null,
      stdio: ['ignore', 'pipe', 'pipe'],
      maxBuffer: 64 * 1024,
    },
  );
  let scan: unknown;
  try {
    scan = JSON.parse(scanBytes.toString('utf8'));
  } catch {
    throw new Error('Android native library scanner returned an invalid result');
  }
  const scanRecord =
    typeof scan === 'object' && scan !== null
      ? (scan as Record<string, unknown>)
      : null;
  const streamedBytes = scanRecord?.streamed_bytes;
  const encodedManifest = scanRecord?.manifest_base64;
  if (
    !scanRecord ||
    typeof streamedBytes !== 'number' ||
    !Number.isSafeInteger(streamedBytes) ||
    streamedBytes < 0 ||
    streamedBytes > maximumBytes ||
    streamedBytes > remainingAggregateBytes ||
    !(encodedManifest === null || typeof encodedManifest === 'string')
  ) {
    throw new Error('Android native library scanner returned an invalid result');
  }
  const manifestBytes =
    encodedManifest === null ? null : Buffer.from(encodedManifest, 'base64');
  return {
    manifest:
      manifestBytes === null
        ? null
        : validateEmbeddedBuildManifest(manifestBytes, expectedCommit),
    streamedBytes,
  };
}

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
export function validateAndroidApk(
  artifact: string | undefined,
  sourceCommit = expectedSourceCommit(),
  maximumNativeLibraryBytes = MAX_ANDROID_NATIVE_LIBRARY_BYTES,
  maximumTotalNativeLibraryBytes = MAX_ANDROID_TOTAL_NATIVE_LIBRARY_BYTES,
): string {
  if (!Number.isSafeInteger(maximumNativeLibraryBytes) || maximumNativeLibraryBytes <= 0) {
    throw new Error('Android native library safety bound must be a positive integer');
  }
  if (
    !Number.isSafeInteger(maximumTotalNativeLibraryBytes) ||
    maximumTotalNativeLibraryBytes <= 0
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
    const manifest = execFileSync('unzip', ['-p', apk, 'AndroidManifest.xml'], {
      encoding: null,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    if (
      manifest.length < 8 ||
      manifest[0] !== 0x03 ||
      manifest[1] !== 0x00 ||
      manifest[2] !== 0x08 ||
      manifest[3] !== 0x00
    ) {
      throw new Error('AndroidManifest.xml is not compiled Android binary XML');
    }
    const identifier = execFileSync(
      'apkanalyzer',
      ['manifest', 'application-id', apk],
      { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] },
    ).trim();
    if (identifier !== E2E_BUNDLE_IDENTIFIER) {
      throw new Error(
        `expected ${E2E_BUNDLE_IDENTIFIER}; found ${identifier || '(missing application id)'}`,
      );
    }
    const archive = inspectAndroidArchive(apk);
    if (archive.manifestCount !== 1) {
      throw new Error('compiled AndroidManifest.xml is missing');
    }
    if (!archive.hasDex) {
      throw new Error('compiled DEX application code is missing');
    }
    if (archive.nativeLibraries.length === 0) {
      throw new Error('x86_64 emulator native library is missing');
    }
    if (archive.nativeLibraries.length > MAX_ANDROID_NATIVE_LIBRARIES) {
      throw new Error('Android native library count exceeds the artifact safety bound');
    }
    let aggregateDeclaredBytes = 0;
    for (const library of archive.nativeLibraries) {
      if (library.declaredSize > maximumNativeLibraryBytes) {
        throw new Error('Android declared native library exceeds the artifact safety bound');
      }
      aggregateDeclaredBytes += library.declaredSize;
      if (aggregateDeclaredBytes > maximumTotalNativeLibraryBytes) {
        throw new Error(
          'Android aggregate declared native library bytes exceed the artifact safety bound',
        );
      }
    }
    let remainingAggregateBytes = maximumTotalNativeLibraryBytes;
    const manifests = archive.nativeLibraries.flatMap((library) => {
      const scan = validateZipMemberBuildManifest(
        apk,
        library.name,
        sourceCommit,
        maximumNativeLibraryBytes,
        remainingAggregateBytes,
      );
      remainingAggregateBytes -= scan.streamedBytes;
      return scan.manifest ? [scan.manifest] : [];
    });
    if (manifests.length !== 1) {
      throw new Error(`expected one native library with an E2E build manifest; found ${manifests.length}`);
    }
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
    'appium:autoWebview': false,
    'appium:includeSafariInWebviews': false,
    'appium:wdaLaunchTimeout': 180_000,
    'appium:wdaConnectionTimeout': 180_000,
  } as WebdriverIO.Capabilities;
}

/** Android — appium-uiautomator2-driver against a validated debug APK. */
export function androidCapabilitiesFor(
  artifact: string | undefined = process.env.MDREADER_ANDROID_APP,
  sourceCommit = expectedSourceCommit(),
): WebdriverIO.Capabilities {
  return {
    platformName: 'Android',
    'appium:automationName': 'UiAutomator2',
    'appium:deviceName': process.env.MDREADER_ANDROID_DEVICE ?? 'emulator-5554',
    'appium:app': validateAndroidApk(artifact, sourceCommit),
    'appium:appPackage': E2E_BUNDLE_IDENTIFIER,
    'appium:autoWebview': false,
    'appium:ensureWebviewsHavePages': true,
    'appium:nativeWebScreenshot': true,
  } as WebdriverIO.Capabilities;
}
