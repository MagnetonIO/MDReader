# Approved MDReader development plan — SQLite throughout

Approved by the user with “Implement the plan” on 2026-10-07. This document records the SQLite plan displayed in the conversation and supersedes the earlier external Turso comparison.

## Product contract and locked decisions

An offline-first Markdown reader for desktop (macOS/Windows/Linux), web, iOS and Android, with automatic Claude Code/Codex artifact capture, read-only MCP retrieval, and optional encrypted cloud sync in the first release. Preserve reading/editing/navigation/search/operational features and useful optional document insights/reviews. Retire generic CatDB schema/mapping/law/enterprise administration; export/migrate user-authored records losslessly. Local reading and capture work without an account. No canvas/chat/collaboration/plugin marketplace/agent write tools in v1.

- SQLite everywhere: native `rusqlite`, browser SQLite WASM/OPFS, cloud SQLite-backed Durable Objects. No Turso/libSQL/PowerSync/PostgreSQL.
- Native desktop/mobile retain Tauri and a shared Rust reader core. Web evolves the existing Next.js/Cloudflare Worker repository. Shared reader packages are versioned artifacts owned by MDReader.
- Native `library.sqlite` is durable configuration, captures/jobs, user state, recovery, outbox/cursors. Separate `index.sqlite` holds rebuildable document/headings/links/chunks/FTS5/`sqlite-vec` data. Original files/provider bodies and durable Markdown captures remain content authority.
- One desktop Rust service owns writes/model state; app, VS Code and stdio MCP delegate to it. Mobile embeds the core in its lifecycle. Web uses a worker with `opfs-sahpool`, one database owner, and cross-tab coordination.
- Heading-aware chunks, exact/semantic/hybrid/recent/related retrieval, bounded source filters, cancellation, freshness and revision/line evidence. Preserve `bge-small-en-v1.5` (384-dimensional) tokenizer/pooling/normalization/query-prefix contract. Local ONNX embeddings; lexical/read operations remain available on model failure.
- Cloud: existing Cloudflare Worker, one SQLite-backed Durable Object per account, R2 encrypted bodies/assets. OIDC/PKCE login with deployment-configured issuer/client IDs; authenticated account/device scope. AES-256-GCM versioned envelopes; library keys transferred only through verified device pairing or exported recovery key. No cloud plaintext content or server content search.
- Sync selected Markdown/captures/assets/bookmarks/saved searches/reading state. Do not sync live database files, indexes, models, raw agent histories or provider credentials. Durable local outbox, idempotent operations, blob upload before atomic revision publication, explicit acknowledgments, conflict preservation, tombstones/checkpoints, device revocation, export/delete/recovery.
- Agent hooks enqueue bounded durable events, return promptly, and never request continuation for capture. Keep source/project/worktree/session/turn/subagent/revision provenance. Suppress duplicates/inherited histories/MCP echoes. Explicit project opt-in and historical backfill selection; hidden reasoning/raw tool logs excluded.
- Local VS Code extension host first; remote/web extension hosts explicitly unsupported in v1. Mobile is reading and reading-state focused; foreground/reconnect sync with platform-budgeted background work.
- Implement only in new `dev-reader-next` branches/worktrees for MDReader and MDReader-Web. Preserve current checkouts, existing web generated-file changes and installed baseline. Do not merge/deploy/publish without release evidence and required authorization.

## Shared interfaces and data flow

MDReader-owned contracts: source registry (select/list/reconnect/disable/forget/health), document provider (read/list/change/resolve assets/guarded write), search (scoped bounded evidence/cancel/freshness), capture (durable enqueue/dedupe/status), sync (blobs/revision commit/change feed/ack/devices), MCP (`list_sources`, `index_status`, `search`, `read_document`, `related`, `resolve_reference`). Generate transport types once; platform/OAuth/CatDB types must not enter the contracts.

Claude plan → durable local Markdown capture → lexical/vector index → Codex MCP retrieval → optional encrypted selected-library sync → offline phone/browser reading → reading-state return. Capturing/indexing a plan never constitutes user approval.

## Execution tasks

### Task 1: Phase 0 — Isolate work and repair baseline

Inputs: reviewed MDReader `f350339e87dd5bbf9329ab3edf423ea109a460f4`, web `5bf52cb85f86bafdb21592f89453d5baeebc6020`, external review/validation evidence.

Repair test aliases and stale monorepo paths, local-connector compilation, reproduced UI/native/provider regressions. Diagnose observed Drive read failure without changing private files/provider authority. Preserve/fix checks rather than delete failures. Record migration fixtures and reproducible commands. Existing failing checks serve as RED evidence; add focused behavioral regressions for uncovered defects before implementation.

Verify: UI/Markdown/shared package tests and typecheck; Rust local/domain/ingest/presentation/app and provider contract tests; relevant web checks. Expected: green deterministic baseline; any live external gate remains explicitly unaccepted rather than misrepresented. Commit baseline fixes and ledger each result.

Produces: reliable checks and reusable source/provider behavior for Task 2; legacy-state evidence for Task 7.

### Task 2: Phase 1 — Extract SQLite reader core

Create CatDB-free MDReader source/document/search/evidence contracts. Implement durable `library.sqlite` and derived `index.sqlite`, migrations, FTS5/vector search, heading-aware indexing, link resolution, jobs/cursors, model/degraded status. Preserve scanner/path safety, cancellation, source grants, cache revisions and guarded file writes. Integrate vertical paths into the shipped app; do not leave a parallel unused core or permanent dual writes. Version shared packages and verify native/mobile/browser builds early.

Write failing integration tests for real SQLite persistence/reopen/rebuild/search/evidence/scope/conflict behavior; implement, run focused tests then affected suites. Expected: reader behavior works without CatDB dependencies, with durable user state intact. Commit verified slices.

Produces: document/index service and portable contracts consumed by Tasks 3–7.

### Task 3: Phase 2 — Automatic capture and MCP

Implement opt-in Claude Code/Codex capture adapters and durable event pipeline, Markdown projections/provenance/deduplication, lifecycle diagnostics and recovery. Implement shared headless service and read-only stdio MCP tools using Task 2. Preserve existing agent configurations during install/uninstall and keep private sessions untouched by tests.

RED→GREEN synthetic process/integration tests: both agent directions, reader window closed, restart, duplicates/forks/subagents/exclusions/model unavailable/queue failure/tool echo suppression. Expected: exact revision-bound outputs retrievable without manual import and bounded scope. Commit verified slices.

### Task 4: Phase 3 — Encrypted cloud sync

Implement client durable outbox and E2EE envelopes/device pairing/recovery; web repository Worker auth/API, per-account SQLite Durable Object and R2 storage. Upload blob before revision publication, enforce account/device scope and operation idempotency, return explicit durable acknowledgments. Preserve conflicting bodies, tombstones and long-offline recovery. Implement selected content/assets/user-state transfer, export/delete/revoke/restore. No server plaintext/search or provider-token replication.

RED→GREEN native/web cross-language encryption fixtures and real Worker/SQLite integration tests: offline roundtrip, interrupted uploads, replay, conflicts/deletes/account switching, wrong-device access, tamper, pairing/recovery, revocation. Expected: encrypted selected library sync works through the real production API; live deployment/identity credentials remain separate acceptance gates. Commit verified slices.

### Task 5: Phase 4 — Complete desktop/mobile/web clients

Integrate private reader library, browser worker persistence/search/offline cache/edit recovery, iOS/Android touch reading/navigation/search/source evidence/deep links/share/open-file/explicit downloads, sync and reading state. Preserve website/docs/downloads, add labeled real bundled public Markdown corpus; no private demo leakage. Keep rendering consistent and indexes derived. Pinned downloads must not be evicted as recent cache. Integrate model/runtime builds, foreground/reconnect sync and bounded background work.

RED→GREEN browser/renderer checks plus named-device native acceptance: full rendering/link corpus, airplane-mode text/assets, return reading state, quota/multi-tab/suspension/termination/low storage; no Tauri in web bundles. Expected: first-class reading on declared hosts. Commit verified slices.

### Task 6: Phase 4 — Standard cloud connectors

Port Drive/Dropbox onto Task 2 source contract. Separate content hash/provider revision/conditional-write token, standard selected-file authorization and desktop/mobile/web credential custody, stable IDs, selection, refresh, pagination/cursors, index coverage, revocation and guarded writes. Preserve uncertain-write reconciliation and restart repair. No permission expansion or private-document mutations as test shortcuts.

RED→GREEN connector/native/UI tests, then disposable-provider live acceptance where authorized/configured: read, expiry/refresh/reconnect, change/delete/revoke, stale writes/unknown outcomes. Expected: correct scopes/revisions and typed useful errors; historical acceptance flags never substitute for current release evidence. Commit verified slices.

### Task 7: Phase 5 — VS Code, migration and release

Implement local extension preview/search/backlinks/status/source jumps over shared contracts/service. Migrate useful optional document insights/review history/configuration/read state; lossless legacy export and rollback; rebuild indexes, remove generic CatDB surfaces/dependencies. Verify install/upgrades/uninstall preserve unrelated agent config. Finish signing/device distribution/download/updater provenance and accessible reader polish. Release publication/merges require their own evidence/authorization.

RED→GREEN extension integration, migration/recovery rehearsal and fresh-clone build; complete P01–P20 acceptance corpus, UI/core/provider/cloud/browser/mobile checks. Expected: no loss of source files/user records, no CatDB prerequisite, real client/source workflows accepted. One fresh whole-branch review at the end, fix material findings with regression evidence.

## Acceptance and budgets

P01 immediate read before model; P02 nested/duplicate links; P03 exact evidence deep links; P04 literal/paraphrase retrieval; P05 source change/delete/revoke; P06 slow-A/fast-B selection; P07 conflict/unknown save; P08 browser offline/quota; P09 keyboard/screenreader/narrow layout; P10 render/asset safety; P11 MCP evidence/scope; P12 local/Drive/Dropbox retrieval; P13 automatic capture; P14 cross-agent restart/handoff; P15 event replay/fork/failure; P16 exclusions/echoes; P17 desktop→mobile/web offline handoff; P18 sync conflict/deletion; P19 account/device/key recovery; P20 lifecycle/transfer failure.

Measure 1k/10k/50k libraries, large files, initial/incremental indexing, CPU/RSS, cancellation and bundle/model/index/cache bytes separately. Initial desktop p95 lexical <150 ms and warm hybrid <1 s on 10k corpus. Native mobile/browser semantics and persistence require actual host evidence, not mocked screenshots. A failed feasibility gate revises the plan explicitly before any framework/database substitution.

## Review focus

Authority revalidation at reads/writes, preservation of original Markdown/user records, durability acknowledgment and replay, stale async UI updates, provider revision/token semantics, real semantic ranking, source/project/device/account isolation, encryption tamper/recovery, blob/manifest publication failure, long-offline deletion resurrection, native/browser/mobile process lifetime and unsupported-host honesty. No “installed”, “synced”, “saved”, or “release-ready” assertion without the corresponding runtime evidence.
