# Unmounted renderer preservation bridge lifecycle

This follow-up repairs the existing bridge and prepares its session snapshot
accessor. It does **not** mount the bridge, register an UnsavedEditor participant,
add native commands or establish profile backup readiness.

The bridge retains a cloned request ID plus owner/renderer identity. An exact
native release retires that attempt even if editor preflight failed before the
persistence gate existed. Duplicate and stale events cannot resume a replacement
attempt; replay suppression retains at most 64 identities. Release and disposal
abort asynchronous continuations. A possibly admitted flush stays denied until
its exact native release; disposing its UI retains only that release listener,
without failure/resume callbacks. Preflight-only disposal removes both listeners.

The existing `sessionWriter` now exposes `snapshot(currentDelta, readerWritable)`.
It combines its actual outstanding field groups with a nonadvancing tracker
`peek`, returns an isolated copy, and omits unsupported reader namespaces. It
neither starts a save nor acknowledges fields nor advances tracker baselines.
Raw legacy rows/unknown lexemes remain native-owned; this accessor does not
round-trip them. The existing native 1 MiB total-session bound is unchanged.
Future composition must explicitly re-enqueue the current accumulated snapshot
on matching resume before consuming tracker deltas; a preservation receipt does
not acknowledge this ordinary queue automatically.

## Focused verification, 2026-10-08

The coordinated frontend-only slot was granted, used and explicitly released.
All fixtures are synthetic and no native APIs/bindings changed.

- Four bridge cases cover failed preflight followed by exact release and fresh
  request; full identity and duplicate rejection; event-payload isolation;
  pending-preflight release/disposal; pending-flush disposal and exact cleanup;
  and partial listener-registration failure.
- Two actual renderer persistence cases cover cancellation before admission and
  a release/replacement while draining the same ordinary session queue. Only the
  fresh request may submit, and ordinary writes stay denied until exact release.
- One additional coalescer case covers interleaved pending/failed groups,
  nonconsuming current peeks, caller mutation isolation, and omission of a real
  future-version read-only reader namespace without serializing legacy arrays.

Restoring the old `if (!releasePreservation(...)) return` condition caused the
failed-preflight regression to fail (1 failed, 3 skipped): duplicate matching
release continued to hit the uncleared attempt. The exact source was restored.
The complete affected group then passed **16 cases** (bridge 4, persistence 9,
coalescer 3; 30 ms test execution, 448 ms Vitest run). TypeScript passed in 2.45 s
and scoped ESLint passed in 0.46 s. No broad suite was run.

Actual commands from `ui/`, using the existing dependency cache:

```sh
node_modules/.bin/vitest run src/shell/profile-transition-bridge.test.ts src/shell/persistence.test.ts src/features/reader-history/session-patches.test.ts
node_modules/.bin/tsc --noEmit
node_modules/.bin/eslint src/shell/profile-transition-bridge.ts src/shell/profile-transition-bridge.test.ts src/shell/persistence.ts src/shell/persistence.test.ts src/features/reader-history/session-patches.ts src/features/reader-history/session-patches.test.ts
```

No Cargo, model, browser, package install, installed profile or full-suite check
is required for this renderer-only lifecycle checkpoint. Native editor ownership
and real mounted preservation remain separate reviewed work.
