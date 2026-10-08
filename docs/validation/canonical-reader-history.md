# Canonical reader history checkpoint

The normal native reader now uses an additive `session.readerHistory` namespace
for exact local, provider and published-device references. Canonical receipts
never enter legacy filesystem `DocumentRef` handlers. This is a product history
checkpoint, not the installed-library preservation/SDK-removal release gate.

## Durable contract

Version 1 bounds open tabs to 32, entries per tab to 32, and recents to 100.
Local/provider references contain the exact source, relative/generated path and
64-character revision. Device references contain the immutable document UUID,
source and revision, with no private cache path. Positions name the same revision
and actual parsed source line, block and offset. A transient selection generation
also fences late callbacks, including A → B → A selection changes.

The existing native JSON owner remains the sole session/settings writer. Ordinary
session saves send only changed legacy field groups and/or the reader namespace.
The single serial queue carries unacknowledged groups through pending, failed and
coalesced writes. Canonical activity does not serialize legacy tabs or recents.
Native merges preserve unknown raw numeric lexemes and metadata on matching rows,
including repeated exact-reference occurrences. The existing total session 1 MiB
bound remains enforced before publication. Mounting does not normalize/save the
reader namespace. Unknown/future/malformed reader state remains read-only, with a
visible diagnostic and its original bytes preserved.

Canonical tabs support selection, close/reopen, pin/reorder within their own
family, mixed-owner MRU cycling, back/forward and saved positions. Recents reopen
the exact copy and can be removed explicitly. Restoring canonical state performs
an exact backend read before applying an anchor, retains unavailable metadata,
and does not reconcile/regrant the old workspace or fall back to a latest copy.
A changed rendering layout restores the verified source line with a diagnostic;
a mismatched revision or out-of-body position is not applied.

## Focused verification

All fixtures are synthetic; no installed/private profile, model, network or GUI
was used. Existing dependency symlinks and the warm native cache were reused.

- 16 initial focused frontend cases passed across history, coalescing, positions,
  canonical reader and composed catalog reader tests.
- Removing the selection-generation check and outstanding-field accumulator
  caused exactly two expected regression failures. Both guards were restored;
  the affected tests subsequently passed.
- 77 distinct affected frontend cases passed across focused runs, including the
  existing editor module's 42 cases, persistence 7, App 7, catalog 8, device reader
  2, canonical reader 2, tabs 1, rail persistence 2, and new domain/patch/position 6.
  Two stale App Settings fixtures were corrected to navigate the actual
  categories; assertions were retained. The additive load DTO required the
  persistence fixture to assert the new native reader baseline.
- `ui/node_modules/.bin/tsc --noEmit` and scoped ESLint passed.
- Actual default-feature shipping native artifact: `CARGO_BUILD_JOBS=2
  CARGO_TARGET_DIR=/Users/mlong/Library/Caches/CatDB/cargo-target-v1 cargo test
  --locked --offline -p mdreader-app --lib profile_transition::persistence::tests`.
  **5 passed**, 461 filtered out, 0.07 s execution after a 23.46 s warm app build.
  The first compile exposed an unmatched brace in one new synthetic fixture;
  that fixture was corrected before the successful run. No bindings changed.
- Seven inherited dead-code warnings remain on this feature's older native base;
  strict native Clippy was not run or claimed. Integration's separate cleanup
  must be composed before a combined strict native check.

Reproduce the focused frontend group from `ui/` using `vitest run` with:
`src/features/reader-history`, `src/features/reader/CanonicalReader.test.tsx`,
`src/features/catalog/App.catalog.test.tsx`, `src/features/device-reader/DeviceReader.test.tsx`,
`src/shell/persistence.test.ts`, `src/shell/TabStrip.test.tsx`,
`src/shell/related-rail-persistence.test.tsx`, `src/shell/edit-integration.test.tsx`
and `src/App.test.tsx`. No full baseline suite or packaging is required for this
checkpoint. The final small recent-list CSS presentation has static review only;
packaged visual acceptance remains open.

## Remaining gates

Durable canonical history currently uses only `native_atomic` hosts. Other hosts
keep their ordinary legacy persistence behavior and canonical history is session
only; Windows durable parity remains unfinished. No new writer is substituted.

The profile-transition renderer bridge is currently unmounted. Its next scoped
integration must use the actual unsaved-editor guard and the same accumulated
field-patch baseline, omit unsupported reader namespaces, and prove the real
freeze/flush/release lifecycle. Merely registering native owners does not prove
preservation readiness. Existing-library export, useful-record mapping, rollback
and final CatDB SDK removal remain separate gates. Real packaged reader history
and position/focus/scroll acceptance also remain open.

## Combined integration gate, 2026-10-08

The coordinator subsequently checked the final actor/history integration tree
`4aa9037ad9309b4c1600f96f76ae9dda1a081278` with the actual default-feature native
artifact in the existing warm cache. The bindings generator passed **1 case in
0.04 s** and the native persistence owner passed **5 cases in 0.08 s** on that
final artifact. An initial attempt reused a stale artifact predating the source
identity/core archive exports and failed on missing exports. The coordinator
verified those exports, refreshed only the core `lib.rs` modification time, and
successfully recompiled local crates (31.53 s, followed by 20.51 s for the final
actor/bindings composition). No source or cache deletion was used. Two inherited
test warnings remained; full strict native Clippy remains open. This combined
gate does not mount the renderer preservation bridge or establish all-writer
preservation readiness.
