# Packaged BGE pause, restart, and idle follow-up — 2026-10-07

The new packaged service passed a controls-only synthetic probe: pause persisted through restart, no passage vectors were installed, and foreground query embeddings initialized actual BGE while background work remained paused. This receipt does not rerun the 120-document throughput measurement or establish retrieval relevance.

Binary: `/tmp/mdreader-reader-next-gui/binaries/paused-idle-reader-service`, SHA256 `825f2bbbfb952ff932e8cb84545a7031d794c81dee789eeb6fd5b311b740c5e8`. The [machine-readable receipt](2026-10-07-packaged-bge-paused-idle.json) includes the parent-captured selected-input fingerprint, source HEAD `2568abc9b01bfc3c879589dd97198c3cdce29ef9`, and per-file hashes. The fingerprint was captured during the native build and checked unchanged before copying the binary; it is a partial selected-input attestation, not a complete reproducible-build proof. Binary bytes were verified unchanged after this probe.

The temporary private profile contained 20 generated topic documents and used only the explicitly public model cache `/Users/mlong/Library/Caches/MDReader/reader-next-acceptance-model`. Fresh policy was Balanced; the probe selected Paused before admitting/indexing the corpus. Actual foreground semantic queries initialized BGE on both owners (176.94 ms and 172.86 ms), with zero hits because no passage vectors existed. These are query-embedding availability observations, not successful semantic retrieval.

| Measured phase | Wall time | Process CPU-time delta | Observed CPU |
| --- | ---: | ---: | ---: |
| Paused, unfinished 20-document job | 5.019 s | 0.01 s | 0.199% |
| Restart with durable Paused policy | 3.015 s | 0.00 s | Recorded 0% |
| Model loaded, source revoked, idle | 8.016 s | 0.00 s | Recorded 0% |

CPU percentages use process CPU-time deltas divided by elapsed wall time, not `ps` lifetime CPU percentages. Sampling resolves 0.01 CPU seconds; recorded zero is bounded by approximately 0.34% over the restart window and 0.13% over the idle window, rather than proof of zero instructions. Passage-vector counts remained 0 before and after both paused windows. Revoking the synthetic source removed pending authority before the final idle window; no passage-inference rerun occurred. Both owners exited and all temporary source/profile data was removed.

Peak RSS was 282,296,320 bytes for the first owner and 280,281,088 bytes after restart. Root Cargo remained idle during measured windows; the host was shared and unrelated builds may have existed. The configured ONNX intra-op setting of one is source/build evidence, not a measured total thread count. Darwin thread-row parsing was found incomplete after this run; thread counts are explicitly unavailable in the receipt. A separate four-thread synthetic Python process reproduced Darwin's blank-USER continuation rows, and the parser plus focused test were corrected without another model run. CPU/RSS measurements do not depend on that parser.

This build uses the scheduler's interruptible one-second Paused wait instead of polling every 20 ms. The earlier short probe recorded approximately 1.4% during pause; this follow-up used a different fixture and shorter workload, so the comparison is directional rather than a controlled performance ratio. Balanced duty, warm query p95, and large-corpus throughput are covered by separate receipts and are not re-established here.

Reproduction:

```sh
python3 scripts/probe-reader-resources.py --pause-only --documents 20 \
  --binary /tmp/mdreader-reader-next-gui/binaries/paused-idle-reader-service \
  --build-inputs /tmp/mdreader-reader-next-gui/pause-idle-build-inputs.json \
  --model-cache /Users/mlong/Library/Caches/MDReader/reader-next-acceptance-model \
  --output docs/benchmarks/2026-10-07-packaged-bge-paused-idle.json
```

Three focused harness self-tests passed, covering CPU-time units, delta-based CPU accounting, and Darwin continuation-row parsing. The saved run retains its original harness hash; the parser correction happened afterward and no old receipt was rewritten.
