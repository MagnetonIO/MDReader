# Desktop configuration

Run `pnpm tauri dev` from your selected worktree. The launcher reads only public
native registration/sync settings from the main checkout's ignored `.env` and
`.env.local`, then the worktree's files in that order. Shell/CI variables take
precedence. `.env.example` lists the supported existing connector names.

For the existing Auth0/Cloudflare development account service, use
`pnpm tauri dev --sync-development`. Package a configured desktop candidate with
`pnpm tauri build --debug --sync-development`. This explicit launcher option
loads the checked-in public registration and is removed before Tauri receives
its arguments. Local/shell overrides still win; `MDREADER_NATIVE_SYNC_STAGE=disabled`
leaves sync off. It does not sign in or upload documents automatically. In
Settings → Account & sync, choose first-device setup or connect an existing
library. Use the existing library/recovery path if this account is already enrolled.
The development service is not a production sync release. Mobile and production
builds require their own complete reviewed configuration.

Existing Google Desktop client IDs and Dropbox app keys/registered redirects
are reused. The Google client secret and old tokens are not loaded. Provider
connections still require fresh system-browser consent; old keychain grants
are not imported. Dropbox defaults to read-only for the explicitly selected
folder, with editing requiring new consent. `selected_folder` registration does
not claim Dropbox's provider-enforced App Folder restriction. For an App Folder
app, paths are relative to its dedicated namespace.

Native overrides are optional and authoritative, including invalid values; a
bad override does not silently fall back to an old registration. Direct Cargo
builds require the public configuration in the environment. CI keeps these
public IDs/redirects in repository variables; it never reads a developer `.env`.
Restart/rebuild the native app after changing registration values because they
are compiled into the native owner. Starting Vite alone does not update them.

# Source-bound mobile builds

From a **clean committed checkout**, use Node/pnpm versions declared in the root
package manifest, installed Tauri mobile prerequisites, generated target project
and the coordinating agent's sole build slot:

```sh
node scripts/development/build-mobile.mjs ios --debug --target aarch64-sim --no-sign --archive-only --ci
node scripts/development/build-mobile.mjs android --debug --target x86_64 --apk --ci
```

These are local unsigned/debug examples, not release approval or signing/upload
commands. Device/distribution arguments require separate reviewed packaging setup.
The wrapper supports macOS/Linux; Windows is explicitly unsupported rather than
invoking a command shell. It runs `pnpm tauri <platform> build` with literal argv
and jobs 2. It does not run dependency installs, deploy, publish or change E2E config.

Before staging a source handoff or spawning Tauri, the wrapper rejects `true`
(case insensitive) or `1` in `CARGO_NET_OFFLINE`, `ORT_SKIP_DOWNLOAD` or
`ORT_OFFLINE`. These flags disable ort-sys's pinned runtime resolver even when
the correct archive is already cached; an APK may link yet fail to load before
app startup. Unset the flag or explicitly set it to `false`/`0`; the wrapper
preserves the supplied environment and never silently overrides it. The pinned
ort-sys 2.0.0-rc.12 resolver normally chooses the first defined flag in that
order. The wrapper deliberately rejects any disabling flag, including one
masked by an earlier false value, because nested mobile builders may filter
the environment. `false`/`0` values still pass through unchanged. `ORT_LIB_PATH`
does not exempt a mobile build: Tauri does not forward it to the nested resolver.
Direct `runMobileBuild` callers must supply these flags as strings or leave them
undefined, matching `process.env`. Defined nonstring values are rejected in the
same preflight; Node's child environment stringification cannot bypass the guard.

The wrapper verifies clean Git HEAD, rejects conflicting inherited release identity,
exclusively creates `src-tauri/.release-source-commit` with mode 0600, and exports
the same `MDREADER_RELEASE_SOURCE_COMMIT`. Nested Xcode/Gradle can lose environment
variables while build.rs reads the handoff. Explicit environment identity must be
a lowercase nonzero SHA; the handoff must be a regular file with exactly that
40-byte SHA and one LF. Both sources must agree. A malformed/present file cannot
fall back to environment or an unbound stamp. No sources retains legacy unbound
development behavior. Existing `.e2e-source-commit` behavior is unchanged.

The wrapper checks HEAD/clean state before spawning and again before success. It
does not freeze the filesystem; concurrent edits remain prohibited. Generated
Xcode project/Info.plist drift can fail the post-build check: an observed mobile
build automatically changed two owned generated files. Inspect and deliberately
review those changes; the wrapper never ignores arbitrary changes or restores
them silently. A built artifact after such failure is not verified provenance.

SIGINT/SIGTERM are forwarded to the child process group, with forced termination
after five seconds. Handoff cleanup waits for group quiescence and checks original
file identity/content. Existing, changed or replaced handoffs are never overwritten
or silently removed. If cleanup fails or SIGKILL/crash leaves a file, stop all
nested builders first, inspect its identity/content and remove it explicitly before
retrying; never clear another running build's handoff. This is coordination inside
a controlled checkout, not protection against an actively malicious same-user writer.

Focused checks (Rust compilation also requires the coordinator's slot):

```sh
node --test scripts/development/build-mobile.test.mjs
rustc --edition=2021 --test src-tauri/build_support/release_identity.rs -o /tmp/mdreader-release-identity-test
/tmp/mdreader-release-identity-test
```

The subprocess tests use temporary synthetic Git repositories and a fake `pnpm`;
they run no Cargo/Xcode/Gradle builds. They establish handoff/argv/failure/lifecycle
behavior, not a real mobile archive stamp. A subsequent real clean-HEAD Xcode
simulator archive passed the wrapper and embedded the exact commit; see the
[bounded receipt](../../docs/benchmarks/2026-10-07-ios-source-stamp-smoke.json).
Android and signed distribution archive identities still need their own checks.

Observed focused evidence: Node **4/4 passed (1.57 s)** and standalone Rust resolver
**3/3 passed (0.00 s; compile/run 0.52 s)**. The first subprocess run exposed a
real CLI-entry bug: macOS canonicalizes temporary `/var` paths to `/private/var`,
so a string-only entry-path comparison silently skipped execution. Comparing real
paths fixed that regression. The root app compiled the default absent-stamp path;
the final helper's additional Unix opened-file identity check compiled in its
standalone tests. No real mobile archive has yet verified this handoff.
