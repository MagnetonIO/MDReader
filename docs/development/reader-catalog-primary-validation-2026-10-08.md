# Primary reader catalog cutover — 2026-10-08

Feature branch `feat/reader-catalog`, checkpoint based on integration `b2bf121`.
This replaces primary Library hydration/body selection with the reader catalog
and inline exact-revision rendering. The standalone legacy list is closed by
default and explicitly labeled **Legacy records (migration)**. No startup legacy
metadata/body query, implicit source activation, network fetch, credential access
or installed-profile mutation was used in validation.

The preserved legacy lifecycle now lives in one feature adapter. Existing
historical Recents explicitly request that adapter with their original identities;
edit receipts, tab histories, editor buffers, export and rollback paths remain.
Canonical selection synchronously retires pending legacy work. Cancellation is
advisory and rejection cannot restore a retired selection. Settings source
browsing uses a modal wrapper over the same Markdown reader/navigation body used
by the primary main column. Catalog/source changes invalidate observations;
canonical reads never construct absolute paths.

Local links use the existing service resolver through a narrow native operation.
Its origin is read at the supplied exact revision before and after resolution;
retained Library membership/root is checked before dispatch and final response.
At most 16 exact candidates are returned. Missing and ambiguity remain explicit;
no first-result guess, provider handle or invented job receipt represents a local
candidate. Provider navigation/fetch choices retain their existing implementation.

## Observed checks

- One native app artifact compiled with `CARGO_BUILD_JOBS=2`, locked/offline and
  the existing `CatDB/cargo-target-v1` cache: **41.09 s**. Merged lockfile crypto
  and Postgres dependencies required additional compilation, then owned workspace
  crates and app. No second Cargo build, dependency install or cache clean.
- Same produced executable `mdreader_app_lib-28e51066d85b91b7`:
  **2 catalog tests passed**, 0.02 s, 449 filtered. Actual SQLite/local files prove
  no implicit admission/regrant, replaced-folder rejection, exact heading line and
  target revision, scoped duplicate-title ambiguity, changed origin and revoke
  rejection. Synthetic temporary profile and Markdown only.
- Same artifact bindings generator: **1 passed**, 0.04 s, 450 filtered. Generated
  delta is the additive `CatalogReference` and Resolve request/reply.
- Meaningful RED: enable the legacy startup guard temporarily. The new primary
  startup regression observed a forbidden legacy hydrate call and failed. Restore
  explicit-only admission before GREEN checks.
- **39 distinct focused UI cases passed**: primary App composition (3), inline
  local ambiguity/history navigation (1), existing shared dialog (7), catalog
  sidebar (4), generated adapter (1), App shell (7), existing legacy identity/edit
  recovery subset (16; 24 unrelated editing cases skipped).
- Initial new cases exposed a malformed Markdown fixture link, heading-accessible
  name mismatch and an actual unhandled advisory-cancellation rejection. Fixtures
  were corrected without weakening behavior assertions; cancellation is now
  safely rejected under the existing local generation fence. Four affected new
  cases passed in 1.49 s. The legacy preservation subset passed in 1.75 s.
- App TypeScript **passed** after one test mock literal annotation; scoped ESLint
  **passed**. Final `git diff --check` passed. Later edits only expanded touched
  composition formatting and documentation; no broad test repeat.

Commands from feature root (UI commands run from `ui/`):

```sh
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/mlong/Library/Caches/CatDB/cargo-target-v1 cargo test --locked --offline -p mdreader-app --lib --no-run --message-format=json
# Reuse the executable produced above; do not invoke Cargo again after binding export:
/Users/mlong/Library/Caches/CatDB/cargo-target-v1/debug/deps/mdreader_app_lib-28e51066d85b91b7 reader_catalog:: --nocapture
/Users/mlong/Library/Caches/CatDB/cargo-target-v1/debug/deps/mdreader_app_lib-28e51066d85b91b7 tests::export_bindings_up_to_date --exact --nocapture
# ui/:
node node_modules/vitest/vitest.mjs run src/features/catalog/App.catalog.test.tsx src/features/reader/CanonicalReader.test.tsx src/shell/ProviderReaderDialog.test.tsx src/features/catalog/CatalogSidebar.test.tsx src/features/catalog/backend.test.ts src/App.test.tsx
node node_modules/vitest/vitest.mjs run src/shell/edit-integration.test.tsx -t 'library source identity follows the committed backend epoch'
node node_modules/typescript/bin/tsc --noEmit
node node_modules/eslint/bin/eslint.js src/features/catalog/ src/features/reader/ src/features/legacy-records/ src/shell/AppShell.tsx src/shell/ProviderReaderDialog.tsx src/shell/Sidebar.tsx src/shell/edit-integration.test.tsx
```

No native Clippy/full app suite, packaged GUI, installed-profile migration,
lossless export activation, remote provider or model acceptance is claimed.
Canonical views are currently volatile/read-only; persistent canonical tabs,
Recents and guarded catalog editing remain follow-up work. Existing local
workspace editing and historical legacy recovery are preserved. The CatDB SDK is
still present behind transitional adapters pending the explicit preservation and
rollback gate. The earlier [foundation receipt](reader-catalog-validation-2026-10-08.md)
records core/service pagination, publication and contention evidence separately.
