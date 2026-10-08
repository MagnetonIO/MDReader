# Strict native JSON startup checkpoint

Root owns minor `feat/native-strict-persistence` in `MDReader-native-persistence`,
based on integration `3f197ca`. It composes strict preservation checkpoint
`bdd190d` and real actor HTTPS acceptance `f6977dc` (local cherry-pick `a261d5f`).
Destination is `dev-reader-next`; remove the minor normally after acceptance.

## Actual source behavior

Unix desktop startup binds the OS-selected app-data directory with `SourceRoot`
before exposing renderer persistence. A retained exclusive lifecycle lock admits
one JSON owner. Startup does not traverse or inventory user files. The two real
session/settings participants register before ordinary effects; the other six
remain missing, so no profile-wide preservation capability can be issued.

The fixed async commands choose the native window label and owner internally;
paths, raw keys and readiness booleans are absent from the renderer contract.
Strict writes bind renderer/owner epochs and exact generations, preserve unknown
raw JSON, and acknowledge file plus directory synchronization. A failed read
blocks saves until an explicit successful matching reload. The Unix desktop app
does not install the generic store plugin, preventing its exit-time writer from
competing with these files. Windows/mobile keep the existing ordinary plugin
path; installed preservation remains unsupported there at this checkpoint.

Renderer persistence/close lifecycle is extracted from the oversized AppShell
into one small hook. Background/debounce errors are handled with a fixed UI
diagnostic. Failed hydration does not enqueue render defaults, and the dirty
document close guard remains active. A rejected/timed-out save keeps the window
open and permits retry; the two-second bound no longer authorizes data loss.
This is ordinary saving, not the editor preflight/profile freeze handshake.

## Focused verification

The sole heavy slot used two Cargo jobs, locked/offline dependencies and the
existing `CatDB/cargo-target-v1` cache. No model, packaging, installed profile or
live account was used.

```sh
cargo check --locked --offline -p mdreader-app --lib
cargo test --locked --offline -p mdreader-app --lib profile_transition:: -- --test-threads=1
```

The actual Tauri crate check passed (57.19 s). Seven actual native cases passed
(50.01 s compilation, 0.27 s execution), covering JSON/root/receipt replacement,
future/duplicate/malformed inputs, exact archival preimages, all-capability
finalization and the new startup test. The startup case retains an unclassified
file and huge unknown JSON numeric lexeme, rejects a competing owner and refuses
preservation before any renderer request while six participants are missing.

The same compiled executable ran only `tests::export_bindings_up_to_date`
(one pass, 0.06 s) and `tests::no_command_runs_on_the_ui_thread` (one pass,
0.29 s). Rust generated the five persistence command bindings; none were edited
by hand. No hundreds-case native suite was executed.

Frontend persistence/related-rail cases: nine pass (final 1.58 s). Close/quit/failed
hydration cases: nine pass (1.59 s), including timeout retention/retry and failed
save retention. The new hydration test initially had a test-only store-name
typo; correcting it to the actual editor store made the selected suite pass.
TypeScript and scoped ESLint pass. Compatible dependency links reused existing
caches; no package installation occurred.

Extending the existing hydration regression also exposed a real retry defect:
a rejected registration promise was cached permanently, so a later explicit
successful reload still returned defaults. The new regression failed against
that behavior. Registration and host lookup now clear only their own rejected
cached promise; an explicit retry still has to verify the newest exact native
identity before writes are admitted. The focused nine cases then passed. No
automatic legacy fallback or blind write retry was added.

## Remaining release gates

The actual app check reports unused legacy/unwired code warnings (92 in the
ordinary library, seven in the lib-test target). These were not suppressed;
strict full native Clippy and legacy cleanup remain release gates. The command
and owner fixtures do not establish native GUI lifecycle or packaged upgrades.
All eight actual owners, service-process backup transport, held editor preflight,
durable selector startup/activation/rollback, Windows/mobile durability and
installed-library acceptance remain unfinished. No preservation UI is enabled
by this checkpoint and no installed library or user browser storage changed.
