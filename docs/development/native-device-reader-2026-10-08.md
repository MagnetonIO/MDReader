# Native device reader checkpoint — 2026-10-08

Status: focused core/service/native/UI validation passed. This is not packaged
desktop, end-to-end sync, backup or release acceptance.

Owner: `feat/reader-device-reader` in `MDReader-device-reader`, based on `49e21b4`,
advanced to Settings `2632677` and merged with reviewed pump/runtime composition
`3f197ca`. The constructor seam is consumed from its committed branch; no WIP
source or private client configuration was copied.

## Product and authority boundary

The canonical Sources catalog adds **On this device**, containing already
published, enabled, nonretired device documents. Listing does not index, recover,
initialize a namespace, contact sync or create a directory. A device receipt
contains its actual document UUID, source ID and immutable body revision. It is
not a local-folder grant, provider handle or inferred private filename.

Selecting a receipt opens the existing normal inline Markdown reader. Exact
device reads and selected PNG/JPEG leases run through the same desktop service
owner as incoming publication. Native composition supplies the actual document
and asset cache roots. The original child grants remain authoritative, including
historical records without synthesized parent-intent history. Reads verify the
retained child identity and exact configured parent-path membership before and
after reading. Revoked sources, retired documents, changed revisions and replaced
roots fail closed.

The core read-only namespace observer distinguishes absent, known original,
current, future and malformed document layouts without adoption. Attachment
`open_existing` validates the exact existing tables/indexes, version, root metadata
and retained directory; it never creates or upgrades them. Existing write-side
mobile initialization remains separate.

## Bounded observations and leases

Device metadata uses UUID keyset pages of at most 100 plus one continuation row,
under the existing 1,000-document quota. The opaque service cursor retains its
first snapshot. That snapshot binds the actual retained query owner, bounded
document/source-epoch metadata, same-connection `total_changes` and `data_version`.
Every final observation and continuation must match the first snapshot. SQLite
documents comparison of `data_version` only on the same connection; this design
conservatively returns stale even for unrelated library commits. It does not use
numeric ordering or promise a clean-host browsing latency. See [SQLite's pragma
contract](https://www.sqlite.org/pragma.html#pragma_data_version).

The native read-only command supports read, manifest, open, chunk and close.
Replies include exact revision evidence; no paths, credentials or custody handles
cross to the renderer. Hash verification runs outside the SQLite owner mutex.
The account/incoming and device adapters share one `AssetRuntime`: at most two
4 MiB image snapshots, 64 KiB chunks and the existing 15-second lease expiry.
Grant epochs fence image leases against revoke/re-enable ABA. The UI reuses the
bounded image controller, loads selected visible images lazily and retires Blob
URLs on selection change/offscreen/unmount. Unselected and remote images remain
placeholders in this reader. Image import/editing and relative document links are
not added by this slice.

## Preservation participant

Native-only `FrozenDevices` denies this coordinator's device-read and device-
catalog admission, drains actual in-flight work with a maximum 30-second deadline,
and closes only leases it issued. Other shared-runtime producers remain separate
participants. The guard binds owner, query connection, runtime, actual roots,
first namespace snapshot, operation, epoch and deadline; a failed drain produces
no guard. It does not freeze account/capture/provider/writer participants or prove
that a backup is ready. Parent preservation composition must freeze those actual
participants independently and retain/revalidate all guards through export.

## Focused validation observed

- Core catalog: **2 passed** (6.59 s warm build, 0.08 s tests). Actual
  read-only SQLite observations cover first/final and continuation fences, grant
  ABA, exact reads and absent/original/future/malformed namespaces. Temporarily
  omitting the final snapshot equality made the committed-state regression fail;
  restoring the guard passed it again (0.08 s). The mutation is not retained.
- Affected core assets: **14 passed**, including new strict no-create/no-adoption
  reopening (1.84 s rebuild, 0.42 s tests). The initial run found an old test
  fixture that changed the Markdown revision but retained an obsolete byte count;
  the fixture now updates both, preserving exact body-size validation.
- Service device: **3 passed** (10.62 s warm build, 0.09 s tests): actual synthetic
  incoming publication through catalog/body/PNG, shared permits and scoped
  freeze/timeout, read-only writer contention, configured-root/future-schema
  rejection and local availability despite invalid account configuration.
- Strict affected core/service library and test Clippy: **passed** (5.88 s).
- Shipping native input/projection: **1 passed** after a 44.78 s warm native
  artifact build. The same fresh artifact ran the actual binding generator:
  **1 passed** (0.05 s). Seven inherited legacy dead-code warnings remain; native
  strict Clippy was not run for this slice.
- UI: **21 affected cases passed** across device reader (2), composed catalog
  app (4), catalog sidebar (4), canonical reader (1) and image controller (10).
  Initial new-test failures exposed an observer mock that selected the heading
  observer and widened mock reply types; both fixtures were corrected, then only
  the affected device cases were repeated. App typecheck and scoped ESLint passed.

Commands ran from this owned worktree, serialized under the assigned slot:

```sh
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/benchmark-target cargo test --locked --offline -p mdreader-core --features device-assets device::catalog::tests -- --nocapture
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/benchmark-target cargo test --locked --offline -p mdreader-core --features device-assets --lib device::assets:: -- --nocapture
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/benchmark-target cargo test --locked --offline -p mdreader-service --features sync-native --lib device::tests -- --nocapture
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/benchmark-target cargo clippy --locked --offline -p mdreader-core -p mdreader-service --features mdreader-core/device-assets,mdreader-service/sync-native --lib --tests -- -D warnings
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/mlong/Library/Caches/CatDB/cargo-target-v1 cargo test --locked --offline -p mdreader-app --lib reader_device::tests::closed_read_inputs_and_projection_do_not_expose_paths_or_native_handles -- --exact
/Users/mlong/Library/Caches/CatDB/cargo-target-v1/debug/deps/mdreader_app_lib-801a27b795f60576 tests::export_bindings_up_to_date --exact --nocapture
```

From `ui/`, using the existing linked dependency cache without installation:

```sh
node node_modules/vitest/vitest.mjs run src/features/device-reader/DeviceReader.test.tsx src/features/catalog/App.catalog.test.tsx src/features/catalog/CatalogSidebar.test.tsx src/features/reader/CanonicalReader.test.tsx src/mobile/device-assets.test.tsx
node node_modules/typescript/bin/tsc --noEmit
node node_modules/eslint/bin/eslint.js src/features/device-reader src/features/catalog/App.catalog.test.tsx src/features/catalog/CatalogSidebar.tsx src/features/catalog/backend.ts src/features/reader/CanonicalReader.tsx src/features/reader/contracts.ts src/shell/AppShell.tsx
```

The `pnpm` launcher attempted dependency-tree repair and aborted before running
tests; direct cached Node entry points avoided any install. No model inference,
network, installed profile or private source was used. No package/lock changes.
Actual packaged desktop/device and end-to-end sync acceptance remain separate
gates. Relative device-document links, image import/editing and preservation
composition with all other producers are not implemented by this slice.

## Combined strict-preferences/device checkpoint

The normal merge of native integration `5429193` produced clean source HEAD
`6d8aa1d98f56a77f1050ff634715e0837deda0fc`. No merge conflicts occurred.
AppShell retained both the extracted persistence lifecycle and the exact inline
device route; native collectors retained preferences and device commands.

One combined native artifact was compiled using the existing CatDB target cache.
The first invocation failed because the shared cache reused a core artifact from
another worktree that lacked this branch's device APIs. Cargo's dependency paths
were relative and its cached input timestamps were newer than the branch's
unchanged inputs. Refreshing only `crates/mdreader-core/src/lib.rs`'s timestamp
forced the correct core compilation, without changing bytes, cleaning the cache
or changing dependencies. The coherent warm build then completed in **34.60 s**,
with the same seven inherited legacy dead-code warnings. Shared cache consumers
must not infer source provenance from an artifact filename alone.

Observed on the freshly built combined artifact:

- `profile_transition::`: **7 passed** (0.24 s).
- Exact device input/projection regression: **1 passed**.
- Existing UI-thread structural check: **1 passed** (0.28 s). The new device
  command is independently declared async and delegates blocking IPC off-thread.
- Actual binding generator: **1 passed** (0.04 s), with **no generated delta**.
- Combined UI: **57 affected cases passed**: editor integration 42, persistence
  7, related-rail persistence 2, device reader 2 and composed catalog app 4.
  The first run passed 48 and failed nine older editor fixtures: the newly
  mounted catalog/provider boundaries were unmocked, and Settings source actions
  were queried while their category was hidden. The fixture now supplies empty
  read-only boundary replies and explicitly navigates Library & sources. All 42
  editor cases then passed (5.49 s total), including failed hydration, native
  close, preference failure and unsaved-editor behavior. No product errors or
  assertions were suppressed; no blanket alert filtering was added.
- Combined app typecheck and scoped ESLint: **passed**.

Native commands, serialized with two Cargo jobs:

```sh
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/Users/mlong/Library/Caches/CatDB/cargo-target-v1 cargo test --locked --offline -p mdreader-app --lib profile_transition:: -- --nocapture
/Users/mlong/Library/Caches/CatDB/cargo-target-v1/debug/deps/mdreader_app_lib-a92063a7fbc15c69 reader_device::tests::closed_read_inputs_and_projection_do_not_expose_paths_or_native_handles --exact --nocapture
/Users/mlong/Library/Caches/CatDB/cargo-target-v1/debug/deps/mdreader_app_lib-a92063a7fbc15c69 tests::no_command_runs_on_the_ui_thread --exact --nocapture
/Users/mlong/Library/Caches/CatDB/cargo-target-v1/debug/deps/mdreader_app_lib-a92063a7fbc15c69 tests::export_bindings_up_to_date --exact --nocapture
```

The UI run used existing cached Node entry points from `ui/`:

```sh
node node_modules/vitest/vitest.mjs run src/shell/persistence.test.ts src/shell/edit-integration.test.tsx src/shell/related-rail-persistence.test.tsx src/features/device-reader/DeviceReader.test.tsx src/features/catalog/App.catalog.test.tsx
DEBUG_PRINT_LIMIT=500 node node_modules/vitest/vitest.mjs run src/shell/edit-integration.test.tsx
node node_modules/typescript/bin/tsc --noEmit
node node_modules/eslint/bin/eslint.js src/shell/AppShell.tsx src/shell/use-app-persistence.ts src/shell/persistence.ts src/shell/persistence-lifecycle.ts src/shell/profile-transition-bridge.ts src/shell/legacy-persistence.ts src/shell/persistence.test.ts src/shell/edit-integration.test.tsx src/shell/related-rail-persistence.test.tsx src/features/device-reader src/features/catalog/App.catalog.test.tsx src/features/catalog/CatalogSidebar.tsx src/features/catalog/backend.ts src/features/reader/CanonicalReader.tsx src/features/reader/contracts.ts
```

No model, network, packaging, installed profile or full native suite was used.
The previously listed packaged, sync and preservation composition gates remain.
