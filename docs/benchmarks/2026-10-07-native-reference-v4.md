# Native reference index v4 — 2026-10-07 follow-up

All 1k, 10k, and 50k synthetic corpora now index and complete the requested link
queries with the normal **256-descriptor** soft limit. At 50k, exact resolve p95
is 0.077 ms, related p95 is 0.581 ms, and linked-to p95 is 4.220 ms. The
[baseline](2026-10-07-native-synthetic.md) needed a process-local FD increase and
completed no linked-to/related sample within its 15-second worker budgets at
10k or 50k. This is scoped core evidence, not whole-branch acceptance.

## What changed

The parent implementation changed the bound index scanner to consume one
retained file at a time. The reference fix adds derived index schema v4:
source-scoped stem/title aliases and resolved directed edges with origin/target
line evidence. Exact path lookup uses the existing source/path unique index;
fallback aliases remain ambiguous. A changed document, added target, deletion,
or heading change rebuilds that source's derived edges in the same transaction.
An unchanged content pass refreshes filesystem modification times and scan
markers without rebuilding its graph or changing content revisions.

Read-only service handles now open existing library v4/index v4 databases with
SQLite read-only flags. They cannot create, migrate, admit, or write. The owner
can lend its existing durable connection to sync through
`with_library_connection`; neither API creates a second writer.

## Measurements

Same generated ~337-byte Markdown files, ring links, corpus sizes, query strings,
limits, five lexical warmups, 50 lexical samples, 20 resolve samples, and three
linked-to/related samples as the baseline. Percentiles use nearest rank; for
three samples p95 is the observed maximum. Link workers reopen connections and
have the same 15-second total budget. All completed; hit-count assertions passed.
Heading/path correctness is covered separately by the reference tests; the
benchmark records returned counts, not full reference payloads.

Host remains Apple M5 Max, macOS 26.7, 48 GiB RAM, APFS. Files are newly generated
and already cached. The development machine and branch were active; differences
are observations from two snapshots, not a controlled attribution of every
latency change to one patch. The measured store BLAKE3 is
`5af556792797bf0a2d29a8ec48002ba8f460bcfab138af3985db58f9d346176d`;
the measured query hash is in the
[raw follow-up](2026-10-07-native-synthetic-reference-v4.json).

All timings below are milliseconds.

| Documents | Empty index | Unchanged pass | One changed file | Broad lexical p50 / p95 | Exact resolve p50 / p95 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 81.17 | 18.69 | 26.20 | 0.437 / 0.572 | 0.046 / 0.134 |
| 10,000 | 954.22 | 266.82 | 343.85 | 5.418 / 7.424 | 0.044 / 0.059 |
| 50,000 | 6646.59 | 1826.82 | 3392.42 | 37.722 / 101.445 | 0.062 / 0.077 |

| Documents | Linked-to p50 / p95 | Related p50 / p95 | Missing resolve p95 | Closed SQLite bytes |
| ---: | ---: | ---: | ---: | ---: |
| 1,000 | 0.191 / 0.786 | 0.147 / 0.277 | 0.100 | 2,506,752 |
| 10,000 | 0.437 / 0.965 | 0.221 / 0.260 | 0.075 | 23,523,328 |
| 50,000 | 3.797 / 4.220 | 0.379 / 0.581 | 0.166 | 117,936,128 |

At 50k, selective-topic p95 is 2.051 ms, unique-sentinel p95 0.199 ms,
`reading tag:topic003` p95 77.099 ms, and path-filter p95 8.149 ms. Broad lexical
p95 remains below 150 ms in this run, but its **maximum was 262.177 ms**. The
baseline broad p95 was 31.152 ms; this follow-up does not establish a lexical
speedup or universal latency bound.

The [resource artifact](2026-10-07-native-synthetic-reference-v4-resource.txt)
records 27.47 s wall, 8.93 s user, 14.10 s system, and 18,628,608 bytes maximum
resident set size. Baseline aggregate values were 82.39 s wall and 287,014,912
bytes RSS. These include the harness and worker workload, not isolated-query or
mobile memory measurements. Source sizes are unchanged from the baseline.

## Costs and remaining bottlenecks

- Derived aliases/edges and their indexes add storage: 50k closed SQLite size is
  117.94 MB versus 78.95 MB baseline. Empty indexing takes 6.65 s versus 4.17 s.
  A one-file change takes 3.39 s versus 1.75 s because an alias/heading/target
  change can affect previously unchanged links, requiring source-wide graph
  refresh. Unchanged passes still scan, read, hash, and update every document.
- Exact resolve and alias fallback use indexed lookups; incoming/outgoing edges
  use covering indexes or their primary key. Full-source resolution per raw
  link has been removed. The raw follow-up includes `v4_*` access-path plans.
- A [supplemental 20-document query-plan diagnosis](2026-10-07-native-reference-v4-query-plans.json)
  includes the complete linked-to search shape. SQLite still visits selected
  source documents once, checks the indexed incoming ID set, and sorts matching
  chunks. This explains remaining source-size dependence without recreating the
  former per-link corpus scan. Path substring GLOB also visits selected-source
  documents and uses a temporary sort. Those planner/query-shape improvements
  are separate from this scoped reference fix.
- Bounds are explicit failures, not silently truncated graphs: at most 100,000
  non-image raw links and 1,000,000 resolved edges per source rebuild, 10,000
  ambiguous matches per reference, and 100,000 target IDs per linked-to filter
  across selected sources. Exceeding a rebuild budget rolls back the transaction
  and retains prior indexed documents/edges. A v3 migration encountering such
  a budget fails without publishing partial v4 state. High-degree graphs and
  extreme ambiguity are bounded but are not latency-qualified by the ring test.

`related` here is the existing combined incoming/outgoing link set. Exact
backlink-only navigation, conceptual/vector graph behavior, semantic models,
service transport responsiveness, real/private corpora, large documents, mobile,
and browser performance are not accepted by this measurement.

## Regression evidence and reproduction

The new 3,000-document test first failed on the previous implementation:
related plus linked-to took **16.596 seconds**, exceeding its generous 2-second
bound. It passes after the fix. New tests also cover relative exact precedence,
basename/title ambiguity, heading and revision refresh, missing-target creation,
delete fallback, image exclusion, source isolation/revocation, v3 migration,
transaction rollback on a link budget, timestamp-only changes, and read-only
schema/write/concurrent-reader behavior. The full core suite passes; scoped
Clippy for the library, new tests, and benchmark passes with warnings denied.

```sh
CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/benchmark-target \
  cargo test -p mdreader-core
CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/benchmark-target \
  cargo build --release -p mdreader-core --example synthetic_perf
ulimit -n 256
/usr/bin/time -l /Users/mlong/Library/Caches/MDReader/benchmark-target/release/examples/synthetic_perf \
  --sizes 1000,10000,50000 --samples 50 --link-budget-seconds 15 \
  --json-out docs/benchmarks/2026-10-07-native-synthetic-reference-v4.json
```

The baseline raw/resource/failure artifacts remain intact. Its compiled
executable is retained outside the repository as
`benchmark-target/release/examples/synthetic_perf-before-reference-v4` in the
MDReader Cargo cache. The harness now also captures supplemental complete-query
plans; corpus generation and measured operations are unchanged.
