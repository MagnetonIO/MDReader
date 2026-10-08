# Native reader retirement composition — 2026-10-08

Base: native integration `e6ed54f8b43184c26d2b8979cf33e517fea8f634`.
The isolated `refactor/retire-mapping-admin` branch composes read-only final
publication fencing `64c5a10`, actual SQLite-open identity `6d7e586` and canonical
archived-review handoff `6c207271` before removing unused mapping administration.

## Result

The obsolete 1,249-line native mapping module and its five desktop/mobile
commands are removed. Source searches found no remaining UI/package/native-E2E
callers. The actual Tauri binding generator removes 392 generated TypeScript
lines. This retires one unused CatDB administration surface; the remaining legacy
engine, overlay, federation and cloud paths and machine-local SDK dependency
remain release blockers.

Canonical local/provider reading retains access to archived reviews using its
exact loaded reference and retained Library grant. Device archive access remains
unavailable until its own membership contract exists. Publication fencing and
SQLite identity are composed with the existing owner; this does not establish
profile-wide writer quiescence or installed migration safety.

## Combined verification

All Cargo checks used two jobs, locked/offline dependencies and existing warm
caches. Only one heavyweight check ran at a time across the team.

The actual app compiled in 43.48 seconds using
`CARGO_TARGET_DIR=/Users/mlong/Library/Caches/CatDB/cargo-target-v1`:

```sh
CARGO_BUILD_JOBS=2 cargo test --locked --offline -p mdreader-app --lib \
  reader_catalog::tests::canonical_review_membership_checks_exact_read_and_final_retained_grant_without_workspace \
  -- --exact --nocapture
```

That case passed. The same emitted `mdreader_app_lib-a92063a7fbc15c69` artifact
then passed the existing exact-local archival authority case, seven
`profile_transition::` cases, the no-command-on-UI-thread gate and the actual
binding generator: eleven distinct native cases in total. A second invocation of
the same binding case confirmed no further generated drift; it is not counted
as another distinct case and did not recompile. The other unchanged provenance
binding generator was not rerun.

Seven inherited legacy unused-code warnings remain; none were suppressed.
Strict Clippy of the entire Tauri crate remains a separate unfinished gate.

Two composed core `device::publication::tests::current_fence::` cases passed
with `device-assets` enabled using the benchmark warm target (4.60 seconds
compile, 0.09 seconds tests). No full suite or model benchmark was run.

Combined UI TypeScript and scoped ESLint passed using existing dependency
binaries:

```sh
./node_modules/.bin/tsc --noEmit --incremental false
./node_modules/.bin/eslint src/reviews/useArchivedReviewSelection.ts \
  src/features/catalog/App.catalog.test.tsx src/shell/AppShell.tsx
```

A preliminary `pnpm exec` refused its automatic module-purge prompt in a
noninteractive shell; no installation or dependency change occurred. Direct
existing binaries completed the checks. Temporary dependency symlinks were
removed before commit; generated Tauri schema output is not source.

The archive feature's eleven affected frontend cases and meaningful callback
mutation are recorded in its own receipt. The SQLite identity checkpoint also
records real ABA and cross-process POSIX lock regression mutations. This
composition does not repeat those suites unnecessarily. `git diff --check`
passes.

## Remaining acceptance

No installed profile, provider originals, live Auth0 registration, deployment,
OS vault, native app launch, package or physical device changed in this check.
Typed durable canonical history, exact native outgoing association/self-echo,
ReaderDaemon and remaining writer preservation, installed migration/export/
rollback, live connectors and release packaging remain open. The actual stable
web preview sign-in is separately verified and is not native sync acceptance.
