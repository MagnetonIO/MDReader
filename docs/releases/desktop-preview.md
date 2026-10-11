# Desktop installers and managed testing plan

Status: planned. No installer build, CI run, mobile service subscription, or release
publication is started by this documentation change. Current agent setup fixes
remain a separate feature batch.

## Downloads

MDReader does not have a public installer release yet. When a verified preview is
published, packages and installation instructions will appear on
[GitHub Releases](https://github.com/MagnetonIO/MDReader/releases).
Until then, use the [source build instructions](../../README.md#build-from-source).

## Build once for each platform

Freeze one reviewed desktop source commit. Use managed GitHub Actions runners,
locked dependencies, the pinned Rust toolchain, and reusable build caches. We do
not maintain local virtual machines for these builds. Run one packaging job at a
time initially, with two Cargo workers, and keep the same compiled artifacts for
testing, signing, and publication.

| Platform | First installer | Build machine | Installation target |
| --- | --- | --- | --- |
| macOS | Signed and notarized `.dmg` | GitHub macOS runner | Apple silicon and Intel Macs; verify both architectures before labeling universal |
| Windows | Signed NSIS setup `.exe` | GitHub Windows runner | Windows x64 |
| Linux | `.deb` and `.AppImage` | GitHub Ubuntu runner | Linux x64; state the tested distribution and minimum runtime requirements |

RPM, Windows ARM64, and additional Linux architectures can follow the first
desktop release. iOS and Android packages belong to a later mobile batch.

The existing stable release workflow is not ready to dispatch: it requires the
exact `origin/main` tip, and the remote repository currently has no `main` branch.
No release signing secrets are configured in GitHub at this checkpoint. The
implementation batch must align the release branch policy and configure signing
before starting distribution. Do not create a stable release tag as a shortcut.

## One desktop acceptance pass

Test the actual installed package on each supported OS using a small public
Markdown corpus and disposable libraries:

1. Install, launch, quit normally, and relaunch. Check packaged runtime libraries.
2. Add two folders; verify initial indexing and background updates in both.
3. Open documents, follow relative links and headings, and display linked assets.
4. Edit, save, reopen, and confirm the exact saved text. Export using the open
   document's filename.
5. Search by filename and content; test empty search and source selection.
6. Connect Drive and Dropbox with dedicated test accounts. Check prompt initial
   results, newest-first ordering, incremental refresh, count completion, create,
   edit/save/reopen, offline behavior, and reconnect.
7. Install an agent integration, explicitly grant selected sources, enable MCP,
   verify retrieval, then disable and uninstall it without removing user files.
8. Check upgrade and rollback in a separate profile. Preserve installed libraries
   and source originals; fresh-profile success is not upgrade acceptance.

Reuse the existing native harness where it works. Hosted macOS build runners do
not satisfy the current GUI harness's interactive privacy permissions; retain a
configured Mac acceptance machine for that step. Add platform smoke checks to the
pipeline without presenting them as the full interactive acceptance pass.

Permission, persistence, recovery, or existing-workflow failures block release.
Store the source commit, package checksums, OS versions, and bounded test evidence
with the release. Label any missing live cloud or upgrade checks explicitly.

## Managed mobile and web testing

Use **BrowserStack App Automate** as the proposed mobile testing service. Upload
compiled iOS/Android test packages and run the existing Appium scenarios on its
managed real devices. Start with one iPhone and one Android device, then expand
only when failures or supported OS versions justify it. This avoids operating a
separate device farm or virtual machine per phone platform.

BrowserStack's [AI Test Companion](https://www.browserstack.com/docs/test-companion/mobile-testing)
can use device screenshots and UI hierarchy to exercise apps and generate test
scripts. Evaluate it with one MDReader flow before committing to a paid plan.
Keep successful flows as repeatable tests with explicit save/reopen and permission
assertions. AI-generated tests need the same outcome checks as authored tests.

The mobile batch still needs its own signed builds, provider callback setup,
library transfer/recovery checks, keyboard and safe-area checks, and release
approval. A mobile testing service runs packages; it does not create a correctly
signed release from an unfinished app.

For the web reader, keep Playwright browser acceptance in the companion web
repository. BrowserStack browser sessions can extend browser/device coverage
after a small compatibility trial; they are not native desktop application tests.

References: [GitHub-managed runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners),
[BrowserStack App Automate API](https://www.browserstack.com/docs/app-automate/api-reference),
[Tauri distribution](https://v2.tauri.app/distribute/).

## Delivery order

1. Publish the source repository and product README with honest download status.
2. Finish and accept the current agent setup fixes separately.
3. Implement the desktop packaging workflow and signing setup as a separate batch.
4. Build one frozen candidate on all three desktop platforms and run the acceptance
   pass. Fix findings without restarting unrelated full checks.
5. Publish one preview release with the verified installers and installation notes.
6. Trial managed mobile testing, then plan the iOS/Android release batch.

Publishing the repository or a build plan does not mark these later steps complete.
