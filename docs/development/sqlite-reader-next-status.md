# SQLite reader implementation status

Integration branches: `dev-reader-next` in MDReader and MDReader-Web. Parallel
features use separate branches/worktrees; see [ownership and cleanup](worktree-workflow.md). This is an implementation checkpoint, **not release acceptance**.
The approved scope remains [the development plan](../plans/sqlite-reader-next.md).
Original `dev` checkouts and the installed baseline are preserved. Migration
changes have not merged into `dev`. The development sync Worker now uses the verified Auth0 issuer and API access-token
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
