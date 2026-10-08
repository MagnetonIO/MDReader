# Archived-review focused verification — 2026-10-08

Scope: explicit desktop stage selection, safe archival presentation, and
transient exact current-revision text comparison. Synthetic/public data only;
no live profile, legacy user database, provider network, model or application
package run. Branch `feat/reader-archived-reviews`, base
`bce887eb879c603afe8b11d40d61c378635a8233`.

## Executed

All Cargo commands used `CARGO_BUILD_JOBS=2`, `--locked --offline`, serialized
with the team build slot and existing caches.

- Service reviews: **4 pass**, 0.19 s. Revocation/edit between initial and final
  exact read, local root/profile replacement, cached provider revision/revoke,
  preserved rejected/unknown records, operation contention, future schema and
  explicit restart reselection.
- Service subprocess: **1 pass**, 0.18 s. Actual no-model service, selected stage,
  list/get/compare, revoke, restart, stable owner scope and unchanged original
  stage/source bytes.
- Service all-target strict Clippy: **pass**, 4.32 s.
- Native cached library-test artifact: final incremental build **23.03 s**.
  Executed only the new local-generation/no-regrant case (**pass**, 0.04 s),
  existing no-index inspector unavailable case (**pass**, 0.01 s) and bindings
  generator (**pass**, 0.06 s) from that same artifact. Other app tests were not
  run. Generated contract is 47 additive lines.
- Frontend: **17 pass** (3 panel, 1 transport adapter, 6 provider reader,
  7 composed app), 1.65 s. Exact compared passage escaping, no auto selection,
  stale reply suppression, error visibility/cleanup, strict native reference
  projection and provider dialog return/inert reader are exercised.
- App TypeScript check and scoped ESLint: **pass**. Typecheck identified two
  local typing issues (promise generic and captured local path), corrected;
  affected panel/app **10 pass** on the final code, 1.40 s.
- Scoped Rust formatting and `git diff --check`: **pass**.

Commands:

```sh
CARGO_BUILD_JOBS=2 cargo test --locked --offline -p mdreader-service --lib reviews::tests
CARGO_BUILD_JOBS=2 cargo test --locked --offline -p mdreader-service --test archived_reviews_process
CARGO_BUILD_JOBS=2 cargo clippy --locked --offline -p mdreader-service --all-targets -- -D warnings
# Compile once; run the emitted test executable with each exact test filter.
CARGO_BUILD_JOBS=2 cargo test --locked --offline -p mdreader-app --lib --no-run --message-format=json
# From ui/, using the existing linked dependency tree without an install:
node node_modules/vitest/vitest.mjs run src/reviews/ArchivedReviewsPanel.test.tsx src/reviews/adapter.test.ts src/shell/ProviderReaderDialog.test.tsx src/App.test.tsx
node node_modules/typescript/bin/tsc --noEmit
node node_modules/eslint/bin/eslint.js src/reviews src/shell/AppShell.tsx src/shell/ProviderReaderDialog.tsx src/shell/ProviderReaderDialog.test.tsx src/shell/lens/InspectorRail.tsx src/App.test.tsx
```

Native exact filters:

- `reader_reviews::tests::exact_local_selection_never_regrants_and_old_same_root_context_fails`
- `overlay::tests::before_an_index_exists_the_inspector_is_unavailable_but_the_schemas_are_known`
- `tests::export_bindings_up_to_date`

## Failure evidence and limits

Temporarily removing final current-document revalidation made the authority
regression fail at the expected revoked-source assertion. Restoring the guard
produced the four passing service cases. An earlier fixture failure was only
missing private-directory permissions; the fixture was corrected before the
meaningful guard mutation. The initial native compile found a missing safe
error `Debug` implementation, corrected before the passing artifact build.

The pnpm wrapper attempted dependency reconciliation and aborted before any
install. Checks then invoked the existing Node tools directly. No new dependency
tree or lockfile upgrade was performed.

Native `--lib` strict Clippy was **stopped/incomplete** after unexpectedly
checking broad dependencies in the otherwise warm test cache; service strict
Clippy passed. This is not a native lint-pass claim. Packaged native picker and
actual desktop modal acceptance remain unrun. Windows archival support,
installed-profile export/activation/rollback and complete CatDB removal remain
open. A cached provider observation says nothing about remote-head freshness.
