# Native legacy import-wrapper retirement — 2026-10-08

Base: clean integration `3d9a5f0`; isolated `refactor/native-warning-cleanup`.

Source inspection and actual production Clippy show that current native imports
queue the headless reader service. The uncalled native `build_embedder`,
`ensure_index`, `ensure_model`, `scan_count` and old loading/done constructors are
removed, with their unused ImportReport dependency. The bounded service progress
listener and its real source/epoch checks remain. Legacy migration/authority
fixtures remain available; this is not complete CatDB removal. Module docs now
describe the actual service-owned path instead of claiming a native model load.

Cloud snapshot/version pins and cached-body eviction keys now decode through two
small named raw SQLite row types. Validation, SQL, source scoping, ordering and
transaction behavior remain with the original owner. A needless borrow in the
profile bundle destination check is also corrected. No dependency, schema, API
or generated binding changes occur.

## Evidence

One heavyweight command at a time, two Cargo jobs, locked/offline, existing warm
targets; no model or broad suite. Initial production `mdreader-app --lib` strict
Clippy failed first on the profile needless borrow. After that correction it
identified 95 errors across dormant legacy paths, unwired profile participants
and three tuple-complexity warnings. That is production integration debt, not the
seven warnings observed in a test artifact. Full native Clippy remains OPEN; no
lint suppression or test-only gate hides the result.

The native test artifact compiled initially in 41.88 seconds. An incorrect test
module filter matched zero cases; it is not counted as validation. The exact
module was corrected to `cloud_documents::tests::store::`. After removing two
newly uncovered dead status constructors, the final actual native artifact
compiled in 10.06 seconds and reports two remaining legacy enum warnings:
`ConnectorUnavailable` and `CachedOffline`.

Six distinct cases passed using that one actual app artifact: incremental
metadata publication, deterministic body eviction, negative-size corruption
refusal, import coalescing, no-command-on-UI-thread and actual binding generator.
The generator produced no source drift. The real profile outer-bundle
preservation/absence/no-replace regression also passed (3.65 seconds compile,
0.12 seconds test) using the warm benchmark target. No new test duplicates a
removed unused function. `git diff --check` passes; new row-decoding modules and
touched persistence files are formatted.

No installed profile, provider original, app process, deployment or release
package changed. Remaining production lint, writer preservation, installed
migration/rollback, native outgoing association and live acceptance remain gates.
