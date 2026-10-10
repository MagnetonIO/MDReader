# Windows desktop loader diagnostics

The full Windows provider-cache workflow requires the actual desktop test
harness to start and run at least two migration cases. A successful compile or
import probe does not satisfy that requirement.

When startup fails, the separate `Windows actual desktop loader diagnostic`
workflow reuses the same pinned toolchain, locked dependencies, Windows 2022
runner and compatible Cargo cache. It compiles only the actual desktop library
test, stages its exact linked runtime DLLs, and runs the bounded diagnostic.
Publishing an exact candidate commit to `ci/reader-windows-loader-diagnostics`
explicitly starts this diagnostic-only route. Alternatively dispatch its
workflow on a named candidate ref with matching `candidate_ref` and full
`candidate_sha`. Neither route deploys or runs migration cases.

Both Windows workflows also cache the locked ORT downloads separately from the
Rust build cache. The locked `ort-sys` downloader puts its hash-named artifacts
under the Windows LocalAppData KnownFolder in
`ort.pyke.io/dfbin/x86_64-pc-windows-msvc`, outside Cargo's target directory. A
warm Cargo cache can therefore retain link metadata while the runtime artifact
is absent. The helper checks the actual Cargo build-script record and, only for
missing exact download directories, runs `cargo clean --locked --package ort-sys`
against the disposable runner target, followed by the same locked desktop test
compile. Other cached packages remain available. The crate selects and verifies
its distribution hash; this helper supplies no download URL, guessed DLL or
system substitution. Present downloads cause no package cleanup. Empty,
foreign, ambiguous or linked artifact directories fail closed; successful cache
restoration alone is not proof that the real harness can load those artifacts.

The separate download cache is keyed by the dependency lockfile and saved after
a successful exact compilation, even if the later loader probe fails. This
repairs older warm caches without invalidating the whole Rust cache. It does
not change production model features or turn a probe into desktop acceptance.

The resulting `windows-app-test-runtime-<SHA>` artifact contains:

- Exact test/runtime hashes, imports and exports, plus real harness `--list`
  exit status before and after linked runtime staging.
- `symbol-probe.json`: each declared imported module, actual module path/hash
  loaded in a fresh diagnostic PowerShell child, and exact name/ordinal
  `GetProcAddress` result and Windows error. Loaded module export reports are
  linked from that receipt for ordinal verification.
- The child's bounded stdout, stderr and input manifest, allowing failures of
  the diagnostic itself to be distinguished from the original harness failure.

The helper changes neither PATH nor installed/system DLLs. Candidate-adjacent
DLLs are loaded by exact path with dependency lookup restricted to that
directory and System32; other declared system/API-set names use System32.
The child records modules already loaded by PowerShell. Its observed paths are
evidence about this child, not proof of what the earlier failing harness loaded.
It does not call imported functions. A nonzero ordinal address alone does not
prove the ordinal is an exported entry; inspect the correlated export report.
These limits follow the Windows [LoadLibraryExW](https://learn.microsoft.com/en-us/windows/win32/api/libloaderapi/nf-libloaderapi-loadlibraryexw)
and [GetProcAddress](https://learn.microsoft.com/en-us/windows/win32/api/libloaderapi/nf-libloaderapi-getprocaddress)
contracts.

The app's Common Controls v6 manifest is shared with library test executables.
The locked Tauri resource compiler normally links its default manifest only to
binary targets, leaving the app's library test executable without it. That
executable also imports `TaskDialogIndirect`, which requires
[Comctl32 version 6](https://learn.microsoft.com/en-us/windows/win32/api/commctrl/nf-commctrl-taskdialogindirect).
`src-tauri/build_support/windows-common-controls.manifest` preserves the exact
locked Tauri default dependency; its separate resource is linked through
`embed-resource::compile_for_everything`. Tauri still supplies the ordinary app
icon and version resources, with its duplicate manifest disabled. No elevation
request, model feature change, alternate system DLL or test-only API stub is
introduced. The actual harness must still start and the full workflow must run
the two desktop cases.

The diagnostic PowerShell child retains its own activation context. A symbol
missing from the Common Controls DLL loaded by that child does not prove that
the application, with its own embedded manifest, loads that same DLL. Read the
actual harness before/after exit status as a separate observation.

Run the portable parsing regression without Windows or a Rust build:

```sh
pwsh -NoProfile -NonInteractive -File scripts/development/windows-imports.test.ps1
pwsh -NoProfile -NonInteractive -File scripts/development/windows-ort-artifacts.test.ps1
```

The expected results are valid named/decorated/API-set/ordinal entries and five
refusal cases; exact missing/present ORT metadata cases and nine refusal cases.
The portable ORT regression does not run Cargo or mutate any existing cache.
Actual cache restore, package reconciliation, download and module loading remain
Windows-runner evidence.
The runtime helper times out only its exact owned diagnostic/test children and
retains bounded reports under RUNNER_TEMP. It still fails if the actual test
harness cannot start; use the unchanged full acceptance workflow after a fix.
