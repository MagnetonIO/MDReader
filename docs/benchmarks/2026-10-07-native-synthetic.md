# Native SQLite synthetic benchmark — 2026-10-07 baseline

The measured baseline has two scale blockers: indexing 1,000 documents fails at
the normal 256-descriptor soft limit, and `linked-to` / related lookups complete
no sample within a 15-second process budget at 10,000 and 50,000 documents.
Ordinary warm lexical queries remain below the plan's 150 ms native target on
this deliberately small-document corpus.

This is scoped native-core measurement, not final branch or device acceptance.
The source snapshot is identified by compile-time BLAKE3 hashes in the
[raw results](2026-10-07-native-synthetic.json), including store hash
`fa0ce5bf1ccd741647fb2c69a09c0584aa95d0f55dbd3480f3fca8653c2e1c85`.
The working tree was under active development; later code changes do not
invalidate this baseline or establish improved results without a rerun.

## Method and environment

Apple M5 Max, arm64, 18 logical CPUs, 48 GiB RAM, macOS 26.7 (25G229), APFS.
Release Rust build; direct public `ReaderStore` APIs; SQLite/FTS5 with zero
embeddings and no model download. One source per corpus, flat files, two sections
and two chunks per document, one relative link per document in a ring, 97 tag/
topic groups, common and unique terms. Sources contain only newly generated
synthetic Markdown and are deleted after each corpus. No user library,
credentials, agent sessions, or application configuration are used.

"Cold index" means the first pass into empty SQLite databases. Generated files
are already in the OS cache; this is not a cold-disk benchmark. Each lexical
query receives five warmups and 50 sequential measured calls, limit 20. P50/P95
use nearest-rank percentiles. Exact/missing resolve have 20 calls after reopening
the core. Linked-to and related request three sequential calls in a separate
process with a 15-second total budget; incomplete work is killed, never reported
as a successful latency. Their first call follows a fresh connection reopen.
Timeout wall includes reopen and prior completed calls, rather than constituting
an exact per-call lower bound. These are observations from one run on an active
development machine, without CPU isolation or confidence intervals.

The normal shell soft FD limit is 256 (hard limit unlimited; host
`kern.maxfilesperproc=184320`). The first 1k run failed with `cannot-inspect`;
[the failure artifact](2026-10-07-native-synthetic-default-fd-failure.txt) is
preserved. Scale measurements below required **raising only the benchmark
process's FD limit to 65,536**. This workaround does not qualify normal-limit
indexing as accepted.

## Index and warm query timings

All values are milliseconds. Exact resolve selects a relative filename plus
`#details` and verifies that one reference is returned. Heading/path correctness
is tested separately; this benchmark records counts rather than full payloads.

| Documents | Empty index | Unchanged pass | One changed file pass | Broad lexical p50 / p95 | Exact resolve p50 / p95 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 56.15 | 16.38 | 16.29 | 0.390 / 0.467 | 0.264 / 0.372 |
| 10,000 | 640.40 | 253.87 | 218.38 | 4.964 / 6.724 | 2.603 / 2.922 |
| 50,000 | 4171.50 | 2008.38 | 1754.55 | 25.016 / 31.152 | 13.311 / 15.065 |

| Documents | Linked-to p50 / p95 (3 requested samples) | Related p50 / p95 (3 requested samples) | Source bytes | Closed SQLite bytes |
| ---: | ---: | ---: | ---: | ---: |
| 1,000 | 277.937 / 283.883 | 278.782 / 279.863 | 337,065 | 1,708,032 |
| 10,000 | 0 completed in 15 s | 0 completed in 15 s | 3,370,065 | 15,642,624 |
| 50,000 | 0 completed in 15 s | 0 completed in 15 s | 16,850,065 | 78,950,400 |

At 10k, query p95 values were 6.724 ms for common `reading`, 0.227 ms for
`topic003`, 0.106 ms for a unique sentinel, 4.488 ms for `reading tag:topic003`,
and 0.523 ms for a path-only filter. At 50k the highest tested lexical p95 was
32.408 ms (`reading tag:topic003`). Missing-reference p95 grows from 0.510 ms
at 1k to 2.909 ms at 10k and 17.103 ms at 50k. Raw per-sample times, returned
hit counts, live WAL/database sizes, and query plans are in the JSON artifact.

The [resource artifact](2026-10-07-native-synthetic-resource.txt) records
82.39 s wall, 59.15 s user, 20.24 s system, and 287,014,912 bytes maximum resident
set size for the benchmark invocation. These aggregate process resource figures
are not per-query memory or a mobile budget.

## Concrete bottlenecks

1. The baseline bound scanner opens each Markdown file and retains its `File`
   authority inside every `ScannedFile` before indexing begins. Descriptor use
   grows with document count. The 1k normal-limit failure, followed by a successful
   identical executable under the raised limit, confirms the operational blocker.
   Bounded streaming must preserve exact opened-file/root authority while closing
   handles after consumption; increasing global limits is not the product fix.
2. Baseline `resolve_reference` materializes every document in the source even
   for an exact filename. Its query uses the source/path index, but still returns
   the whole source. The approximately linear 1k→50k resolve timings agree with
   that implementation.
3. Baseline `query_filters` (`linked-to`) and `related` enumerate source links and
   call full-source resolution once per link. With one link per document, this
   is quadratic document materialization. Query plans confirm source-scoped
   document/link indexes exist; missing indexes alone do not explain this loop.
   The result limit does not bound traversal work. At 1k both are already about
   280 ms; at 10k/50k they do not finish a sample within the worker budget.
4. An unchanged or one-file-change index pass still scans, opens, reads, hashes,
   and touches scan markers for the entire source. The 50k observations are
   about 2.01 s and 1.75 s respectively; this is source-wide incremental work.

A single synthetic corpus of ~337-byte files does not establish large-file,
multiple-source, real-document, cold-storage, semantic, service transport, CPU
saturation, cancellation, browser, or mobile acceptance. Exact backlink-only
results are not measured here; `related` is the existing combined incoming /
outgoing set. Follow-up changes require the same harness to be rerun under the
normal FD limit, preserving this baseline and its source hashes.

## Reproduce

From the native worktree:

```sh
CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/benchmark-target \
  cargo build --release -p mdreader-core --example synthetic_perf
ulimit -n 65536
/usr/bin/time -l /Users/mlong/Library/Caches/MDReader/benchmark-target/release/examples/synthetic_perf \
  --sizes 1000,10000,50000 --samples 50 --link-budget-seconds 15 \
  --json-out docs/benchmarks/2026-10-07-native-synthetic.json
```

The raised limit reproduces the baseline scale measurements only. Omit `ulimit`
for normal-limit validation. The example's `--help` describes bounded options.
