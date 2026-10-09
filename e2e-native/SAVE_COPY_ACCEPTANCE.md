# Disposable macOS Save-a-copy acceptance

Coordinator-only native setup. The helper creates a new marked private temp root,
imports one fixed public Markdown through `device::import`, verifies its actual
read/search, closes its writer and returns metadata. It never discovers profiles,
starts/stops processes, cleans existing directories, reads credentials, or changes the
shipping app. Do not use `desktop-fixture.ts` or its termination helper here.

From the reviewed integration checkout, create the fixture (core-only warm build):

```sh
cargo run -p mdreader-core --features device-assets --example save_copy_fixture -- --create-disposable-public-fixture
```

Keep the returned JSON as the receipt. `profileRoot` is the only allowed profile;
do not substitute an existing library. Build the separate E2E app once:

```sh
mdreader_copy_sha="$(git rev-parse HEAD)"
MDREADER_SOURCE_COMMIT="$mdreader_copy_sha" MDREADER_RELEASE_SOURCE_COMMIT="$mdreader_copy_sha" pnpm tauri build --features e2e-build --config src-tauri/tauri.e2e.conf.json --bundles app
```

Use the resulting signed **MDReader E2E.app**, identifier `dev.mdreader.app.e2e`.
The normal `dev.mdreader.app` bundle is prohibited: even `--reader-profile` opens
its default selection controller. For this E2E app, set the returned values:

```sh
MDREADER_E2E_PROFILE_ROOT="$mdreader_copy_root" MDREADER_E2E_CORPUS_ROOT="$mdreader_copy_corpus" MDREADER_E2E_SOURCE_ID=00000000-0000-0000-0000-00000000e2e0 "$mdreader_copy_binary"
```

Here `mdreader_copy_root`, `mdreader_copy_corpus` and `mdreader_copy_binary` are
explicit coordinator variables populated from the receipt and verified bundle.
No profile CLI arguments: the E2E root guard refuses conflicting selection.
It requires this bundle identity, exact marker, private mode, and an immediate
`mdreader-e2e-*` directory under the canonical OS temporary root. Local corpus
membership is test-only; device reading/editor/export use actual shipping APIs.
If avoiding model downloads, start that exact bundle's service in a separate
coordinator-owned terminal before the GUI with
`"$mdreader_copy_binary" --mdreader-service --no-model "$mdreader_copy_root/reader-v1"`.
Close the GUI normally; use the native owner's bound drain/shutdown protocol for
the exact service. The helper offers no process lifecycle or cleanup operation.

In the actual app, open **On this device → public-save-copy.md → Edit**. Replace
the full editor buffer with the public text in `expectedDraft` (exact LF ending).
Do not choose Save changes. Exercise **Save a copy…** in this order:

1. Cancel the native dialog; check the editor still shows its dirty draft.
2. Choose `collisionDestination`, confirming macOS's existing-file choice if
   needed. The native exclusive create must refuse; check the draft remains.
3. Choose the previously absent `copyDestination`; check confirmation and that
   the draft remains unsaved in the source editor.

After each step, a filesystem-only verifier can run while the GUI owns SQLite:

```sh
cargo run -p mdreader-core --features device-assets --example save_copy_fixture -- --verify "$mdreader_copy_root" cancelled
cargo run -p mdreader-core --features device-assets --example save_copy_fixture -- --verify "$mdreader_copy_root" collision
cargo run -p mdreader-core --features device-assets --example save_copy_fixture -- --verify "$mdreader_copy_root" saved
```

Run each command after its matching step, not all at the end. The verifier reads
only bounded fixed public fixture files under the marked temp root, never opens
SQLite or changes bytes, and checks source/owned copy/collision stay exact while
the new copy matches the expected draft hash. It prints hashes/booleans only.
It cannot prove native dialog presentation or dirty-buffer retention; record
those separately from actual UI observation. This accepts the device editor
path only; provider authentication/grants, original-file saves, mobile/web export,
and production-profile startup remain outside this synthetic check.
