# Proportionate verification

Requested by the user during implementation on 2026-10-07. Test coverage protects
behavior and authority boundaries; test count is not a delivery metric.

- A fix starts with the smallest regression that reproduces the failure. Assert
  observable behavior, not private function calls or a copy of the implementation.
- Run that test while iterating. Run the affected module suite after the slice is
  stable. Expand only for changed shared contracts, a new failure, or an unresolved
  integration risk. Do not repeatedly run the full baseline for a local fix.
- Use unit tests for parsing/state/ranking rules, real SQLite integration for
  transactions/reopen/revocation, subprocess checks for process/crash boundaries,
  and a small number of real browser/native scenarios for host behavior. Avoid
  duplicating the same assertions at every layer.
- Keep rendering tests targeted to visible behavior. Preserve meaningful existing
  regressions; retire obsolete product expectations explicitly when behavior is
  intentionally replaced, while preserving required migration coverage.
- Use injected clocks/gates for scheduling policy. Wall-clock CPU, memory,
  responsiveness and idle measurements belong in short runtime probes; a test
  passing does not establish a resource budget.
- Full native/renderer checks and the three-engine browser matrix run at an
  integration checkpoint. Release/device/migration acceptance and the final fresh
  whole-branch review are separate gates.
- Large actual-model benchmarks are deliberate performance experiments. Record
  binary hash, workload, indexing mode and host contention. Never run 10k/50k
  inference as the default verification of a feature fix.

Examples from the worktree root:

```sh
# A reader source grant regression only
cargo test -p mdreader-core --test sqlite_library repeated_admission
# Background inference behavior, including persisted policy and query bypass
cargo test -p mdreader-service --lib
# The mobile vertical slice; no desktop/provider/release suite
cargo test -p mdreader-app --lib mobile_reader::tests
pnpm --filter @mdreader/app test src/App.mobile.test.tsx
# Indexing controls presentation/acknowledgment
pnpm --filter @mdreader/app test src/shell/IndexingPanel.test.tsx
```

`scripts/check-reader-baseline.sh` remains a checkpoint command. It intentionally
runs broad legacy and reader suites and is not the per-edit command.
