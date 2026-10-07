# SQLite reader implementation status

Development branches: `dev-reader-next` in isolated MDReader and MDReader-Web
worktrees. This is an implementation checkpoint, **not release acceptance**.
The approved scope remains [the development plan](../plans/sqlite-reader-next.md).
Original checkouts and the installed baseline are preserved. Nothing has merged
or published; the new sync protocol has not deployed.

| Plan task | Current evidence | Remaining work |
| --- | --- | --- |
| 1. Baseline | Reproduced UI/native/provider failures repaired; deterministic baseline passed. | Live provider acceptance belongs to Task 6. |
| 2. SQLite reader | Native durable `library.sqlite`, disposable `index.sqlite`, source authority, FTS5/sqlite-vec, jobs, revision evidence and indexed links implemented. Desktop search/import/resolve delegates to a headless owner. Core checkpoint `fe706a9`. | Whole app still contains legacy CatDB paths; transport cancellation, migration and host acceptance remain. |
| 3. Capture/MCP | Both visible-output adapters, private spool, durable Markdown/provenance, automatic admission/indexing, six read-only MCP tools, packaged service bootstrap and echo exclusion pass synthetic process checks. Project settings and review/apply/remove configuration UI implemented. | Actual supported-client installation/trust and live cross-agent acceptance; Windows capture implementation; selected backfill UI and broader echo cases. |
| 4. Encrypted sync | Cross-language AES-GCM fixture, native durable outbox/incoming receipts, browser encrypted outbox and Worker/SQLite/R2 protocol tests pass. Synthetic OIDC enrollment/pairing/recovery authority tests pass. | Browser custody/PKCE/pair UI and durable incoming projection in progress; native transport/custody, real issuer registration, rotation/export/delete/long-offline acceptance remain. |
| 5. Clients | Real Next `/app` imports/reads/searches local Markdown with SQLite OPFS and offline shell. Three-engine backend/UI checks pass. | Rendering/asset/export/quota/cache-upgrade gaps; desktop GUI acceptance; mobile SQLite/pinning/share/deep links/sync and actual device lifecycle acceptance. |
| 6. Providers | Existing revision/ETag contract regressions pass. CatDB-free provider contract and staged protocol extraction underway. | Standard selected authorization, complete provider port, owner registry/index integration, disposable live-provider acceptance. |
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
  warm hybrid p95 5.55 ms. **The 10k actual-model hybrid budget is not accepted.**
  [Probe receipt](../benchmarks/2026-10-07-actual-bge.json).

Ranked Related distinguishes Markdown links, explicit shared tags and measured
chunk-vector similarity. Tags do not stand in for legacy concepts. Legacy
knowledge entities/concepts and optional insights remain migration gaps;
the reader reports these and missing/partial vector coverage even on empty lists.

## Evidence boundaries

Native core: 27 unit + 6 reference + 4 ranked + 9 library tests pass; the ignored
low-descriptor helper is executed by its parent, and the manual performance
fixture was separately executed. Core all-target Clippy passes with warnings
denied. Latest complete renderer run: 475 tests; eight capture/integration panel
checks pass separately after subsequent installer UI wiring. Shared UI: 60 tests.

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
