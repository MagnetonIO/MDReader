# Native categorized Settings validation

Branch `feat/reader-settings`, based on primary catalog cutover `be4be70`.

The native Settings modal now uses Appearance, Library & sources, Account & sync,
Backup & storage, and Advanced categories. Desktop uses a left navigation column;
narrow layouts use a labelled section selector. Content scrolls inside the modal.
Legacy account/CatDB controls remain explicitly available under Advanced →
Migration / legacy connections. Their handlers and retained backend lifecycle
remain unchanged. Indexing, agent capture and diagnostics are in Advanced.

The existing oversized Settings presentation was split into cohesive modules
under `ui/src/features/settings/`; each new production module is under 400 lines.
No AppShell, service, core, manifest or generated binding changes are included.
Unsupported native backup and device-sync operations are described honestly;
this UI does not create new exports, remote grants or sync capabilities.

## Focused checks

Run from `ui/`, using the existing compatible linked dependency tree:

```sh
node node_modules/vitest/vitest.mjs run src/features/settings/SettingsNavigation.test.tsx src/shell/SettingsSheet.test.tsx
node node_modules/typescript/bin/tsc --noEmit
node node_modules/eslint/bin/eslint.js src/features/settings/ src/shell/SettingsSheet.tsx src/shell/SettingsSheet.test.tsx
```

26 distinct tests passed: two category/focus lifecycle regressions and all 24
existing Settings cases. The first run passed 24 and failed two deferred legacy
cloud mutation cases because the tests opened Library instead of the moved
Advanced/Migration section. Only that navigation was corrected; both cases then
passed, retaining their successful/failed late-mutation refresh assertions.

Typecheck passed after removing an unsupported `exact` test-query option. Scoped
lint passed after removing one unused extracted type import. Only the affected
checks were repeated. `git diff --check` passed.

After review, the focus filter also checks each element's own computed display
and visibility, in addition to hidden/inert/closed-disclosure ancestors. The
existing focus case was extended with direct display:none and visibility:hidden
controls. This final narrow adjustment has not been rerun because the coordinating
agent holds the sole heavyweight-check slot. Earlier 26-test evidence precedes
that adjustment.

## Remaining acceptance

No native app build, browser/GUI run, or installed-profile mutation was performed.
Actual narrow-window and 200% zoom geometry, last-control reachability, native
focus behavior and screen-reader announcements remain runtime acceptance gates.
No backup, device-sync or legacy-profile migration acceptance is claimed.
