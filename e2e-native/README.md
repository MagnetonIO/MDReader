# Native reader acceptance

Run from a named disposable worktree based on the integration candidate. Use
Node 24, the repository's declared pnpm version, pinned Rust toolchain and native
Tauri prerequisites. iOS requires Xcode and an available arm64 iOS Simulator;
Android requires the SDK, an x86_64 emulator and its matching WebView driver.
Keep the single native build slot across agents.

```sh
pnpm install --frozen-lockfile
export MDREADER_SOURCE_COMMIT="$(git rev-parse HEAD)"
```

The separate Appium/WDIO dependencies live in this directory. A complete
committed toolchain lock is still pending; `npm install --prefix e2e-native`
resolves the current declared versions and is not a frozen-install receipt.
The mobile Appium drivers are package dependencies. Appium is started locally
by the WDIO service, with one session at a time.

Build mobile artifacts with `node e2e-native/build-mobile-e2e.mjs ios` or
`node e2e-native/build-mobile-e2e.mjs android` after initializing the appropriate
E2E project in the disposable worktree. The initialization and production-project
preservation steps are in `.github/workflows/tauri.yml`. These wrappers bind the
artifact to the selected source commit and E2E app identity. Use the artifact
from that exact worktree, rather than an old app in another build cache.

## iOS Simulator

List available devices and choose a simulator reserved for this run:

```sh
xcrun simctl list devices available
export MDREADER_IOS_UDID=YOUR-SIMULATOR-UUID
export MDREADER_IOS_APP="$PWD/src-tauri/gen/apple/build/mdreader-app_iOS.xcarchive/Products/Applications/MDReader E2E.app"
xcrun simctl boot "$MDREADER_IOS_UDID"
xcrun simctl bootstatus "$MDREADER_IOS_UDID" -b
node --experimental-strip-types e2e-native/validate-artifact.ts ios "$MDREADER_IOS_APP" "$MDREADER_SOURCE_COMMIT"
npm --prefix e2e-native run test:ios
```

Skip `boot` if this exact simulator is already booted. Optional
`MDREADER_IOS_DEVICE` and `MDREADER_IOS_VERSION` describe that simulator; the
explicit UDID selects it. The app validator requires the arm64 Simulator ABI,
`dev.mdreader.app.e2e` and the exact source manifest. A physical-device build
requires separate provisioning and acceptance; this configuration refuses it.

## Android emulator

Start your disposable x86_64 AVD, then use its exact serial from `adb devices`:

```sh
adb devices
export MDREADER_ANDROID_UDID=emulator-5554
export MDREADER_ANDROID_APP=/ABSOLUTE/PATH/TO/CURRENT-E2E.apk
adb -s "$MDREADER_ANDROID_UDID" shell getprop sys.boot_completed
node --experimental-strip-types e2e-native/validate-artifact.ts android "$MDREADER_ANDROID_APP" "$MDREADER_SOURCE_COMMIT"
npm --prefix e2e-native run test:android
```

Readiness requires `sys.boot_completed` to return `1`. Missing, malformed or
`auto` UDIDs fail closed. The harness does not fall back to device names or
another attached device. The Android artifact must contain the E2E package,
x86_64 app library and exact source manifest. Physical-device APK acceptance
is a separate gate.

## What the run verifies

The mobile spec imports public Markdown through the real DOM file input,
checks exact native SQLite receipts and stale-revision refusal, performs text
search, selects only the specified PNG and JPEG, checks their exact leased
bytes and Blob decoding, then verifies those assets, original Markdown, pin
and appearance after native termination/relaunch. The 3×2 JPEG is a committed
synthetic solid-color fixture; no image-generation dependency runs in tests.

This is local lifecycle acceptance. OS picker interaction, radio-off behavior,
cloud sign-in/sync and physical devices require separate real runs. Adding
assertions or passing TypeScript does not establish simulator/Android acceptance.

WDIO owns its Appium service. Stop only the simulator/emulator reserved for
your run afterward (`xcrun simctl shutdown "$MDREADER_IOS_UDID"` or
`adb -s "$MDREADER_ANDROID_UDID" emu kill`). Preserve failing-run logs and the
exact artifact/source identity; never dump native tokens or private libraries.
Before removing the clean temporary worktree, preserve any setup files you
need. A source-manifest mismatch means the artifact must be rebuilt from the
selected candidate, rather than relaxing the guard.

References: [XCUITest capabilities](https://appium.github.io/appium-xcuitest-driver/latest/reference/capabilities/),
[UiAutomator2 capabilities](https://github.com/appium/appium-uiautomator2-driver).
