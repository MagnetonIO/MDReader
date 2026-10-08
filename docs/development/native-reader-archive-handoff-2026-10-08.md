# Canonical reader archival-review handoff — 2026-10-08

Source base: integration `e6ed54f8b43184c26d2b8979cf33e517fea8f634`, fast-forwarded into
`feat/reader-device-reader`. This bounded followup changes only the existing
archival-review composition and native retained Library membership guard. It does
not retire mapping commands, migrate recents, change bindings or activate profile
migration.

## Result and authority

The primary inline local/provider reader exposes Archived reviews for its actual
loaded current reference, including a document reached through a link. Only the
exact source ID, relative path and revision enter comparison. The existing archive
panel still requires explicit native stage selection and shows the actual compared
passage; no archive locator grants access to an original file.

The inline reader stays mounted behind the modal, retaining its link history and
loaded exact body. Closing returns directly to that reader without a new body
read. A new parent selection invalidates/unmounts its archive before paint, releases
its selected archive handle, and prevents a late comparison from displaying or
restoring the previous selection. Settings-provider and legacy-workspace return
paths remain separate.

Canonical local comparisons have no active-workspace generation. The native
adapter now captures an existing retained Library membership before comparison,
then rechecks the same registry grant, actual root identity and enabled source
before returning. No Admit, source activation or absolute-path fallback is added.
The legacy local path still requires its original workspace generation and final
epoch fence. Providers retain their cached-only exact service comparison path.
DeviceReader exposes no review action: configured device-root comparison is not
implemented and no service-only device source gains Library membership here.

## Focused frontend evidence

Used existing cached dependency symlinks; no install, Cargo process, model,
packaging, private profile or GUI launch. Commands ran from `ui/`:

```sh
node node_modules/vitest/vitest.mjs run src/features/catalog/App.catalog.test.tsx -t 'canonical local archive' --reporter=dot
DEBUG_PRINT_LIMIT=500 node node_modules/vitest/vitest.mjs run src/features/catalog/App.catalog.test.tsx src/reviews/ArchivedReviewsPanel.test.tsx src/features/reader/CanonicalReader.test.tsx --reporter=dot
DEBUG_PRINT_LIMIT=500 node node_modules/vitest/vitest.mjs run src/features/catalog/App.catalog.test.tsx -t 'canonical provider archive|a new canonical selection' --reporter=dot
node node_modules/typescript/bin/tsc --noEmit
node node_modules/eslint/bin/eslint.js src/features/catalog/App.catalog.test.tsx src/reviews/useArchivedReviewSelection.ts src/shell/AppShell.tsx
```

- First regression with the handoff callback temporarily removed: expected RED,
  missing Archived reviews button (1 failed/6 skipped, 1.38 seconds). Restored the
  production callback before other checks.
- Three affected modules: 9/11 passed initially. Two new fixtures were corrected:
  provider receipts must use generated `records/<digest>.md` paths; the simulated
  external selection replacement captures its entry point before modal inertness.
- Repeated only those two corrected cases: 2 passed/5 skipped, 1.42 seconds. All
  eleven affected cases therefore passed; no repeated broad suite.
- App TypeScript and scoped ESLint passed. Final JSX readability changes were
  whitespace only; `git diff --check` passed.

The composed cases prove both local/provider linked receipts, exact comparison
arguments, no legacy/current-path lookup, inline/history return without reread,
selected-handle release and late-comparison rejection. The existing device case
also asserts absence of the archive action.

## Native acceptance — combined checkpoint

The root ran both cases successfully in its single combined actual app artifact,
together with the actual binding generator and existing no-command-UI-thread gate.
See [the combined receipt](native-retirement-composition-2026-10-08.md):

```text
reader_catalog::tests::canonical_review_membership_checks_exact_read_and_final_retained_grant_without_workspace
reader_reviews::tests::exact_local_selection_never_regrants_and_old_same_root_context_fails
```

The new native fixture uses real embedded ReaderBackend grants and exact reads,
then verifies missing membership, revocation and root replacement refusal without
an active workspace. It was authored and statically reviewed in the feature
worktree, then executed successfully in the combined native checkpoint. Native
runtime/package acceptance is not claimed by these headless checks. Installed-
library export, candidate conversion/activation and rollback remain separate
release gates.
