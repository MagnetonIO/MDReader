# Packaged service with actual BGE: scale acceptance

The completed actual-BGE 10,000-document run meets the requested warm query
budgets: exact p95 **5.308 ms** (<150 ms), hybrid p95 **182.214 ms** (<1 s).
Semantic p95 is **180.365 ms**. All 90 measured queries passed source, revision,
line evidence, and expected-topic checks without degradation. Initial passage
indexing still takes **992.128 s** in this immutable debug binary; profiling found
repeated vector-payload status counting, subsequently fixed in core but not included
in this measured executable. These are shared-host synthetic-fixture measurements,
not isolated-host or broad relevance acceptance.

## Measured baseline

- Executable: the ad-hoc signed debug `MDReader Reader Next Probe.app` bundle,
  supplied by the parent after its native build. SHA-256:
  `1d075541ed836b0b3f9e23cabadc686a25907148aee7b290ee3006a468da6b5c`.
  The executable hash was unchanged after the run.
- Host: macOS arm64; the raw receipt records the platform version. A separate
  synthetic GUI probe could run on the same host, with its model disabled.
- Public BGE cache explicitly selected:
  `/Users/mlong/Library/Caches/MDReader/reader-next-acceptance-model`.
  No user sources, histories, installed profile, or live client settings were used.
- One disposable admitted local source, 1,000 Markdown files, one actual chunk per
  file, 20 meaningful topics, deterministic differing observation text. The raw
  receipt records the corpus digest. All 1,000 chunks received actual BGE vectors
  through the packaged service; the script does not open SQLite or install vectors.
- One cold lexical index, followed by model initialization/first actual query and
  a durable `start_index` job. That queued generation does an additional unchanged
  lexical pass before embedding; its time is included in the embedding/job total.
- Each mode: five discarded warmups followed by 30 serial timed loopback searches,
  alternating topics. Exact uses broad topic keywords; semantic and hybrid use
  paraphrases. Latency includes framing, service dispatch, actual query embedding
  when applicable, and retrieval. Revision-checked `read` validation follows each
  timed sample and is excluded from latency.
- Every sampled hit was checked for source identity, corpus membership, revision,
  positive valid line ranges, and text belonging to the original Markdown. Every
  query found its expected topic within the first five results; semantic/hybrid
  required actual semantic-rank evidence and no degraded status.

| Measure | 1k baseline |
|---|---:|
| Packaged service startup | 51.003 ms |
| Synthetic Markdown bytes | 330,193 B |
| Cold lexical index | 160.813 ms |
| Model initialization + first actual query, existing public cache | 212.974 ms |
| Queued unchanged index + all passage embeddings | 74,517.548 ms |
| Warm exact p95 | 1.607 ms |
| Warm semantic p95 | 22.079 ms |
| Warm hybrid p95 | 26.400 ms |
| Failed/degraded samples | 0 / 90 |
| Service wall / user CPU / system CPU | 76.42 / 25.24 / 17.65 s |
| Service maximum RSS | 287,113,216 B |
| Executable logical size | 149,009,632 B |
| Debug bundle logical / allocated size | 149,112,101 / 149,123,072 B |
| Live index.sqlite + WAL + SHM | 7,835,224 B |
| Public cache logical / unique allocated size | 267,612,160 / 133,824,512 B |

All 90 measured queries also placed their expected topic first. Warm p50/max were
0.889/1.714 ms exact, 18.090/22.809 ms semantic, and 20.000/27.391 ms hybrid.
These are coarse topic expectations, not a broad relevance-quality benchmark.

The cache logical size counts linked file names; unique allocated bytes deduplicate
inode identities. Bundle traversal does not follow directory symlinks. The receipt
records individual live SQLite/WAL/SHM sizes and all per-query samples. CPU/RSS are
from Darwin `/usr/bin/time -l` around the packaged service, excluding the Python
corpus generator and query client. Index-size values are measured with the owner
running and include WAL state, not a closed/checkpointed portable backup size.

## Concrete scheduler finding

Observed progress was 144 vectors at 10 s, 287 at 20 s, 426 at 30 s, 561 at 40 s,
696 at 50 s, 827 at 60 s, and 955 at 70 s. The scheduler slept 20 ms after every
active turn, including successful completion and document installation, rather
than only when all sources awaited model work. A one-chunk document needs separate
admission/completion/install turns, so this overhead recurs per file. The 1k run
confirms the cost; attribution to inference versus SQLite requires further profiling.

The interrupted 10k launch observed a 1,705.050 ms cold lexical index and 116 actual
vectors after 10.1 s. It was stopped deliberately and its temporary owner/corpus
removed. These values are diagnostic only; the later completed optimized 10k run below
provides query acceptance evidence.

## Reproduction

Run from the native worktree after the parent supplies a freshly built bundle:

```sh
python3 scripts/benchmark-reader-service.py --self-test
python3 scripts/benchmark-reader-service.py \
  --binary 'target/debug/bundle/macos/MDReader Reader Next Probe.app/Contents/MacOS/mdreader-app' \
  --model-cache /Users/mlong/Library/Caches/MDReader/reader-next-acceptance-model \
  --documents 1000 \
  --output docs/benchmarks/2026-10-07-packaged-bge-1k.json
```

Use `--documents 10000` and a distinct output name for 10k. The script pins and
rechecks executable bytes, keeps source-authority checks enabled, uses only the
explicit public cache, bounds response frames and wall deadlines, and removes its
synthetic profile/corpus on normal completion. Four self-tests cover deterministic
corpora, source/path/revision/line evidence, actual loopback envelopes/frame bounds,
and percentile/resource interpretation. Query-quality and budget failures remain
visible in the receipt; they are not relabeled as semantic success.

Raw baseline: `2026-10-07-packaged-bge-1k.json`. No GUI, mobile, cloud, release-size,
cold-model-download, or broad semantic-quality claim follows from this fixture.

## Scheduler change and optimized 1k measurement

The runtime now distinguishes `Progress`, `Waiting`, and `Idle`. After progress,
its zero-time wake check immediately starts another bounded, fair turn and drains
captures again. It retains a 20 ms backoff when all active jobs await the model and
a one-second idle poll. The model actor, deadlines, batch bounds, source authority,
generation checks, and live-revision install checks are unchanged.

A focused real worker-loop regression with 20 immediately available fake operations
failed before the fix at 1.4326 s against a 900 ms bound, then passed in a test run
of roughly 60 ms. The blocked-source fixture explicitly verifies the waiting
backoff. Full service tests (27) and all-target Clippy pass. These synthetic worker
results justify the scheduler change. The subsequent actual-BGE run below measures
its effect; the subsequent 10k run measures the larger fixture.


The optimized native build is measured through an immutable copy supplied by the
parent: `/tmp/mdreader-reader-next-gui/binaries/optimized-reader-service`, SHA-256
`f64a8eb00ec82a6799fe84e5d5495f52d6080d348576b6f50384a71401d59314`. The script verified
that the staged signed app bundle contained those exact executable bytes before
recording bundle size. The executable remained unchanged throughout the run.
The deterministic 1k corpus hash matches the baseline exactly.

| Measure | Optimized 1k |
|---|---:|
| Packaged service startup | 51.792 ms |
| Cold lexical index | 149.592 ms |
| Model readiness + first actual query | 213.724 ms |
| Queued unchanged index + all real passage vectors | 30,805.031 ms |
| Warm exact / semantic / hybrid p95 | 1.875 / 23.637 / 28.469 ms |
| Failed/degraded queries | 0 / 90 |
| Service wall / user CPU / system CPU | 32.89 / 26.85 / 16.11 s |
| Service maximum RSS | 287,227,904 B |
| Executable / matching debug bundle logical bytes | 149,338,560 / 149,441,029 B |

Passage-indexing wall time improved about 2.42x. Total service CPU remained nearly
the same (42.89 s baseline versus 42.96 s optimized), consistent with removing
scheduler waits rather than substituting synthetic vectors or less inference.
The measured warm query budgets also pass at 1k. A separate no-model GUI probe,
core/service checks, and an iOS simulator compilation ran during early 10k indexing.
The parent stopped its Cargo builds before query measurement. Other unrelated
AgentHero builds were reported system-wide and were not controlled by this fixture;
shared-host contention may affect both indexing and query wall time. Per-service
CPU/RSS excludes those separate processes.

Raw optimized receipt: `2026-10-07-packaged-bge-1k-optimized.json`.


## Completed optimized 10k actual-BGE run

The same immutable executable and public model cache served a fresh disposable
10,000-document corpus (3,301,968 B; corpus SHA-256
`8c24f72a975beaa2eb6dad893e16fddeaaeb673b5210ddfaee4d2467a5623b93`).
Every chunk received an actual BGE passage vector through the service. All 90
measured queries placed the expected topic first; no query failed or reported
semantic degradation. The benchmark owner/profile/corpus were removed afterward.

| Measure | Optimized 10k |
|---|---:|
| Startup | 49.540 ms |
| Cold lexical index | 1,919.924 ms |
| Model readiness + first actual query, existing cache | 218.048 ms |
| Queued unchanged lexical pass + 10,000 actual passage vectors | 992,128.316 ms |
| Warm exact p50 / p95 / max | 1.815 / 5.308 / 5.410 ms |
| Warm semantic p50 / p95 / max | 136.741 / 180.365 / 213.223 ms |
| Warm hybrid p50 / p95 / max | 135.882 / 182.214 / 196.455 ms |
| Failed/degraded queries | 0 / 90 |
| Service wall / user CPU / system CPU | 1,005.70 / 863.51 / 226.47 s |
| Service maximum RSS | 289,013,760 B |
| Live index.sqlite + WAL + SHM | 54,063,784 B |
| Total temporary data logical / allocated bytes | 58,335,482 / 59,445,248 B |
| Public model cache logical / unique allocated bytes | 267,612,160 / 133,824,512 B |
| Immutable executable logical bytes | 149,338,560 B |

The optimized 1k receipt recorded the matching signed debug bundle at 149,441,029 B
logical (149,450,752 B allocated). The 10k run used its immutable executable copy,
so its receipt intentionally leaves bundle size unset rather than inspecting a
bundle the parent could rebuild. Its executable SHA-256 remained
`f64a8eb00ec82a6799fe84e5d5495f52d6080d348576b6f50384a71401d59314`.
The cache was unchanged. Model initialization here uses an existing public cache;
cold download time and release bundle size are not measured.

The roughly 32x increase in passage-indexing time for 10x the documents is a
concrete limitation, despite passing warm search budgets. A bounded native sample
of this exact process showed the scheduler reading vector payloads during each
status update while the model owner mostly waited. The separate
[`2026-10-07-native-status-counts.md`](2026-10-07-native-status-counts.md) report records
that finding and a focused schema-5 compact-membership fix: 10k status samples fell
from roughly 232 ms median to roughly 1 ms. **This immutable packaged run predates
that fix.** It proves the observed warm budgets and actual BGE evidence, not the
new fix's end-to-end indexing speed. A new packaged run is needed to measure that.

Raw completed receipt: [`2026-10-07-packaged-bge-10k-optimized.json`](2026-10-07-packaged-bge-10k-optimized.json).
Reproduce it with the same command above, substituting the immutable binary path,
`--documents 10000`, and a distinct output filename. The fixture exercises 20
coarse topics, single-section short documents, one local source, and serial warm
queries. It does not establish concurrent-source throughput, large-document
chunking, provider performance, or general semantic relevance quality.
