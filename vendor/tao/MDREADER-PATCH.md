# Pinned Tao scene lifetime fix

This directory contains the exact crates.io `tao 0.35.3` source (archive SHA256
`d1c93047acf68669466a34690ac58cca7010bd1b201e1ec86f1fd0a75d3dd4a9`),
with one production-line change in `src/platform_impl/ios/view.rs`:
`Retained::as_ptr(&config)` becomes `Retained::autorelease_ptr(config)`.

This is upstream [tauri-apps/tao#1245](https://github.com/tauri-apps/tao/pull/1245),
commit `f2163508104413ef0609178dcebdd5803b496211`. UIKit must receive a live
autoreleased scene configuration. The iOS 27 SDK requires the scene lifecycle.
No other Tao source, dependency or platform behavior is changed. Retain the
upstream licenses. Remove this patch when the pinned Tauri runtime supports a
released Tao containing the fix.
