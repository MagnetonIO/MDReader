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
| 4. Encrypted sync | Cross-language AES-GCM fixture, native durable outbox/incoming receipts, browser encrypted outbox and Worker/SQLite/R2 protocol tests pass. Synthetic OIDC enrollment/pairing/recovery authority tests pass. | Browser custody/PKCE/pair/recovery UI and durable incoming projection are implemented with synthetic issuer checks. Native transport/custody, real issuer registration, selected asset/user-state transfer, rotation/export/delete/long-offline acceptance remain. |
| 5. Clients | Real Next `/app` imports/reads/searches local Markdown with SQLite OPFS and offline shell. Local ZIP v2 backup, selected PNG/JPEG assets, pins/exact reading anchors and bounded Mermaid rendering are committed; current affected Chromium checks pass 11 backend and 14 actual Next UI cases. Native mobile now ships an account-free embedded SQLite import/read/search/pin/position shell with durable interrupted-import recovery. | Final cross-engine matrix for subsequent web features, encrypted asset/user-state transfer, native/mobile assets/export/share/deep links/sync and actual device lifecycle acceptance. Desktop probe controls passed; broader host acceptance remains. |
| 6. Providers | CatDB-free Drive/Dropbox protocols, durable registry/cursors, mutation journal and exact revision-gated core projections pass synthetic checks. | Native standard selected OAuth/Picker, vault/factory/coordinator/RPC and shipping sidebar integration in progress; disposable live-provider acceptance remains. |
| 7. Extension/migration/release | Local VS Code service adapter, readonly preview/search/reference/status and scoped process tests; inspected VSIX built. | Actual extension-host acceptance; useful insights/reviews migration, lossless export/rollback, CatDB removal, host/release evidence and final whole-branch review. |

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

Native core affected suite: 55 tests and all-target strict Clippy pass. The latest local native mobile checkpoint runs only three behavioral tests plus binding export (four passed, 0.12 s); these cover original bytes, index loss, revocation, bounded imports, atomic grant publication, interrupted-copy recovery and quota reservations. The affected mobile/controls UI checks pass 11 tests; desktop integration checks pass 25 and shared search passes 21. App typecheck and scoped lint pass. These focused checks follow [the testing policy](testing-policy.md); broad native/renderer and final cross-host matrices remain integration/release gates rather than per-fix commands.

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
