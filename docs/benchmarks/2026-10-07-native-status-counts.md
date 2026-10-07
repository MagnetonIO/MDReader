# Compact progress counts for the native vector index

Actual packaged 10k BGE indexing exposed repeated source-wide vector-payload reads
inside progress reporting. A one-second sample of only the synthetic service owner
retained 37 samples per thread: the scheduler spent 22 of its samples in
`phase -> record_index_job_if_current -> index_status`, including 17 in vec0 point
lookups. The model-owner thread was waiting for work in 30 samples and inferring
in seven; other ONNX worker threads are omitted, so these are **not** whole-process
CPU percentages. The excerpt is saved in
`2026-10-07-packaged-bge-10k-index-profile.txt`.

`index_status` ran before every document/batch admission. Its count query joined
chunks to the vector virtual table, which fetched vector payloads for point
lookups merely to count membership. As installed vectors accumulated, this work
recurred over the growing corpus. It adds quadratic total work across a complete
initial embedding run. The same payload lookups affected discovery of missing
passage vectors during recovery.

Index schema **5** adds `embedding_memberships(chunk_id, source_id)` with a chunk
foreign key and a covering source index. `install_embeddings` upserts membership
in the same transaction as vector replacement. Chunk/document deletion cascades
membership and uses the existing vector cleanup trigger. Failed inference
installation or source indexing rolls back both representations. Counts and
pending-document lookup use this compact metadata, leaving vector payloads for
actual retrieval.

Owner migration backfills existing v4 rows once using public `vectors.rowid`
iteration with vectors outermost, followed by chunk/document identity joins.
It uses no sqlite-vec private shadow-table ABI. Readonly handles require the owner
to migrate first. The durable library remains schema 4; bookmarks, source grants,
capture records and jobs are not migrated by this change. Recreating the derived
index recreates empty membership and requires ordinary source reindex/embedding.

## Regression measurement

This separate core regression uses one synthetic Markdown document with 10,000
sections/chunks and deterministic unit vectors installed through the public core
API in one transaction. It is **not** the packaged actual-BGE benchmark, and none
of these vectors are substituted into that benchmark. After one warmup, ten
`index_status` calls preserve exact document/chunk/vector totals and must each
complete under 20 ms.

| Observed debug test run | p50 | p95 / maximum |
|---|---:|---:|
| Before fix, RED | 231.904 ms | 388.012 ms |
| After fix, GREEN | 1.080 ms | 1.137 ms |
| Subsequent GREEN | 0.774 ms | 0.803 ms |

Both the actual BGE indexing process and other early-stage compilation could run
on the same host during these tests. These are observed bounded-regression
measurements, not calibrated isolated microbenchmarks. Compact indexed counts
still traverse matching metadata entries; they are not claimed to be constant
work at arbitrary corpus size.

New `tests/status_counts.rs` covers source isolation, reinstall without double
counting, rollback after the second vector fails validation, rollback of chunk
cascades after a later invalid UTF-8 Markdown file, stale-revision rejection,
revision changes, document deletion, revoke/regrant, Forget cleanup, v4 migration,
readonly reopening, and derived-index recreation while preserving a bookmark.
The existing readonly-version and v3 fixtures now correctly represent schema 5.
Full core tests (50) and service tests (27) pass, as does all-target Clippy with
warnings denied. No runtime progress throttle was added: compact counts resolve
the demonstrated payload-read cost while preserving current progress reporting.

Reproduce:

```sh
CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/benchmark-target \
  cargo test -p mdreader-core --test status_counts -- --nocapture
```

The concurrent packaged BGE 10k run is pinned to the earlier immutable executable
and does not include this count fix. Its latency/resource acceptance is reported
separately in `2026-10-07-packaged-bge-scale.md`.
