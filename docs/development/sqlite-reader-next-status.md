# SQLite reader implementation status

Development branches: `dev-reader-next` in isolated MDReader and MDReader-Web
worktrees. This is an implementation checkpoint, **not release acceptance**.
The approved scope remains [the development plan](../plans/sqlite-reader-next.md).
Original checkouts and the installed baseline are preserved. Nothing has merged
or published; the new sync protocol has not deployed.

| Plan task | Current evidence | Remaining work |
| --- | --- | --- |
| 1. Baseline | Reproduced UI/native/provider failures repaired; deterministic baseline passed. | Live provider acceptance belongs to Task 6. |
| 2. SQLite reader | Native durable `library.sqlite`, disposable `index.sqlite`, source authority, FTS5/sqlite-vec, jobs, revision evidence and indexed links implemented. Desktop search/import/resolve delegates to a headless owner. Core schema-5 membership counts and exact provider projections are committed; source grants and trusted metadata can publish atomically. | Whole app still contains legacy CatDB paths; transport cancellation, migration and host acceptance remain. |
| 3. Capture/MCP | Both visible-output adapters, private spool, durable Markdown/provenance, automatic admission/indexing, six read-only MCP tools, packaged service bootstrap and echo exclusion pass synthetic process checks. Project settings and review/apply/remove configuration UI implemented. | Actual supported-client installation/trust and live cross-agent acceptance; Windows capture implementation; selected backfill UI and broader echo cases. |
| 4. Encrypted sync | Cross-language AES-GCM fixtures, native durable outbox/incoming receipts and browser selected Markdown/PNG/JPEG/pin/position transfer are committed. Browser account UI and bounded foreground/reconnect orchestration pass actual Next + synthetic issuer + Worker/SQLite/R2 checks, including interrupted acknowledgment and restart. Default-off native transport has bounded HTTPS, durable receipts and injected authority, with actual synthetic Worker checks. | Native verified identity/custody and typed incoming projection, arbitrary library/account switching, conflict-resolution UI, real issuer registration, rotation/export/delete/long-offline acceptance remain. |
| 5. Clients | Real Next `/app` imports/reads/searches local Markdown with SQLite OPFS and offline shell. Local ZIP v2 backup, selected PNG/JPEG assets, pins/exact reading anchors and bounded Mermaid rendering are committed. Native mobile uses embedded SQLite; actual iOS simulator import/read/search/pin/update evidence found and fixed toolbar scrolling and container-relocation failures. | Final cross-engine matrix for subsequent web features, native/mobile assets/export/share/deep links/sync, physical-device and longer lifecycle acceptance. Desktop probe controls passed; new provider composition requires a fresh packaged check. |
| 6. Providers | CatDB-free Drive/Dropbox protocols, durable registry/cursors and mutation journal; native selected OAuth/Picker, private credential custody, provider factory/coordinator/RPC and cached reader panel are implemented. Durable admission receipts prevent retired credentials from reviving sources. Local logical links preserve exact current binding/revision, distinguish ambiguity/uncached targets and fetch only on explicit request. A separate provider sidebar and explicitly selected provider sources join canonical global search, including provider-only search without a workspace. | Guarded edits; disposable live-provider OAuth/Picker/refresh acceptance. Prototype schema-1 profiles with missing admission history reject unknown credentials rather than inventing replay safety. |
| 7. Extension/migration/release | Local VS Code service adapter, readonly preview/search/reference/status and scoped process tests; inspected VSIX built. CatDB-free offline exporter preserves explicitly supplied raw snapshots and bounded typed known rows, with verified atomic publication and crash checks. | Actual extension-host acceptance; useful insights/reviews staging and current-source binding, coordinated installed-profile export/rollback, CatDB removal, host/release evidence and final whole-branch review. |

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

Native transport checkpoint `7f958e9` remains disabled by default and has no
production authority constructor or renderer RPC. Focused tests include actual
local HTTPS requests to the synthetic Worker/SQLite/R2 fixture. Received payloads
are not marked applied without a typed projection transaction; native custody,
production enrollment/recovery and live issuer acceptance remain open.

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
Useful reviews still need explicit current-source binding, reader presentation,
coordinated installed-profile migration and rollback before CatDB removal.

Mobile attachment work is currently uncommitted. Its isolated native harness
passes seven affected behaviors in 0.17 seconds, including two observed failures
now fixed: add/remove cycles could revive an older pending manifest, and a
revoked image read retained its bounded slot until expiry. Persisted manifest
generations and immediate failed-session cleanup address them. PNG/JPEG copies
remain private, explicitly selected and revision-bound; no source folder is
inferred. New inventory/forget cleanup, app command composition, generated
bindings, renderer integration and actual mobile acceptance are not yet verified.

The bindings drift guard `e3ece21` now runs only its two artifact-generating
tests and checks the current `ui/src` paths, including staged drift. Lightweight
dispatch and real temporary-Git checks pass without launching Cargo. The next
native integration checkpoint must still exercise both actual generators.

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
