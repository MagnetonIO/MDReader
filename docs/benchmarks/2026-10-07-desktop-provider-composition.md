# Desktop provider composition checkpoint

This is an ad-hoc signed macOS arm64 **debug probe**, not release or live-provider
acceptance. Original installed MDReader and private libraries remain untouched.

The native app now registers the selected-provider custody commands and starts
the same provider-enabled reader owner used by the desktop reader. Settings
opens the selected source panel; a cached selection opens the shared Markdown
renderer against its exact source/path/revision. Source evidence, explicit Back,
modal focus isolation and stale-selection rejection are covered by seven App
composition and two dialog tests. They pass; app typecheck and scoped lint pass.
Images remain unavailable in this initial provider copy, and logical links and
guarded editing remain subsequent slices.

Build command, from the isolated native worktree:

```sh
pnpm --filter @mdreader/app tauri build --debug --bundles app \
  --config /tmp/mdreader-reader-next-gui/tauri.probe.conf.json
```

The overlay uses a separate probe bundle identifier and ad-hoc signing. An
immutable bundle copy was made before further provider source edits. Its binary
SHA256 is `09fcac6a0e09233773d36c0d7af56bbb64bef9fbd34f3f0ada862ec6f1dc2ec7`
and its executable is 158,065,808 bytes. Observed source inputs are checkpoint
`feb7a2d`. The embedded release commit is forty zeroes because this debug build
did not provide `MDREADER_RELEASE_SOURCE_COMMIT`; this does **not** establish
release source provenance. The shipping release gate remains open.

The exact bundled executable ran `--mdreader-service --no-model` against a new
temporary synthetic library. Its authenticated IPC checks passed:

- Headless ping and empty provider registry work with OAuth unconfigured.
- A selected local Markdown directory admits, indexes and returns one FTS hit.
- Exact-revision read preserves UTF-8 BOM and CRLF original bytes.
- Reading line 3 persists; stopping and restarting the same bundled executable
  preserves source authority, search evidence, original bytes and saved line.
- Provider registry requests still succeed after local indexing and restart.

The [sanitized machine receipt](2026-10-07-desktop-provider-composition.json)
contains no IPC capability, credential or private file content. No live provider,
OS credential consent or provider network request was exercised.

The GUI attempt failed before attachment with `Sky Computer Use native pipe
startup failed`. Consequently this checkpoint does not claim actual packaged
Settings/Picker navigation, native OAuth or GUI acceptance. Successful component
tests and a running packaged service do not replace that gate.
