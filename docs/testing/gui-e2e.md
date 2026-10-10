# Reader UI and native acceptance

The browser suite exercises the actual React shell and shared Markdown renderer
against synthetic Tauri IPC responses. It does not run a native window, SQLite
owner, provider, model, or installed library. A browser pass establishes rendering
and interaction behavior only.

The previous `?realengine` mode, CatDB server bootstrap, `/__catdb` Vite proxy and
`real-engine.spec.ts` have been retired. The obsolete query flag is rejected so
an old acceptance URL cannot silently receive synthetic answers. No CatDB server
or SDK is required to start the current browser suite.

## Browser rendering and interaction

From the repository root, use the pinned Node and pnpm versions:

```sh
pnpm install --frozen-lockfile
pnpm --filter @mdreader/app test:e2e
```

`ui/playwright.config.ts` starts the Vite frontend on port 1420, including the
shared-package build. It does not reuse a running frontend. Chromium and WebKit
exercise the same synthetic `ui/src/e2e/harness.ts` boundary. Playwright browser
binaries must already be available for the selected projects.

For the Settings layout and focus cases only:

```sh
pnpm --filter @mdreader/app test:settings-browser
```

A focused file/project selection uses Playwright's normal arguments:

```sh
pnpm --filter @mdreader/app test:e2e e2e/showcase.spec.ts --project chromium
```

This remains a synthetic renderer check, not fresh-clone native acceptance.
Keep results in the ignored Playwright output directories. Synthetic fixtures
must not contain private library bodies, credentials, or copied user profiles.

## Native SQLite and process boundaries

Native authority, revision fences, durable state and restart behavior are checked
separately against the reader-owned Rust components. Relevant bounded suites
include:

```sh
CARGO_BUILD_JOBS=2 cargo test --locked -p mdreader-service --test catalog_process
CARGO_BUILD_JOBS=2 cargo test --locked -p mdreader-service --test provider_runtime
CARGO_BUILD_JOBS=2 cargo test --locked -p mdreader-mcp --test stdio
```

Only one heavyweight build/check runs across the team at a time. Obtain the
coordinator's slot, keep Cargo at two jobs, and reuse the existing compatible
shared target cache; do not create a cold target or run concurrent cache writers.
Choose the affected suite rather than running all of these for every UI change.
These tests use disposable synthetic data. They do not establish actual provider
consent, platform file-picker behavior, or native GUI accessibility acceptance.

Generated command and provenance drift is checked with
`CARGO_BUILD_JOBS=2 ./scripts/check-bindings.sh`; never hand-edit the generated
TypeScript command surface.

## Packaged native GUI acceptance

A separate packaged-app checkpoint must exercise the real native window and
commands on an explicitly disposable profile. Preserve installed applications,
user libraries and unrelated running processes. Record the source/artifact
identity, target platform and actual actions completed. Native file dialogs,
provider consent, mobile sharing, OS custody and accessibility each require their
own real platform evidence; renderer tests cannot substitute for them.

`e2e-native/` contains platform harnesses outside the pnpm workspace. Their
presence is not proof a driver or platform flow currently works. Run a supported
configured harness or the approved interactive control surface, and report an
unavailable driver/control surface as an acceptance gap rather than switching to
fixture responses. Never describe a browser mock, generated package, or compiled
mobile artifact as successful native runtime acceptance.

The mobile device-library spec keeps four real WEBVIEW/native-owner cases:
selected public Markdown import and exact bytes; lexical search and stale-revision
refusal; pin plus individually selected PNG/JPEG receipts and decoded images; and
a real terminate/relaunch preserving bodies, images, pin and appearance. Reader
controls are reached through **Reader options** and its Actions, Outline,
Appearance and Details tabs. A long BOM/CRLF public handbook lets the suite check
a noninitial source block and its pixel offset across font and text-size reflow,
then verify the same computed appearance after relaunch. Navigation and setting
changes use product controls; geometry and native observations do not write a
reading position or preferences. DOM file-input selection is still distinct from
acceptance of the native OS picker.

For iOS, prepare the disposable linked checkout with
`node e2e-native/prepare-ios-project.mjs` before committing its E2E identity and
running the clean mobile build wrapper. That checkout-only Apple project must
identify `dev.mdreader.app.e2e` / `MDReader E2E`; keep the identity preparation
separate from reusable test-source commits and never merge it into a production
Apple project. The helper uses the existing YAML dependency and preserves mobile
plugins and URL handlers. No simulator or installed-profile reset is required.
