# Native ranked Related — 2026-10-07

The new core API combines actual two-hop Markdown graph, explicit shared tags,
and stored-vector evidence. Warm p95 with 100,000 synthetic chunk vectors across
50,000 documents is **730.118 ms**, after fixing a measured 8,327.440 ms vector
access-path regression. The existing one-hop incoming/outgoing `related` API
remains compatible with MCP and the extension. This is core evidence, not
desktop/service/mobile/browser or semantic-quality acceptance.

## Contract and preservation boundaries

`ReaderStore` adds `related_ranked(source, path, limit)`,
`related_ranked_query(source, path, query, limit)`, and
`related_ranked_cancelled(source, path, Option<&str>, limit, cancellation)`.
They return `RelatedRanked { anchor, hits, availability }` from one consistent
index read transaction, with root/source authority checks before and after.
Read-only handles support these methods. Unknown indexed anchors return an empty
outcome with `anchor_missing`; unavailable/revoked sources fail closed.

Every hit carries source ID, relative path, revision, title, one-based line,
component contributions, graph depth, tag intersection/union counts, and observed
cosine when measured. The score is the sum of contributions, not a probability:

- Graph: best outgoing resolved Markdown path, depth at most two, contribution
  0.5 for one hop and 0.25 for two. Self/cycle results are excluded. Incoming
  backlinks remain available through the separate existing link API.
- Shared tags: **0.5 × Jaccard over indexed frontmatter tags**. The component is
  named `shared_tags`, never `concept`. It measures actual intersection/union.
- Vector: **0.4 × cosine clamped to [0, 1]**. The query direction is the mean of
  the anchor's installed chunk vectors; each target uses its best chunk cosine.
  F64 accumulation and normalization preserve that mean direction. Targets are
  grouped before the final result limit, without a chunk top-k candidate window.
  Missing target vectors are `None`; a measured zero or negative cosine remains
  distinguishable from missing evidence. Zero-only unrelated candidates are
  omitted, rather than filling the list with unrelated documents.

This follows the actual pinned legacy SDK v1.0.0 implementation at
`2c7b8bc45b09abf87dc52ef4f7db2a3cf286f3ae`, inspected in CatDB's engine search and
SQLite relation/vector code. Its vector behavior was source mean against best
destination chunk despite a misleading destination-centroid description. Its
graph followed outgoing edges with 0.5 decay, including knowledge entities/spans
lowered to documents; its concept signal traversed derived concept joins.

**Legacy knowledge entity/span edges and concept joins are not migrated here.**
Availability explicitly returns `not_migrated` for `legacy_graph_entities` and
`legacy_concepts`. Frontmatter tags do not substitute for that entity model.
Optional document insights, reviews and their migration remain outside this
slice and are not claimed preserved by ranked retrieval.

Vector availability is `available`, `no_anchor_vectors`, `zero_centroid`, or
`anchor_missing`, with anchor chunk/vector coverage and actual evaluated target
chunk/document counts. Existing stored vectors can be used without loading a
model. Partial anchor coverage is disclosed and uses only installed vectors.
The API performs no model inference or lexical fallback disguised as semantics.

The query variant reuses search's tag/path/after/linked-to and literal-term
filters. Filters apply before final limit and vector distance evaluation;
intermediate graph documents can be outside the final filter. Result ordering is
score descending, then path and line. Limits are 1–100 hits. Explicit work bounds
fail rather than return a partial ranking: 100,000 graph neighbor edges visited,
100,000 candidate documents, and 200,000 anchor or eligible target vectors.
Those count bounds do not establish a wall-clock bound on every possible source.

## Synthetic measurement

Same Apple M5 Max / 48 GiB / macOS 26.7 development host as the
[link benchmark](2026-10-07-native-reference-v4.md), normal FD limit 256, release
build. Each temporary corpus has ring links, two chunks/document and frontmatter
tags `[benchmark, topicNNN]` with 97 topic groups. The common tag intentionally
makes every other document a tag candidate. Vectors are deterministic 384-float
synthetic values, not model outputs. Each target's first coordinate is positive;
the anchor vector is a unit direction. There is no semantic-quality claim.

The ignored manual fixture uses five warmups and twenty samples per workload;
nearest-rank p50/p95. Graph+tags runs before vector installation. Full ranking
runs with real vec0 rows, excluding both anchor chunks and evaluating all other
chunks. The filtered workload selects `note000002.md`, still observes graph
depth two and evaluates its two vector distances. Assertions verify availability,
scope/counts and that returned vector contributions were actually computed.

Bulk vector setup uses current synthetic chunk IDs in one SQLite transaction to
avoid per-document fsync dominating setup. Correctness tests use the public
revision-guarded install API. Bulk setup is not production inference/ingestion
performance. At 50k the nominal vector payload alone is 153,600,000 bytes; actual
database/cache/model sizes were not separately measured in this fixture.

All timings below are milliseconds.

| Documents / vectors | Graph+tags p50 / p95 | Full ranking p50 / p95 | Full max | Filtered p95 |
| ---: | ---: | ---: | ---: | ---: |
| 1,000 / 2,000 | 1.213 / 1.358 | 12.411 / 13.715 | 15.371 | 0.846 |
| 10,000 / 20,000 | 16.236 / 20.501 | 143.129 / 169.026 | 183.782 | 5.608 |
| 50,000 / 100,000 | 121.982 / 149.717 | 669.899 / 730.118 | 835.684 | 28.499 |

[Raw samples and compile-time store/related hashes](2026-10-07-native-related-ranked.json)
identify the measured snapshot. The measured related hash is
`a355d12799d4ab2fc917b9d1a13f43fc709067cfc060c87c2e49b6ecaca3612a`;
subsequent formatting does not change those frozen measurements. The
[resource artifact](2026-10-07-native-related-ranked-resource.txt) records
46.45 s wall, 26.22 s user, 13.80 s system and 227,311,616 bytes maximum RSS for
the complete direct test-binary invocation. Corpus generation, indexing and
vector setup are included in aggregate resources, excluded from query timings.
The machine/branch were active; this is one run per snapshot, without confidence
intervals or CPU/storage isolation.

## Reproduced access-path bottleneck

The first scalar-distance SQL joined documents/chunks to vec0. SQLite reordered
it into vec0 point lookups following source/path document order. Those visits
jumped between large vector blobs and repeatedly traversed overflow pages.
An actual macOS process sample shows the hot stack
`vectors → sqlite3_step → vec0Filter_point → vec0_get_vector_data → blobReadWrite
→ getOverflowPage → pread`; the
[active-thread profile](2026-10-07-native-related-ranked-point-profile.txt) is
preserved. Inspection of sqlite-vec 0.1.9 confirmed point/fullscan vector reads
fetch blobs, while its fullscan row IDs are ordered by storage chunk/offset.

Forcing vec0-first `CROSS JOIN` iteration keeps those accesses in storage order.
The same candidate/filter logic and correctness regressions pass. No top-k
truncation, centroid proxy, embedding cache or fabricated score hides work.

| Documents | Initial point-query full p95 | Storage-order full p95 |
| ---: | ---: | ---: |
| 1,000 | 79.326 | 13.715 |
| 10,000 | 1446.756 | 169.026 |
| 50,000 | 8327.440 | 730.118 |

The [point baseline raw samples](2026-10-07-native-related-ranked-point-baseline.json)
and [resource artifact](2026-10-07-native-related-ranked-point-baseline-resource.txt)
remain intact (235.96 s wall; 230,948,864 bytes maximum RSS). The improvement has
a tradeoff: filtered p95 at 50k rises from 15.856 to 28.499 ms because the forced
scan still visits vector row IDs even when only two distances qualify. Evaluated
distance counters do not pretend to count all physical row-ID/metadata visits.

Remaining work is linear exact vector scanning plus grouping, and wide-tag
candidate enumeration/metadata sorting. The 50k common-tag workload collects
49,999 candidate documents before limit. Large/high-degree/multi-source/partial
libraries, real embeddings, cancellation under SQL work, service transport and
named device hosts require their own measurements. This report does not accept
the service model scheduler or whole-branch migration.

## Regression and reproduction

The first new behavior test ran against a minimal wrapper over legacy link-only
results and failed: only `direct.md` was returned where actual vector-only,
tag-only and two-hop candidates were required. The implemented API passes four
scoped tests covering those candidates, weighted components, exact evidence,
mean-versus-target-best-chunk semantics, cycles, filters, source revocation,
missing/zero/negative/partial vectors, revisions/deletes/reopen, bounds and
cancellation. A test with both 4,200-chunk anchor and 4,200-chunk target preserves
small neighbors beyond sqlite-vec's KNN window ceiling. The default core suite
and scoped Clippy pass; this manual performance fixture was also run in release.

```sh
CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/benchmark-target \
  cargo test -p mdreader-core --test related_ranked
CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/benchmark-target \
  cargo test --release -p mdreader-core --test related_ranked \
  ranked_related_synthetic_performance -- --ignored --exact --nocapture
```

The fixture prints `RELATED_PERF_JSON:` with the raw data and removes all
temporary corpora/databases. The initial point-query executable is retained in
the external Cargo cache as `release/deps/related_ranked-before-sequential-scan`.
