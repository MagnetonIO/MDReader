# SQLite reader implementation status

Integration branches: `dev-reader-next` in MDReader and MDReader-Web. Parallel
features use separate branches/worktrees; see [ownership and cleanup](worktree-workflow.md). This is an implementation checkpoint, **not release acceptance**.
The approved scope remains [the development plan](../plans/sqlite-reader-next.md).
Original `dev` checkouts and the installed baseline are preserved. Migration
changes have not merged into `dev`.

## Current combined candidate — 2026-10-09

Native candidate `6e75b6a` combines the compact cloud sidebar, Settings-owned
cloud search selection, guided Account & sync setup and fresh read-only Drive
browsing. Source headers have a provider icon, a single-line name and a small
accessible refresh control; expanded sources show files without About text or
loaded counts. Drive folder browsing uses bounded Previous/Next pages, including
folders beyond the former 1,000-row truncation. Connecting Drive read-only starts
fresh consent; existing Picker grants are never widened silently.

The combined checkpoint passed 49 affected UI cases and TypeScript checking;
the final compact catalog changes passed 14 affected cases (six repeated,
eight additional). The Drive native connector passed eight focused cases.
Independent combined source review found no material blocker. One warm-cache,
two-job desktop build completed in 87 seconds, with embedded source identity
verified. The separate development package has a verified hardened Developer ID
signature and passed configured-account status, graceful owner shutdown and
writer-release checks in a disposable profile. It is not notarized or a release.
The open desktop remains `db47a9f`; switching to the staged package awaits the
user's normal quit. The installed `/Applications/MDReader.app` remains unchanged.

Dropbox's four entries are the connected folder's complete metadata observation
with subfolder traversal disabled, not a count of all Markdown in the account.
New sidebar presentation does not fetch document bodies automatically.

The owned iOS simulator package `69f03b5` passed all four actual runtime cases
with harness `d2d643d`: selected BOM/CRLF import, exact original-byte/revision
reads, lexical search and stale-revision refusal, pins, selected PNG/JPEG rendering,
unselected-image refusal, and restart preservation of bytes, assets, pin and dark
appearance. The run took 26 seconds without rebuilding the app. The former PNG
failure was a corrupt public fixture; `639b861` replaces it with a valid synthetic
PNG and adds a bounded preflight. Product image validation is unchanged.
This package predates the latest guided Account setup and Drive paging changes;
real mobile provider OAuth, physical-device and network-offline acceptance remain
open. Windows
ordinary-session SQLite ownership is implemented, while the full Windows
profile migration/return adapter is still on its retained major branch and has
not passed a Windows build or runtime acceptance.

Web source `9081821` is deployed to Vercel Preview `dpl_DKa8TijxLCLtzetHGMBcFEVPKz9R`
and the stable reader-next Preview alias. The unique URL passed six browser cases
with one expected registered-origin authentication skip; the stable alias passed
three cases including actual Auth0 PKCE navigation. These establish navigation,
not authenticated enrollment or encrypted transfer. The development Worker is
at version `6d74d35e-46cf-4d47-a636-6cd4e30397d0`, serving 100% of requests in
deployment `e56e3081-8612-4c91-acab-01fd8f07fbd5`. Its health, exact-origin
preflight, wrong-origin refusal and anonymous browser/native refusal passed live.
Existing SQLite Durable Object namespace, encrypted R2 bucket, public bindings
and runtime configuration are unchanged. Production readiness remains false;
the separate production configuration remains disabled.

Remaining release gates include complete installed-profile migration/return and
original parity; live fresh Drive consent/browsing and provider edit/offline
workflows; authenticated encrypted pairing/recovery/transfer; remaining real
agent hook/backfill/Windows checks; mobile provider assets/export/positions,
physical-device and browser lifecycle acceptance; authenticated live Cloudflare
sync acceptance; final branch review and signed/notarized
release packaging. Neither `dev` nor the installed baseline has been replaced.
Private receipts and disposable packages remain outside Git in
`~/Library/Caches/MDReader/reader-next-acceptance/`.

Fresh-clone setup at `1800038` passed a frozen offline pnpm install in 2.81 seconds
and one Tauri frontend/native build in 60.99 seconds, using the existing Cargo
cache with two jobs. No dependencies, `.env` files, private credentials or build
outputs were copied into the clone. The newly built executable passed actual
headless readiness, local indexing/search, exact BOM/CRLF original-byte reading,
stale-revision refusal, unchanged originals, graceful shutdown and writer-lock
release in a disposable library. Model download and optional account configuration
were absent. The probe initially assumed SHA-256 content revisions; correcting
that assumption to the actual indexed revision required no product change.
This accepts fresh-clone build/headless startup, not interactive GUI startup,
cross-host setup, signing or production distribution. Its receipt is retained in
`fresh-clone-1800038/` under the private acceptance cache.

## Earlier checkpoints

The development sync Worker now uses the verified Auth0 issuer and API access-token
contract; unauthenticated requests and exact browser-origin boundaries were checked live. The actual Vercel Preview passed offline
reading, tab contention and hostile-Markdown checks. Auth0 development registrations
now share one issuer/API audience with separate public browser/native clients;
native/browser/Worker access-token integration passed focused checks and two local
HTTPS native interoperability cases. Configured Preview deployment now passes
five deployed browser checks, including navigation to the registered Auth0 login.
The reported hidden Connect action is fixed and deployed (`5bb05b2`, receipt
`c09c0cb` in the web integration). Three focused deployed checks passed, including
200% text at desktop/mobile widths, real Auth0 navigation and offline reopen.
The separate enlarged-text empty-panel fix (`b25bc33`, receipt `5eedd56`) is also
deployed and merged. Four affected deployed checks pass in17.5 seconds. Live browser identity verification has now been observed; the user also reports
a Connected state. Authenticated encrypted transfer remains unverified. The new
standard web Settings panel is passing deployed keyboard/mobile/zoom checks;
its final shared-Preview promotion is recorded in the web receipt. These do not
establish production release acceptance.

Native desktop callback ownership is merged as `a4607b4`. It binds the exact
IPv4 loopback listener before browser dispatch, bounds HTTP bytes and login time,
and cancels on logout/replacement. Three focused callback cases, 14 affected
authority cases, one actual local HTTPS/TCP callback case, strict affected Clippy
and a production library check passed; independent source review found no blocker.
OS browser integration, persistent wrapping/bootstrap/pair/recovery and native
account UI remain separate gates. The web deployment receipt is merged as
`88abc58` in the companion integration; its Worker version is
`7e985862-cf00-449e-86ad-30b2b6245fa3`. Production was not promoted.

| Plan task | Current evidence | Remaining work |
| --- | --- | --- |
| 1. Baseline | Reproduced UI/native/provider failures repaired; deterministic baseline passed. | Live provider acceptance belongs to Task 6. |
| 2. SQLite reader | Native durable `library.sqlite`, disposable `index.sqlite`, source authority, FTS5/sqlite-vec, jobs, revision evidence and indexed links implemented. Desktop search/import/resolve delegates to a headless owner. Core schema-5 membership counts and exact provider projections are committed; source grants and trusted metadata can publish atomically. Existing device documents/import recovery/pins/positions and selected PNG/JPEG assets now use portable core owners with schema and live-owner guards. | Whole app still contains legacy CatDB paths; service integration of accepted core owner transitions, migration and host acceptance remain. |
| 3. Capture/MCP | Both visible-output adapters, private spool, durable Markdown/provenance, automatic admission/indexing, six read-only MCP tools, packaged service bootstrap and echo exclusion pass synthetic process checks. Project settings, review/apply/remove configuration and explicit selected-event backfill UI implemented. | Real isolated Claude approval/connection, Codex tool discovery/read and installed VSIX Extension Host checks now pass (receipt linked below). Packaged desktop entrypoint/hooks, desktop backfill UX/model turn, global client setup and Windows runtime remain open. |
| 4. Encrypted sync | Cross-language AES-GCM fixtures, native durable outbox/incoming receipts and browser selected Markdown/PNG/JPEG/pin/position transfer are committed. Browser account UI and bounded foreground/reconnect orchestration pass actual Next + synthetic issuer + Worker/SQLite/R2 checks, including interrupted acknowledgment and restart. Default-off native transport has bounded HTTPS, durable receipts and verified OIDC/PKCE authority with scoped custody, including actual synthetic Worker checks. | Native product integration, standard persistent wrapping/bootstrap/pair/recovery, replacement/state/deletion projection, arbitrary library/account switching, conflict-resolution UI, live issuer login, rotation/export/delete/long-offline acceptance remain. New-content incoming publication now commits original Markdown/assets, exact receipts and applied cursor atomically under session/selection/source fences; focused real SQLite/reopen/rollback tests pass. |
| 5. Clients | Real Next `/app` imports/reads/searches local Markdown with SQLite OPFS and offline shell. Local ZIP v2 backup, selected PNG/JPEG assets, pins/exact reading anchors and bounded Mermaid rendering are committed. Native mobile uses embedded SQLite; actual iOS simulator import/read/search/pin/update evidence found and fixed toolbar scrolling and container-relocation failures. Explicit bounded native PNG/JPEG selection, recovery and cleanup pass focused app/UI checks; one synthetic PNG passed actual picker/render/restart/cleanup acceptance. | Final cross-engine matrix for subsequent web features, newer mobile layout/JPEG/owner-migration device acceptance, export/share/deep links/sync, physical-device and longer lifecycle acceptance. Desktop probe controls passed; new provider composition requires a fresh packaged check. |
| 6. Providers | CatDB-free Drive/Dropbox protocols, durable registry/cursors and mutation journal; native selected OAuth/Picker, private credential custody, provider factory/coordinator/RPC and cached reader panel are implemented. Durable admission receipts prevent retired credentials from reviving sources. Local logical links preserve exact current binding/revision, distinguish ambiguity/uncached targets and fetch only on explicit request. A separate provider sidebar and explicitly selected provider sources join canonical global search, including provider-only search without a workspace. | Guarded edits; disposable live-provider OAuth/Picker/refresh acceptance. Prototype schema-1 profiles with missing admission history reject unknown credentials rather than inventing replay safety. |
| 7. Extension/migration/release | Local VS Code service adapter, readonly preview/search/reference/status and scoped process tests; inspected VSIX built. CatDB-free offline exporter preserves explicitly supplied raw snapshots and bounded typed known rows, with verified atomic publication and crash checks. Archival document-review staging, bounded CLI reads and portable exact-revision text comparison preserve records and rejection history. | Installed isolated VSIX Extension Host acceptance now passes. Archived review presentation/current-source binding is now merged; coordinated installed-profile export/rollback, CatDB removal, packaged host/release evidence and final whole-branch review remain. |

## Measured retrieval

These are synthetic local measurements, not mobile/browser or deployed-cloud
benchmarks. Reports preserve the machine/resource context and costs of fixes.

- Under the ordinary file-descriptor limit, indexing completes at 50,000 files.
  Indexed references eliminate the earlier 15-second related/linked-to timeouts.
  [Reference report](../benchmarks/2026-10-07-native-reference-v4.md).
- Ranked Related scans stored vectors exactly. At 10k documents its warm p95
  improved from 1,446.756 ms to 169.026 ms; at 50k, from 8,327.440 ms to
  730.118 ms. Narrow filtered scans pay an explicit full-row-ID iteration cost.
  [Ranked Related report](../benchmarks/2026-10-07-native-related-ranked.md).
- Actual original BGE model: a paraphrase with zero lexical matches retrieves
  the expected document; 384 dimensions and normalization checked. Four-document
  warm hybrid p95 5.55 ms. The subsequent packaged 10k receipt records warm exact p95 5.308 ms and hybrid p95 182.214 ms, with 10,000 actual passage vectors and no failed/degraded query samples. Shared-host debug-build evidence does not establish release/device acceptance. [Scale report](../benchmarks/2026-10-07-packaged-bge-scale.md).
  [Probe receipt](../benchmarks/2026-10-07-actual-bge.json).

Ranked Related distinguishes Markdown links, explicit shared tags and measured
chunk-vector similarity. Tags do not stand in for legacy concepts. Legacy
knowledge entities/concepts and optional insights remain migration gaps;
the reader reports these and missing/partial vector coverage even on empty lists.

- A short real-model balanced probe observed about 25% of one core during background indexing, compared with about 108% lifetime CPU for the older full-speed 10k benchmark. The old benchmark process has exited. Pause and full-speed overrides persist; foreground queries bypass the background budget. This is an inference admission policy, not a hard OS CPU quota; short spikes remain possible. [Resource report](../benchmarks/2026-10-07-packaged-bge-resource-controls.md).
- The resource probe found unfinished paused jobs still polling at about 1.4% CPU. Commit `2568abc` changes that wait from 20 ms to up to one second, interrupted by policy/job wakes. Two focused policy regressions pass. A separate 20-document packaged probe observed 0.199% CPU with unfinished work paused, durable pause after restart and loaded idle below the 0.13% measurement-resolution bound. Fixtures differ, so this is directional evidence rather than a controlled speedup ratio. [Follow-up receipt](../benchmarks/2026-10-07-packaged-bge-paused-idle.md).

## Evidence boundaries

Development resource policy is enforced by Cargo's two-job default and Android
Gradle's two-worker, nonparallel project configuration (`83f16f4`). The team runs
one heavyweight verification at a time and reuses compatible caches. This limits
concurrency, not CPU percentage; compilers can use internal threads. Filtered
tests can still compile many dependencies on a cold target. The ordinary
[testing policy](testing-policy.md) keeps broad checks at integration checkpoints.

Native core affected suite: 55 tests and all-target strict Clippy pass. Four focused mobile storage behaviors pass in 0.11 s, including interrupted imports and app-container relocation with the same source identity and revoked-source protection. Provider admission replay checks pass 9 provider and 5 runtime tests; native custody wrappers pass 5 focused tests. The desktop provider composition passes 7 App and 2 cached-reader dialog tests; app typecheck and scoped lint pass. These focused checks follow [the testing policy](testing-policy.md); broad native/renderer and final cross-host matrices remain integration/release gates rather than per-fix commands.

The fresh [desktop packaged composition checkpoint](../benchmarks/2026-10-07-desktop-provider-composition.md)
passes headless provider-registry access without OAuth, local admission/index/read,
original-byte preservation and saved reading line through restart. Its GUI
attachment failed before interaction, so native provider GUI acceptance remains
open. Eleven focused mobile composition tests now include a reproduced failed
read that previously displayed an indefinite loading message; an explicit error
and user-requested retry replace that state. This UI fix has not yet been rebuilt
and exercised on the simulator.

Web foreground sync checkpoint `a25a2bf` passes Chromium sync 10/10, OPFS 11/11,
configured production `/app` 1/1, default offline/tab contention 2/2 and 48 unit
tests (two existing skips), plus targeted authority/progress regressions. The
default build stays unconfigured; live issuer and deployment acceptance remain
open. Details and limitations are recorded in the web repository's
`lib/sync/FOREGROUND-SYNC.md`.

Provider link checkpoint `a8d8b18` passes 17 affected service tests and six native
wrappers. Its renderer integration passes six dialog, seven provider-panel and
seven App tests. Three local link tests exercise selected metadata and exact
current grants without network access. Metadata scans are bounded at 16,384 rows,
ambiguity at 16 choices and target input at 4 KiB; this is not an indexed link
scalability claim. Drive links support flat selected filenames, not inferred
folder hierarchy. Shared renderer anchor handling passes 12 focused tests and
prevents handled links from additionally invoking browser fragment navigation.

The mobile follow-up passes 13 composition tests plus three selected-link tests
and two shared heading-resolution tests. A delayed reading-position acknowledgment
can no longer close a newer tablet selection. Flat imported filenames support
wiki `.md` aliases, `./` relative links and decoded heading fragments; duplicate
names remain ambiguous, and qualified folder paths never fall back to a basename.
The shared heading resolver uses actual sanitized outline IDs/source lines.
An extended existing provider test reproduced previous content appearing beneath
a new selection before passive effects; rendered bodies are now bound to their
exact selection immediately. The mobile reader uses the same identity guard.
These follow-ups pass app typecheck and scoped lint; actual rebuilt device/GUI
acceptance remains open.

Provider sidebar/search checkpoint `35cccd1` passes 44 affected UI checks plus
native provider-only, mixed-ranking and stale-epoch regressions. Exact provider
source/revision evidence survives palette and Related navigation; provider hits
never become inferred local filesystem grants. Default source selection is empty.

Native transport checkpoint `7f958e9` remains disabled by default. Authority
checkpoint `de9adac` adds verified native OIDC/PKCE, ES256 request proofs, scoped
OS-vault interfaces, uncertain-write/restart reconciliation and transport-owned
401/403 invalidation. Eight focused authority regressions and one actual local
HTTPS synthetic issuer → Worker/SQLite/R2 case pass; the affected crate passes
31 ordinary tests, strict Clippy and its feature-disabled check. Late old-session
rejections cannot invalidate replacement sessions. This is not live issuer or
OS-vault runtime acceptance. Production product construction, persistent
standard wrapping/bootstrap/pair/recovery, mobile custody and typed incoming
projection remain open. Received payloads are not marked applied without a typed
projection transaction. See `crates/mdreader-sync/AUTHORITY.md`.

The rebuilt iOS simulator bundle has SHA256
`137e028186aabb54a366252084757d059cfb0b16675d395b0ffed5b655d3d7d2`.
The completed Android arm64 debug APK has SHA256
`1fc505c9fd8c1ea11d5f814cc12fec16b9d60b20dffbd32a0ad080a4ab6703f1`
and is 517,414,088 bytes. These are build-only artifacts, not new GUI/device
acceptance. Android debug size is a release-footprint investigation gate; no
release-size claim follows from it. Release source-stamp provenance has not been
verified for these artifacts. The installed baseline remains unchanged.

The [legacy migration inventory](legacy-migration-inventory.md) identifies
durable reviews, graph/operation history and accepted-write repair records.
CatDB removal requires lossless raw snapshots plus typed exports, validation and
a separate-profile rollback; it cannot discard these databases as search indexes.
The first implementation slice exports only explicitly supplied offline snapshots
and does not discover, migrate or activate an installed library.

Archival review staging `32e3996` is committed. It re-derives typed proposal rows
from the verified raw snapshot, preserves exact records/reviewer fields and
rejection tombstones, and publishes a new read-only staging database. Four
review behaviors join the affected crate's 11 passing unit tests and one CLI
process check; crate-only strict Clippy passes. Archived approval never implies
current graph membership, source authority or a verified present-day citation.
CLI checkpoint `d28e594` exposes `stage-reviews`, `list-reviews` and `get-review`
with mandatory selected export UUID/manifest hash, bounded all-or-nothing JSON
serialization and sanitized errors. One actual subprocess fixture covers stage,
replay, list/get, missing records, wrong identity, limits and unchanged staging
bytes; it passes in 0.08 seconds. The existing export/verify process check passes
in 0.04 seconds and crate-only strict Clippy passes. Output explicitly labels
archival scope, unavailable current-document evidence and unperformed binding;
raw rows remain separate. Useful reviews still need explicit current-source
binding, reader presentation, coordinated installed-profile migration and
rollback before CatDB removal.

Mobile attachment checkpoints `423c5af` (native) and `a32fada` (UI) are committed.
Ten actual app-native behaviors pass in 0.25 seconds; 13 mobile composition and
10 asset UI checks, app TypeScript and scoped lint pass. Two observed failures
were repaired: add/remove cycles could revive an older pending manifest, and a
revoked image read retained its bounded slot until expiry. Persisted generations
and immediate failed-session cleanup address them. New checks cover metadata-only
inventory/forget with revoked or changed originals, retained quota after failed
private deletion, target truncation and additive/future-schema handling. The
backend is split into focused modules with transactional versioned SQL. Actual
bindings and app command composition compile; isolated feature strict Clippy
passes. Original Markdown remains unchanged. A new iOS simulator debug bundle passed
actual separate Markdown/PNG system-picker import, rendering, restart/foreground
and Sources inventory/forget, preserving the original PNG hash. Its embedded
release source identity is zero, so release provenance remains open. Android,
JPEG/physical-device and asset-root relocation acceptance remain open.
See [mobile image contract](mobile-assets.md).

The bindings drift guard `e3ece21` runs only its two artifact-generating tests and
checks current `ui/src` paths, including staged drift. Both actual generators now
pass. The old generator was observed rewriting identical bytes and changing the
timestamp of an `include_str!` input. A test-only writer now preserves unchanged
artifacts. The actual warm drift guard passes in 1.42 seconds with generated-file
and test-binary timestamps unchanged; see [testing policy](testing-policy.md).

Vercel practices are recorded in both worktrees' `AGENTS.md`. Web commit `84e446f`
replaces stale parent-monorepo/Turborepo commands with frozen root installation
and `pnpm build`; its README describes the local SQLite reader and separate
Cloudflare sync boundary. Root commands, generated-asset build path, ignored
local secrets/config and setup links pass static checks. At that earlier
documentation checkpoint, remote settings and actual Preview acceptance were
unverified. The later October 8 deployment receipts above supersede that status.

The [actual iOS smoke receipt](../benchmarks/2026-10-07-ios-reader-smoke.md)
records standard file-picker import, exact original bytes, reading, pinning,
search and application replacement. The newest 16 px search-field adjustment
builds and installs, but its final visual focus check remains open because the
computer-use service could no longer attach to Device Hub. This does not prove
physical-device, long-document position, minimum-iOS runtime or airplane-mode
acceptance.

The new mobile API keeps original files outside the disposable index. Complete reserved copies recover only against their persisted directory identity and exact byte hash; partial/changed copies remain invisible to source retrieval, consume quota and have an explicit incomplete-copy removal flow. No directory crawl infers consent and no original selected file is removed. Actual share/deep-link/device/cloud acceptance remains open.

Packaged service receipts bind the exact binary hashes they exercised; later
source edits require rebuilding and rerunning those receipts. They use temporary
synthetic libraries, not installed profiles or private agent histories.

Configuration previews are concrete bytes and explicit apply is required. Client
capability flags are user assertions, **not measured runtime compatibility**.
Installation is distinct from an agent trusting/activating a hook or MCP server.
Owned entries are removed conservatively; changed user entries are preserved or
refused. A noncooperating editor's final compare/rename race remains documented.

Cloud resources exist only for development. The deployed infrastructure remains
fail-closed. Keys, provider credentials and agent histories are not uploaded.
Browser nonextractable CryptoKeys do not protect against same-origin script/XSS
or a compromised browser profile. Live authentication and recovery must be
accepted separately before enabling production sync.

## Latest continuation checkpoint

- `4a5e317` adds bounded native incoming preparation: exact encrypted root/chunk
  authentication and reconstruction, strict UTF-8, selected asset structure,
  typed positions and opaque tombstones. Seven focused tests and the affected
  crate (39 passed, five explicit integration cases ignored) plus strict Clippy
  pass. It cannot mutate originals or advance applied/acknowledged cursors.
  [Owner/projection gates](../../crates/mdreader-sync/INCOMING.md) remain open.
- `082f81a` compares an archived review's actual normalized cited text with an
  explicitly selected owner/source/path/revision, separating recorded/current
  and reviewed/current equality. Three meaningful regressions observed RED then
  GREEN; 14 affected unit and two CLI process tests plus strict Clippy pass.
  Native owner integration is absent; archival review, unavailable graph state
  and source-binding-required labels remain unchanged.
- [iOS image receipt](../benchmarks/2026-10-07-ios-assets-smoke.json) records the
  exact debug executable hash and observed frozen source. GUI acceptance covers
  one synthetic PNG, original preservation, persistence and quota cleanup.
  This does not prove release source identity, physical devices or whole-plan
  mobile acceptance. The system picker/control layout exposed a usability issue;
  follow-up selection-first layout has separate verification and packaging gates.
- `ead4dca` adds explicit review/enqueue/deselect of one bounded final-output JSON
  event. It never discovers or reads history/transcript paths. Selection retains
  existing root identities and exclusions; metadata deselection remains possible
  after root revocation. Exact reviewed replay recovers only its matching policy
  artifact after an interrupted SQLite save, and stale capture settings cannot
  undo a later selection change. Both regressions observed RED then GREEN.
  Capture checks (25), latest affected service checks (nine including a real
  restart process), two focused app/bindings tests, 19 affected UI tests,
  TypeScript, scoped lint and strict crate Clippy passed. Desktop GUI and actual
  supported-agent installation remain separate gates.
- `760747e` adds an exclusive source-commit handoff for nested Xcode/Gradle builds,
  with strict parsing, literal arguments, bounded cancellation and owned cleanup.
  Four actual temporary-Git/fake-builder subprocess tests and three std-only Rust
  helper tests passed. A real iOS archive embedded the correct nonzero commit,
  but the wrapper rejected generated project/plist drift. That attempt is not
  verified provenance. The reviewed output is two product-name quotes and the
  existing base plist's three macOS folder descriptions, merged automatically by
  Tauri; no new access or permission configuration was authored. The canonical
  tracked output must pass a subsequent clean-HEAD packaging check.
- That subsequent wrapper check passed from clean committed `1b6a556`:
  [source-stamp receipt](../benchmarks/2026-10-07-ios-source-stamp-smoke.json).
  The final executable embeds the exact commit; post-build source remains clean
  and the owned handoff is removed. Launch on the disposable simulator preserved
  both synthetic documents and the existing pin. This verifies unsigned debug
  simulator source identity, not signed release acceptance. New image-layout
  interaction remains unaccepted because Device Hub capture was distorted.
- `3176021` moves the existing device document/import/pin/line-state owner into
  cohesive portable core modules, preserving tables, original files, retained
  identities and disabled grants. Namespaced schema adoption validates and keeps
  populated old records, rejects future/malformed layouts and rolls back with
  its owning transaction. `7eb8a18` exposes local-source publication on a supplied
  transaction, with root/cache separation revalidation and no implicit regrant.
  [Device owner contract and limits](device-library.md) distinguish these changes
  from unfinished incoming apply and inherited recovery/cleanup limitations.
  Five owner tests, three admission tests and the affected core checkpoint
  (62 passed, manual performance case ignored) plus strict core Clippy passed.
  Future-version and post-callback cache-overlap regressions observed RED then
  GREEN. Native asset authority's schema bypass independently reproduced RED
  and passed after delegating to the shared owner. One native app compile
  (28.32 seconds) then 11 asset tests and one exact binding generator passed
  from the same artifact; generated wire types are unchanged. Scoped asset
  Clippy also passed. No broad app suite, model benchmark or second packaging
  run was used to verify this extraction; packaged owner-upgrade acceptance
  remains open.

- Portable selected assets now live in `mdreader-core::device::assets` behind
  optional `device-assets`; native code retains wire DTOs and worker scheduling.
  The old native implementation/tests are moved, with identical SQL/files/quotas,
  rather than duplicated. Errors are typed and fixed. Cached stores, prepared
  publications, snapshots and read permits are fenced to their live owner/root
  and runtime; current asset schema/root authority is revalidated. Independent
  4 MiB bounds precede allocation. Two focused ownership regressions observed
  RED then GREEN; all 13 asset tests, 62 default-core tests (manual performance
  case ignored), and strict all-target core Clippy with/without the feature pass.
  Default core and service dependency trees exclude the decoder. Lockfile changes
  only existing dependency edges, with no package/version/checksum drift.
  An actual embedded-owner/native-dispatch regression reproduced Close failure
  after asset-directory replacement, then passed after Close stopped reacquiring
  storage; both snapshot permits are released and exact copied PNG bytes remain.
  The RED app compile took 31.55 seconds and the warm fix compile 9.86 seconds;
  only that regression ran. The exact binding generator passed from the same
  final artifact; generated wire contracts are unchanged. Builds were serialized
  with two jobs; no full app suite, model benchmark or packaging run was used.
  Static independent review and scoped formatting/diff checks pass. Existing
  malformed prototype schema-adoption and pre-acquisition native Chunk retention
  limits are recorded in [the owner contract](device-library.md). Packaged upgrade,
  supplied-transaction incoming publication and live sync remain open; the
  [next bounded incoming gates](../../crates/mdreader-sync/INCOMING.md) are design,
  not applied/deployed evidence.

## October 8 isolated integration checkpoint

- [Actual client/extension receipt](../../crates/mdreader-mcp/validation/client-hosts-2026-10-08.md):
  15 affected extension checks, actual isolated Claude/Codex and installed VSIX host.
  The host found and verified a custom-URI delimiter fix. Global user configurations
  were unchanged; this is not packaged desktop entrypoint acceptance.
- [Incoming owner contract](../../crates/mdreader-service/src/sync/incoming/README.md):
  five real SQLite/filesystem fixtures, final-revalidation mutation RED then restored
  GREEN, strict feature Clippy. New content only; no plaintext identity bypass.
- [Native auth contract](../../crates/mdreader-sync/AUTHORITY.md): 11 ordinary authority
  cases, strict native/transport feature Clippy, two actual local HTTPS auth-v2
  Worker cases. Combined incoming/auth service five tests pass after integration.
  Live Auth0 and OS custody are separate gates.
- Companion web auth-v2 commit `65bf43f` passes Worker types/dry-runs, 26 actual local
  Miniflare cases, five Chromium browser scenarios, configuration/types/lint and
  the focused ID-purpose regression. Development deployment uses the exact existing
  Worker/SQLite DO/R2 resources and one Auth0 tenant; Production stays unchanged.

## October 8 core, archive and combined-tree checkpoint

Native integration `e3608e9` includes core transitions `c54b959`, archived review
presentation `fb02b38` and schema hardening `db1fc37`. The core document namespace
advances additively to version 2; this is independent of library/index versions.
Replacement publishes fresh immutable document identity and retires the old one
atomically. State changes use monotonic generation fences, including explicit
source disable/enable cycles. Retirement removes reading/search authority while
preserving exportable bytes/history. Service replacement/state/deletion wiring
is still in progress; accepted core code alone does not establish live sync.

Explicit selected archive staging now supports bounded listing, inspection and
exact-revision reader comparison. Owner identity, stage identity and current
source authority are revalidated before results publish. Weak metadata tables,
views and extra identity rows fail closed. Packaged picker/modal acceptance and
installed-profile coordinated export/activation/rollback remain open. See
[the archive validation receipt](archived-review-validation-2026-10-08.md).

After integration, the coordinator ran the warm, serialized affected checkpoint:
33 default core tests passed (0.23 seconds), five `sync-native` incoming service
cases passed (0.13 seconds), four archived review cases passed (0.22 seconds),
and strict service `sync-native --all-targets` Clippy passed (6.13 seconds).
Compilation used two jobs and the existing shared cache. No broad app suite or
model benchmark ran; final native app Clippy is still pending.

The next primary-screen slice replaces the CatDB-backed library hydration and
body-read paths with a paged reader-owned catalog, explicit coverage and current
source/index authority. A separate native custody slice adds persistent wrapping
and recovery material with standard JWE conformance. Both start from `e3608e9` in
owned worktrees; neither is accepted yet. Physical-device, live-provider,
fresh-clone, offline app-version upgrade and packaging gates remain open.

## Subsequent native lifecycle checkpoints

Incoming service v2 is merged as `bc6ca6e`: lossless ordered ledger migration,
immutable history and whole-head CAS, replacements, exact line/pin state and
content/state tombstones. Nine focused real SQLite/filesystem lifecycle cases
passed. Review caught adoption of a later reread head; the implementation now
retains the first checked snapshot and checks it at commit. The extended exact
replacement lifecycle passed after that correction (0.10 seconds), and strict
service feature Clippy passed (2.56 seconds). Tombstone/dirty/unknown-baseline
conflicts hold the cursor; state-item deletion preserves local pin/line. Native
product UI and live transport acceptance remain open.

Custody checkpoint (a), `ce9ace2`, persists independent wrapping/recovery material,
strictly preserves original v1 records and keeps empty-key pairing/recovery
candidates inactive. Two wrapping checks, eleven authority checks, explicitly run
native/browser JOSE interoperability and strict affected Clippy passed. Existing
locked package versions/checksums remain unchanged; sixteen crypto graph packages
were added. See [the custody receipt](../../crates/mdreader-sync/KEY-LIFECYCLE.md).
Bootstrap/network recovery/pairing, OS-vault host acceptance and mobile remain open.

Integration `165e180` contains both slices. The earlier combined-tree checkpoint
predates these two commits; their final combined checks remain pending the next
coherent integration gate. No installed-profile migration or live sync acceptance
is implied. The new profile-transition audit found no existing profile-wide writer
quiescence owner: swallowed save failures, bounded close draining and a surviving
reader daemon must be resolved before claiming a consistent installed backup or
activation. Originals remain preserved.

The actual deployed offline app upgrade failed safely, retaining the prior app
and synthetic library. Old open-page CSS paths returned404 on the new deployment.
A focused manifest-owned-generation fix passed its meaningful RED/GREEN checks
and independent review; its new Preview exists but old-to-new runtime acceptance
is still failing and being diagnosed. Do not count that release gate as passed.

## Catalog and enrollment integration

Integration `118a49f` includes catalog foundation `e488b5b` and native enrollment /
recovery `ba561d9`. Catalog browsing has bounded, snapshot-fenced pages and exact
reader opens; it uses the existing read-only owner, separates historical index
coverage from completed observations, and reports provider failures independently.
The obsolete second provider sidebar is removed. Focused core/service/process/
provider/UI and native binding checks passed; see [the catalog receipt](reader-catalog-validation-2026-10-08.md).
The primary legacy Library hydrate/body path is still being replaced. This
foundation is not evidence that CatDB dependencies or installed migration are gone.

Native enrollment now persists nonce-bound exact intents and uncertainty before
dispatch, reconciles lost bootstrap/recovery replies, and preserves same-material
retry history. The actual local HTTPS Worker case exercises bootstrap, recovery,
restart and revoked-device refusal. Twenty-one authority checks, two explicit
browser JOSE gates and strict affected Clippy passed. See [native enrollment](../../crates/mdreader-sync/NATIVE-ENROLLMENT.md).
Native product UI, pairing, real OS-vault and live issuer acceptance remain open.
Combined integration verification still awaits its coherent serialized checkpoint.

The deployed offline-upgrade probe additionally observed a mixed generation:
previous deployment HTML with a newer manifest. The next web fix fingerprints
build-owned HTML/worker/assets and rejects incoherent downloads while retaining
the prior cache. Focused mutation RED/GREEN checks passed; deployment acceptance
is in progress. An already loaded old client lacks the new explicit worker-update
call; tests must disclose any browser lifecycle update they invoke rather than
claim that old first-click behavior was retroactively repaired.


## Settings and native preservation checkpoint, 2026-10-08

Native primary catalog cutover `be4be70`, profile preservation foundation
`a705958`, verified pairing `a7824dd` and categorized Settings `720ef12` are
merged into reader integration. Native Settings has five standard categories,
with legacy connections under Advanced/Migration. All 26 focused Settings
cases passed; the final own-element visibility adjustment passed the two
navigation/focus cases again. Packaged native visual acceptance remains open.

Profile preservation is a guarded foundation, not installed-profile migration:
real participant writer freeze/handoff, selector startup, activation and rollback
remain unavailable until their actual owners are connected. Pairing passes its
focused authority/actual local HTTPS Worker/JOSE cases but has no shipping native
account coordinator/UI yet. CatDB removal, live provider/device acceptance and
release packaging remain open.

## Native typed sync pump integration, 2026-10-08

The typed foreground pump (`a2d17f8`) and shared owner asset-runtime constructor
(`0c4e19f`) are composed with the current Settings integration. An explicit sync
action hydrates authenticated dependencies and delegates contiguous revisions to
the existing atomic SQLite publisher; it acknowledges durable applied state only.
Cancellation drains retained blocking jobs. The native account freeze supplies
an exact-scoped participant guard, not a profile-wide preservation proof.

The combined tree passed three account cases, ten incoming SQLite/filesystem
cases and strict service `sync-native --all-targets` Clippy, using the warm cache
and two jobs. See [the integration receipt](native-sync-pump-integration-2026-10-08.md).
Actual account-actor HTTPS flow/cancellation acceptance, outgoing immutable
graphs, product document/asset discovery, all-writer preservation and release
packaging remain separate gates. No installed data or deployment changed.

## Strict native preferences startup and actual actor proof, 2026-10-08

Unix desktop now composes the real strict session/settings JSON owner at startup
and omits the plugin exit-time writer. Failed hydration cannot save defaults;
failed or timed-out close saves keep the window open with retry. The touched
persistence responsibility moved from AppShell into a small lifecycle hook.
The actual Tauri crate passed seven focused native cases and two existing binding/
command-thread gates; eighteen affected frontend cases, TypeScript and scoped
lint pass. See [the startup receipt](native-strict-persistence-2026-10-08.md).
Six other real participants remain missing; installed preservation and durable
activation/rollback are still unavailable. Native unused-code warnings are
recorded, not suppressed, and strict full native Clippy remains open.

The actual local HTTPS account-actor fixture now proves recovery/PKCE through
the real transport/vault codec and encrypted Markdown/PNG publication, pin/line
state and deletion. A cancellation mutation removing actual blocking-job drain
failed as expected; restored source passed, together with three account cases
and strict service feature Clippy. This uses synthetic identity/test-only keyring
storage, not live Auth0 or OS-vault acceptance. See
[the actor receipt](../../crates/mdreader-service/src/accounts/acceptance/README.md).

## Native reader, capture and outgoing composition, 2026-10-08

The primary device reader and strict preferences are composed and verified;
the smoke harness now resolves this standalone repository from any caller CWD.
Capture producer admission `302b0e1` and immutable outgoing graph foundation
`e02b8c9` now compose with that reader. The combined service artifact passed
eleven focused cases across capture admission/Runtime, device reads, account
lifecycle, actual local HTTPS actor and real hook subprocess. Default and
shipping service/capture strict all-target Clippy pass. See the
[combined receipt](native-capture-outgoing-composition-2026-10-08.md).

Product native document upload remains unavailable until exact selection,
durable native association and receipt-proven same-document echo are complete.
SQLite-open identity and read-only final publication fencing are separate
checkpoints in progress. Installed migration/rollback, all-writer preservation,
CatDB removal, live provider/device acceptance and packaging remain open.

## Native authority, archive access and unused mapping retirement, 2026-10-08

Read-only final publication fencing `64c5a10`, actual SQLite-open identity
`6d7e586` and canonical local/provider archived-review access `6c207271` compose
with the primary native reader. The unused 1,249-line mapping administration
module and five commands are retired; actual generated bindings shrink by 392
lines. Eleven distinct actual-app cases, two core publication cases, TypeScript
and scoped lint pass. See [the combined receipt](native-retirement-composition-2026-10-08.md).

This supersedes the prior queued archive-native check and in-progress identity/
final-fence status. It does not establish CatDB removal or release readiness.
Durable canonical history, selected native upload association/self-echo, remaining
writer preservation, installed activation/rollback, live connectors and packaging
remain open. Seven inherited Tauri warnings and strict full-native Clippy remain.

The same isolated composition includes `6076cce` bounded per-dependency receipt
validation, retaining full graph publication checks. All three affected sync
graph cases pass on the combined tree; selected native upload remains unavailable
until durable association and same-document echo acceptance.

## Native legacy import-wrapper retirement, 2026-10-08

Uncalled native model/index startup wrappers and obsolete status constructors are
removed; current import work continues through the headless service. Cloud raw
SQLite row decoding now uses small named types. Six focused actual-app cases and
the real profile outer-bundle regression pass; generated bindings are unchanged.
The actual production Clippy probe exposed broader dormant/unwired code debt, so
full-native lint remains open. See [the receipt](native-legacy-warning-retirement-2026-10-08.md).

## Selected archive final file fence, 2026-10-08

Archive snapshots now retain actual opened Markdown/asset file evidence, owner
identity and asset metadata through final caller-transaction validation. Changed
or replaced images cause provisional association rollback. Two new real-SQLite
cases, a meaningful omitted-fence RED, restored thirteen-case publication GREEN
and strict core/device-assets all-target Clippy pass. See [the core receipt](../../crates/mdreader-core/src/device/publication/README.md).
Service association/self-echo adoption and acceptance remain in progress; this
does not enable native QueueDocument RPC/UI or claim cross-device acceptance.

## Selected native association and self-echo storage, 2026-10-08

`7989ace` adopts the selected archive fence and binds an explicit native document
UUID/raw BLAKE3 revision to its immutable encrypted wire graph in one borrowed
SQLite transaction. Receipt-proven own-device content echo advances the existing
head/association/applied cursor atomically while preserving native document,
source and asset UUIDs, pins and exact line. It performs no duplicate import,
implicit selection or source regrant. Full decrypted content is compared with the
selected native snapshot outside owner locks.

Two private-seam real SQLite/filesystem lifecycle cases pass, including rollback,
reopen, future namespace refusal, exact receipt/operation authority, same-document
application and dirty-generation refusal. Omitting only the final caller fence
produced the expected RED; restored code passed all twelve affected incoming
cases and three graph cases. Strict sync/native-authority and service/sync-native
all-target Clippy pass. See [the scoped receipt](../../crates/mdreader-service/src/sync/outgoing/README.md).

Actual public IncomingCoordinator queue admission through an account actor,
authenticated outgoing network/self-echo, retained queue cancellation/freeze and
live OS-vault acceptance remain unverified. QueueDocument product RPC/UI is still
disabled. Repeated full graph proof hashing/allocation under owner access remains
an explicit responsiveness cost; transaction-bound proof reuse is follow-up work,
not a reason to weaken final archive/session fences. Restart reauthorization,
outgoing user-state/deletions, history/graph cleanup and live release remain open.

## Actual native selected-sync actor checkpoint, 2026-10-08

The public native-only coordinator now serializes explicit document/operation
UUIDs through its actual account actor, retained blocking projection lease and
existing authenticated foreground pump. The existing synthetic local HTTPS
OIDC/PKCE/Worker/SQLite/R2 fixture imports a document plus PNG through real core
APIs, uploads its immutable graph with strict scoped receipts and applies its
own echo without replacing native document/source/asset UUIDs, pins or exact line.
A pre-admission cancellation retains the actual blocking job and refuses freeze
proof; a cancellation during HTTPS upload preserves the already-admitted graph
byte-for-byte. Surrounding incoming/reauthorization/tombstone checks remain.

One scheduled TLS-child case passed; bypassing only the late authority/selection
fence caused the expected canceled-admission RED, exact source was restored, and
the same case passed. Three affected account unit cases and strict all-target
service/sync-native Clippy pass. See [the exact receipt](../../crates/mdreader-service/src/accounts/acceptance/README.md).
AccountIntent/RPC/renderer DTO/UI are unchanged; QueueDocument remains a product
integration gate. Actual OS-keychain/live issuer/mobile/installed acceptance,
old-intent reauthorization, outgoing user-state/deletions and cleanup remain open.
Repeated full graph proof owner cost is documented and not optimized in this slice.

## Profile SQLite lock and identity correction, 2026-10-08

Candidate, inventory and snapshot verification no longer open/close extra live
SQLite/recovery descriptors. Actual borrowed connection movement checks and
nofollow directory-relative stamps reject replaced handles/roots while preserving
SQLite's process locks. Candidate receipts receive a final actual-handle fence;
snapshots revalidate before and after owner-connection backup.

Five original-behavior regressions exposed lock loss or false file identity. The
final affected profile suite passes all twelve cases, including a completed
snapshot challenged by an actual competing SQLite process; strict all-target
profile Clippy passes. See [the exact receipt](profile-sqlite-lock-verification-2026-10-08.md).
This does not activate migration. Exact native SQLite/recovery classification,
all-owner freeze composition, installed preservation/conversion/rollback and
cross-host release acceptance remain open.

## Native profile storage classification, 2026-10-08

Native owner registration now distinguishes durable SQLite from rebuildable
indexes and reserves their recovery sidecars. Both preservation entry points
reject raw-copy misclassification, missing durable main files and unknown
SQLite/index/recovery claims before any participant flush. Two new actual native
regressions and the affected eleven-case profile-transition suite pass. See
[the scoped receipt](profile-storage-layout-verification-2026-10-08.md).

All actual owners must still declare their databases. Complete writer/custody
composition, cross-process backup and installed migration/activation/rollback
remain open; this change does not enable migration.

## Held service preservation control, 2026-10-08

The actual configured desktop Runtime retains its real daemon/account/device/
capture freeze guards behind an operation/owner-epoch/root/boot-bound control
session. A trusted typed client pins the existing endpoint and reconciles lost
Begin/Release replies without restarting or rebinding the service. Deadline
expiry and partial/uncertain acquisition retain guards; explicit release checks
actual producer admission and releases the daemon barrier last. Independent
provider/vault custody is not inferred from daemon drain.

Three actual TCP/SQLite fixtures pass for lost replies/ordinary-request denial,
stale identities and expiry retaining guards. A real ordinary-admission bypass
produced behavioral RED; exact restoration returned all three to GREEN. Strict
all-target service Clippy passes for default and sync-native. See
[the scoped receipt](../../crates/mdreader-service/src/preservation/README.md).
No remote database borrow, backup operation, native all-eight registration,
selector activation, installed-profile migration or release acceptance is claimed.
The actual server-owned staged backup and native owner composition are next.

## Exact selected native document product action, 2026-10-08

Native Settings now offers explicit sync for the currently opened On this device
reading receipt (document/source/raw-byte revision), scoped to the current verified
account/library/device. The production closed RPC uses the existing retained
account actor and foreground pump. Stable opaque operation identity is retained
before dispatch and across navigation/lost replies; only exact readonly same-actor
absent/applied reconciliation settles it. Storage-only Web Locks serialize
cooperating renderer writers; absent support disables upload while local reading
remains available. No automatic selection/upload/retry or renderer secret input.

The existing actual synthetic OIDC/PKCE/HTTPS Worker/SQLite/R2 case passes through
the production selected RPC, stale expected-revision rejection, strict encrypted
graph receipts, same-document echo and exact applied inspection, retaining prior
cancel/freeze/interrupted-upload assertions. Omitting selected revision equality
caused the expected RED; exact restoration returned GREEN. Accounts3/RPC1/
incoming12, native projection1, source binding generator1, UI/storage11, typecheck,
scoped lint and strict service/sync-native Clippy pass. See the
[exact commands, failures and receipt](../../crates/mdreader-service/src/accounts/acceptance/README.md#exact-selected-document-product-rpc-checkpoint-2026-10-08).

Actual OS keychain/live issuer/installed UI/mobile and cross-window WebView lock
acceptance remain separate. Old admitted graph reauthorization, outgoing
pin/position/deletion transfer, multiple-key policy and cleanup/export remain open.
This does not merge or validate later preservation/Runtime-Arc composition.


## Full migration combined candidate and installed handoff, 2026-10-08

Candidate `88a6e4e` contains the no-SDK legacy/modern conversion, usable documents
and relationships, archived history with explicit source binding, native default
selection and preservation-first rollback. Cleanup `1ae8627` removes orphan
monorepo web tooling and refreshes the profile documentation. Combined checks
passed: native offline10/startup2, legacy export16 (one explicit helper ignored),
UI7 and typecheck, generated bindings and strict Clippy. The unsigned native app
was packaged successfully.

Actual UI selected the installed profile and retained the original application
launch identity. Original legacy GUI59751 closed normally. AuthAcceptance
GUI45802 subsequently quit through its native Quit menu and released the JSON
writer lock. Its older background service supported preservation Inspect but
predated Shutdown; a new Begin did not confirm Held, so no automatic signal was
sent. The user explicitly authorized retirement of that exact leftover test
service45889. After checking its command and installed writer-lock handle, it
received ordinary SIGTERM. Actual closed-owner acquisition and import success
are still not claimed. PID4332 was a separate disposable authentication daemon,
with no installed-profile handles; its earlier termination was not closure proof.

Combined correction `9e54e37` retains native handoff identities before dispatch
across modern import/resume, dismissal and archive rollback. Explicit retry and
UI reattachment keep the same operation; pending rollback cannot become a new
import. Independent source review confirmed the handoff findings resolved.
Affected checks passed: native offline11 (2.25 seconds), import/bridge13, UI
typecheck and strict app-library Clippy. The unchanged settings surface passed14
cases at the preceding combined checkpoint. No per-feature full suites ran.
An unsigned debug acceptance package of `9e54e37` was built in59.6 seconds with
the shipping `dev.mdreader.app` identifier, without replacing the installed app.
It has not yet passed actual installed-library or Finder acceptance.

A clean clone of `bb6066e` installed frozen dependencies offline in2.4 seconds
(814 reused packages, zero downloads), compiled with two Cargo jobs and rendered
a synthetic Markdown folder in the actual native WebView. Filename search,
outline and indexing one file/two chunks worked. The actual Vercel Preview
`dpl_AvqqLESKdKnKinAySXnzcj2hm3Ri` passed public-example import, phrase search,
pin persistence after reload and standard Settings navigation on its isolated
deployment origin. Its source-commit linkage was not established by inspect;
these observations do not accept authentication or production sync.

### Installed-library acceptance and corrections, 2026-10-09

The unsigned `85160ed` package prepared and activated a durable separate profile
from the actual installed library. It produced589 readable copies and retained
the original archive and history. Most copies represent preserved metadata and
insights; this count does not mean589 original Markdown bodies. Actual reading,
normal quit/default-selection reopen, retained reading tab and pin persistence
worked. Complete original parity before activation covered38 files,19 directories
and5 opaque symlinks.

Acceptance found three concrete gaps: imported search silently selected only128
device sources, the first reopened reading request reported Busy without a retry,
and resumed return to the original required durable provenance and a separate
controller to avoid acquiring the current GUI's own lifetime lock. Closing the
migration controller also changed the original SQLite SHM ctime through a readonly
SQLite connection. Post-close original parity therefore failed; the earlier
preactivation match is not proof of completed rollback safety.

Combined candidate `6b02459` corrects these paths. Search shares a bounded1200-source
contract; its real130-source service regression passed. Busy reading has an
explicit exact-selection retry. Native return persists verified provenance and
opens a separate private controller, allowing the imported window to close
normally before explicit Retry return. Original SQLite DB/WAL/SHM/journal inputs
are copied under retained owner locks, recovered only in private directories and
revalidated by exact descriptors, stamps, hashes and present/absent membership.
Backup and committed-candidate verification use explicit guarded snapshot paths;
live-owner inode/path checks remain unchanged.

Combined verification passed13 affected native offline cases in1.93 seconds,
including original WAL/SHM fidelity after owner closure, restart, stale revisions,
assets and rollback preservation of candidate changes. The focused import and
device-reader UI cases passed13 tests; TypeScript, generated native bindings,
strict Clippy for app/profile and formatting of23 affected Rust files passed.
One heavyweight ran at a time with two Cargo jobs and the retained cache. The
unsigned shipping-identifier acceptance package built in55.05 seconds. The prior
acceptance GUI quit normally; both exact owned services closed through native
preservation drain, with released writer locks confirmed.

The new package is running, but native automation currently cannot access its
window. The user was asked to unlock/foreground it. Fresh actual import,
large-library search, default/Finder reopen, separate-controller return and
complete original parity after all handles close are still required. The installed
`/Applications/MDReader.app`, original profile and `dev`/`main` are preserved.
There is no merge to `dev`, installed replacement, release acceptance or claim
that the full CatDB migration is complete.

### Provider navigation and configured native setup, 2026-10-09

Combined source `61e914b` repairs the reported Google callback rejection, missing
Dropbox navigation, oversized imported-history rail and unavailable account setup.
Google's standard issuer parameter is accepted only for the exact Google issuer,
after the existing state and duplicate checks. Native connection summaries keep
cloud sources visible before indexing; browsing provides an explicit metadata
load action. Imported history defaults closed and the Library rail scrolls without
squeezing its navigation. Unavailable/disconnected sync hides dependent actions.
The explicit desktop `--sync-development` option selects the existing public
Auth0 registration; local/shell overrides remain authoritative.

One combined review found no concrete material defect. Focused checks passed:
20 UI cases, two launcher cases, nine OAuth cases, two native sync configuration
cases, TypeScript, scoped ESLint, strict app-library Clippy and Rust formatting.
Two WebKit Settings cases passed desktop/narrow layout and keyboard/focus checks.
These browser checks use an IPC fixture, not the native runtime.

The unsigned shipping-identifier package built in53.54 seconds. Its actual
disposable service reported configured sync and recovery availability, then
closed through preservation drain with writer-lock release confirmed. The exact
previous service for the selected migrated library also drained normally; its
replacement preserves the complete existing Dropbox source summary and Balanced
indexing policy, and reports sync configured. The new GUI was launched explicitly
against that same profile. Receipt and package are outside Git under
`~/Library/Caches/MDReader/reader-next-acceptance/provider-repairs-61e914b/`.
The two clean, integrated builder worktrees and minor branches were removed.

Native automation still fails at pipe startup. User confirmation of the rendered
sidebar/setup, live Google Picker and Auth0 enrollment/transfer remain pending.
This restart does not accept a fresh full migration or its rollback. The baseline
installed app, original profile and `dev`/`main` remain preserved.
