# Actual BGE resource controls: short packaged probe

The new immutable debug service observed roughly **25% of one CPU** during this
small balanced indexing workload, stopped installing vectors while paused,
preserved pause across restart, and recorded **0.00 CPU seconds over 8.012 seconds**
with its real model loaded and no work pending. Foreground semantic-only queries
can still fail fast with an explicit Busy status during background inference;
this probe does not establish uninterrupted semantic availability.

Measured executable SHA-256:
`6fc393b90f6c86da2b08f44b4a48d8f6da9b86752ac2435b8462faceecd3d963`.
The parent supplied `/tmp/mdreader-reader-next-gui/binaries/balanced-reader-service`
as an immutable copy of the newly built debug bundle. The bytes remained unchanged.
This build includes schema-5 compact vector membership counts, the persisted
balanced/paused/full-speed policy, and `with_intra_threads(1)` for actual BGE.
One configured ONNX intra-op thread is source/build evidence, not a measurement of
all process threads or an OS CPU quota.

The fixture used 120 disposable Markdown documents (114,748 B), 20 synthetic
topics, exactly 120 actual BGE passage vectors, and the explicitly public existing
cache `/Users/mlong/Library/Caches/MDReader/reader-next-acceptance-model`. It used no
user sources, histories, installed profile, or client configuration. Both temporary
service owners exited and their corpus/profile were removed after the probe.
No full-speed comparison or new 10k model run was needed.

| Phase | Wall time | CPU-time delta | Observed CPU | Vector count |
|---|---:|---:|---:|---:|
| Fresh default balanced, background only | 0.760 s | 0.19 s | 24.99% | 0 → 9 |
| Foreground queries during background | 0.277 s | 0.08 s | 28.88% | job still embedding |
| Settled pause | 5.012 s | 0.07 s | 1.40% | 12 → 12 |
| Restart with durable paused policy | 3.009 s | 0.04 s | 1.33% | 12 → 12 |
| Resume balanced, finish remaining vectors | 10.241 s | 2.53 s | 24.71% | 12 → 120 |
| Model loaded, fully indexed, idle | 8.012 s | 0.00 s | 0.00% recorded | 120 |

CPU percentages are **process CPU-time deltas divided by each phase's elapsed
wall time**, using Darwin `ps` cumulative time at the endpoints. They are not the
lifetime `%CPU` displayed in a process list. CPU time has 0.01-second resolution;
the idle result means below roughly 0.13% at that resolution, not literally zero
instructions. The settled-pause measurement still has an unfinished job and
scheduler polling, unlike the completed idle phase. The configured policy targets
25% inference wall duty through cooldown; the protocol does not expose actor
admission-duration counters, so this probe does not measure that exact fraction.
Short instantaneous spikes remain possible.

Three foreground exact samples took 1.15–1.74 ms. Three hybrid samples took
4.79–5.50 ms, each with actual semantic-rank evidence. All three semantic-only
samples encountered a busy model and returned in 1.18–3.01 ms with zero hits and
an explicit unavailable diagnostic; none is counted as semantic success. After
pause settled, a foreground actual semantic query succeeded in 4.05 ms, proving
that pause blocks background admission while allowing foreground model use.
Every returned hit retained checked source/path/revision/line/text evidence.
These nine mixed samples describe availability and bounded responsiveness, not
p95 query acceptance; the separate completed 10k receipt provides warm p95 evidence.

Fresh startup was 46.393 ms and real model readiness/first query with the existing
cache was 216.418 ms. Darwin `/usr/bin/time -l` recorded first-owner maximum RSS
292,749,312 B (6.73 s wall, 0.45 s user + 0.11 s system CPU), and restart-owner
maximum RSS 278,822,912 B (21.98 s wall, 2.53 s user + 0.19 s system CPU). Whole
harness elapsed time was 28.822 s. The host was shared; unrelated AgentHero builds
were reported, a separate no-model GUI probe existed, and the parent kept Cargo
idle during this run. This is observed shared-host evidence, not isolated-host
or release-build acceptance.

The first thread-count sampler misunderstood Darwin `ps -M`'s fixed output
columns. Its zero counts were invalid; the receipt explicitly marks thread
measurements unavailable and records that correction. CPU/RSS fields use the
separate valid sampler. The parser was corrected and has a focused self-test;
there was no extra model run to manufacture replacement thread counts.

Reproduce from the native worktree with a parent-supplied immutable current build:

```sh
python3 scripts/probe-reader-resources.py --self-test
python3 scripts/probe-reader-resources.py \
  --binary /tmp/mdreader-reader-next-gui/binaries/balanced-reader-service \
  --model-cache /Users/mlong/Library/Caches/MDReader/reader-next-acceptance-model \
  --documents 120 \
  --output docs/benchmarks/2026-10-07-packaged-bge-resources.json
```

Three small measurement-helper tests pass: CPU-time units, phase deltas, and Darwin
thread-column parsing. The probe reuses the existing bounded loopback and source
evidence helpers rather than duplicating transport tests. The raw receipt retains
binary/harness/helper hashes, every phase sample, all nine foreground outcomes,
policy acknowledgement, restart evidence, actual vector counts, and failures.
