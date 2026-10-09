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

Run the portable parsing regression without Windows or a Rust build:

```sh
pwsh -NoProfile -NonInteractive -File scripts/development/windows-imports.test.ps1
```

The expected result is valid named/decorated/API-set/ordinal entries and five
refusal cases passing. Actual module loading remains Windows-runner evidence.
The runtime helper times out only its exact owned diagnostic/test children and
retains bounded reports under RUNNER_TEMP. It still fails if the actual test
harness cannot start; use the unchanged full acceptance workflow after a fix.
