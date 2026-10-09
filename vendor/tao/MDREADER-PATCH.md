# Pinned Tao iOS lifecycle fixes

This directory contains the exact crates.io `tao 0.35.3` source (archive SHA256
`d1c93047acf68669466a34690ac58cca7010bd1b201e1ec86f1fd0a75d3dd4a9`),
with the scene lifetime change in `src/platform_impl/ios/view.rs`:
`Retained::as_ptr(&config)` becomes `Retained::autorelease_ptr(config)`.

This is upstream [tauri-apps/tao#1245](https://github.com/tauri-apps/tao/pull/1245),
commit `f2163508104413ef0609178dcebdd5803b496211`. UIKit must receive a live
autoreleased scene configuration. The iOS 27 SDK requires the scene lifecycle.

`src/platform_impl/ios/scene.rs` also delivers `connectionOptions.URLContexts`
through the same `Event::Opened` queue as warm URL opens. This addresses
[tao#1248](https://github.com/tauri-apps/tao/issues/1248), following the shared
handler approach in [tao#1257](https://github.com/tauri-apps/tao/pull/1257).
The parser skips invalid URLs and logs no URL payload. This patch covers the
custom scheme used by MDReader reading links; it does not add universal-link
registration or claim cold-start `userActivities` support.

The cold-launch getter retains an optional Objective-C result: UIKit returned
`nil` for `URLContexts` on an ordinary launch in the owned iOS 26.5 and 27
simulators. Calling the generated non-null getter aborted the app before its
reader mounted. A missing set now produces no URL event; incoming sets still
use the same parser and event queue.

No dependency versions or non-iOS behavior are changed. Retain the upstream
licenses. Remove these patches when the pinned Tauri runtime supports a released
Tao containing both fixes. Acceptance must include a packaged cold launch from
Safari, not only a pasted link or a warm app.
