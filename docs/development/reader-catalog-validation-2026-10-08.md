# Reader catalog foundation validation — 2026-10-08

Branch `feat/reader-catalog`, based on `e3608e9`. Synthetic temporary Markdown,
SQLite profiles and provider records only; no network, model, credential vault,
installed profile or private history was used. This is a catalog foundation and
canonical read boundary, **not the primary Library/CatDB hydrate cutover**.

## Observed checks

All Cargo commands used `CARGO_BUILD_JOBS=2`, `--locked --offline`, and the existing
`/Users/mlong/Library/Caches/MDReader/benchmark-target` cache except the native
artifact described below. No dependencies/lockfile versions changed.

- Core catalog integration: **3 passed**, 0.04 s. Actual indexed evidence,
  100+1 keyset boundary, readonly continuation, reindex/regrant stale rejection,
  failed index rollback, empty/unindexed distinction, historical adoption with
  disabled grants/bookmarks/originals preserved, future/weakened schema rejection.
- Core catalog final-fence unit: **1 passed**, 0.01 s. A real source ABA and a
  real reindex commit after rows were observed both reject the pending reply.
- Meaningful RED: temporarily removed the expected-snapshot comparison; the
  reindex stale assertion failed. Restored the guard; core integration **3 GREEN**.
- Service catalog integration: **3 passed**, 0.03 s. Opaque cursor replay/stale
  handling, partial failures without admission, and a held writer mutex while
  readonly local page/list finish within the 1 s test deadline. Provider metadata
  explicitly reports Busy in that contention case; this is not a throughput claim.
- Actual no-model service subprocess: **1 passed**, 0.10 s. Canonical exact body
  read from a catalog receipt; restart invalidates volatile continuation.
- Provider catalog integration: **1 passed**, 0.05 s. Selected uncached rows,
  exact admitted cache receipt/read, cache publication stales continuation,
  revoke blocks reads, private account/key/version/body absent from catalog JSON.
  No factory was constructed. Initial failure exposed missing opaque-handle
  schema initialization on factory-free profiles; reused the existing initializer.
- Provider publication final fence: **1 passed**, 0.01 s. Actual metadata
  snapshot publication after row observation rejects the pending page.
- Core/service strict all-target Clippy: **passed**, 4.74 s after replacing the
  flagged observation tuple with a named private type. No lint suppression.
- UI: **13 distinct focused cases passed** across catalog sidebar (4), shared
  reader dialog (7), generated adapter (1), canonical provider hit guards (1).
  The first run executed 10 cases in 1.17 s; after removing the duplicate provider
  browser and moving search scope into catalog rows, the 6 affected cases passed
  in 0.71 s. Original canonical hit assertions were moved unchanged.
- App TypeScript and scoped ESLint: **passed**. Final typed adapter uses generated
  contracts, preserves typed failures and rejects wrong reply operations.
  Subsequent presentation-only formatting expanded effects/actions/JSX for review;
  TypeScript syntax parsing and `git diff --check` passed without rerunning suites.

Commands (from the feature root unless otherwise noted):

```sh
CARGO_BUILD_JOBS=2 cargo test --locked --offline -p mdreader-core --test catalog
CARGO_BUILD_JOBS=2 cargo test --locked --offline -p mdreader-core --lib catalog::tests
CARGO_BUILD_JOBS=2 cargo test --locked --offline -p mdreader-service --test catalog --test catalog_provider --test catalog_process
CARGO_BUILD_JOBS=2 cargo test --locked --offline -p mdreader-service --lib provider::catalog::tests
CARGO_BUILD_JOBS=2 cargo clippy --locked --offline -p mdreader-core -p mdreader-service --all-targets -- -D warnings
# ui/, using existing linked dependencies:
node node_modules/vitest/vitest.mjs run src/features/catalog/CatalogSidebar.test.tsx src/shell/ProviderReaderDialog.test.tsx
node node_modules/vitest/vitest.mjs run src/features/catalog/CatalogSidebar.test.tsx src/features/catalog/backend.test.ts src/shell/provider-reading.test.ts
node node_modules/typescript/bin/tsc --noEmit
node node_modules/eslint/bin/eslint.js src/features/catalog/ src/shell/AppShell.tsx src/shell/ProviderReaderDialog.tsx src/shell/ProviderReaderDialog.test.tsx src/shell/provider-reading.test.ts
```

## Native artifact and bounds

One native app test artifact was compiled using the existing compatible
`/Users/mlong/Library/Caches/CatDB/cargo-target-v1` cache:

```sh
CARGO_BUILD_JOBS=2 cargo test --locked --offline -p mdreader-app --lib --no-run --message-format=json
```

Compilation took **39.05 s**, rebuilding owned workspace crates and the native
app; no broad third-party cold build. Reused that same produced executable
`mdreader_app_lib-ea1c8dfb063947eb` directly for:

- `reader_catalog::tests::retained_library_membership_does_not_admit_regrant_or_follow_a_replaced_folder`:
  **1 passed**, 0.04 s, 449 filtered out. Existing membership alone never admits,
  disabled grant remains disabled, replaced folder does not regain authority.
- `tests::export_bindings_up_to_date`: **1 passed**, 0.06 s, same artifact.
  Generated change is exactly the new command and additive Catalog DTOs.

No full app/workspace suite, native Clippy, packaging, GUI, remote provider or
installed-profile migration acceptance is claimed. Native/GUI release acceptance
and primary hydrate/body replacement remain separate gates. Legacy bodies,
receipts, Recents/history and export/rollback paths were not removed. The former
standalone provider browser is replaced by one catalog list with explicit cloud
search selection; no duplicate browse lists are shipped by this checkpoint.
