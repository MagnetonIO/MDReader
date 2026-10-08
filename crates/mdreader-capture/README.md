# MDReader capture foundation

This crate enqueues opted-in visible Claude Code/Codex final output into a bounded private filesystem spool. The one reader service projects ordinary Markdown and acknowledges only after committing durable capture admission and an indexing job. Hook processes never open SQLite, invoke a model, start the service, or request agent continuation.

## Supported inputs and honest limits

Official documentation checked on 2026-10-07:

- [Claude Code hook reference](https://code.claude.com/docs/en/hooks): `Stop` and `SubagentStop` expose `last_assistant_message`, with session/cwd and subagent identifiers. `prompt_id`, when present, supplies turn correlation. This adapter records its kind as `prompt_id`; an unavailable identifier stays null. A subagent handback report is not necessarily the subagent's closing message; that report is not captured by this adapter.
- [Codex lifecycle hooks](https://learn.chatgpt.com/docs/hooks): local command `Stop` and `SubagentStop` provide visible final text and `turn_id`; project hook definitions require trust. Hook stdin is supported. Cloud-orchestrated command hooks and remote extension hosts are outside this adapter's supported execution host.
- [Codex advanced configuration](https://learn.chatgpt.com/docs/config-file/config-advanced): the legacy `notify` command receives one JSON argument for `agent-turn-complete`, including `thread-id`, `turn-id`, `cwd`, and `last-assistant-message`. No other notification event is assumed. Hook and notification transport delivery for the same Codex occurrence deduplicates.

No transcript/history path is read. Unknown input fields, user prompts, raw tool logs and hidden reasoning fields are discarded before persistence. Only the final visible message field is eligible. Tool, compaction, startup and error events are excluded. Streaming output, interrupts, older versions without final text, plan-file discovery, and tool-delivered subagent reports are not silently reconstructed. Missing visible output has an explicit skip reason.

Known reasoning tags, generated projection markers and `mark_retrieval()` markers suppress capture. The service must prefix every MCP retrieval text result with `mark_retrieval()`. A markerless assistant paraphrase cannot be reliably classified as a retrieval echo from a final-text hook alone; this is an incomplete semantic echo acceptance gate, not a guarantee of provenance inference.

## Project opt-in and selected backfill

The owner passes an exact policy path; there is no upward policy discovery, implicit project admission, or private history scan. Example policy (replace paths before use):

```json
{
  "enabled": true,
  "project_id": "chosen-project",
  "project_root": "/absolute/project",
  "repository_root": "/absolute/repository",
  "worktree_roots": ["/absolute/project"],
  "capture_subagents": false,
  "excluded_paths": ["private", "secrets"],
  "excluded_sessions": [],
  "selected_backfill_sessions": []
}
```

`enabled` defaults to false. Each worktree must be explicitly admitted; no sibling is automatically included. Exclusions are relative directory prefixes, applied to the canonical event cwd. A cwd symlink escaping the grant is excluded. A granted root itself cannot be a symlink. System ancestor aliases such as macOS `/var` are normalized. Projection revalidates the current policy against the captured canonical project/repository/worktree roots.

`backfill` accepts an explicitly supplied sanitized final-output event only for a session in `selected_backfill_sessions`. It does not discover or read historical files. Changing that list or project opt-in before projection also prevents projection. Excluded events never enter the queue. Disabling a project retains already queued events for explicit owner recovery; it does not destroy them.

### Desktop selected-output review

Agent capture settings provide a separate **Import selected past output** panel. Enable capture for the current local folder, explicitly choose one UTF-8 final-output JSON event (at most 256 KiB), choose its actual agent format, then review the supplied session/optional turn/subagent, working folder and exact visible text. Confirming selects that supplied session and queues only that reviewed output. Missing IDs, original event time and repository revision remain unavailable; import capture time is the current receipt time. The format choice is user-supplied authorship, not agent verification.

This is not a transcript importer. JSONL histories, directories, plan files and plain Markdown without supplied event provenance are unsupported. Paths mentioned by unknown fields are never opened. Review uses the same visible-output parser as hooks and returns only canonical visible fields; unknown reasoning/tool/prompt fields are not rendered or persisted. Known reasoning/retrieval markers and all existing project/subagent/session/path exclusions still apply. Only current retained root/source grants can accept a review or enqueue: selecting a session never recaptures or regrants a folder.

Queue success means durable enqueue, not completed projection/indexing. If a reply is lost, explicitly retry the same canonical reviewed event; the existing occurrence identity deduplicates it across restart. Session selection may have committed even if enqueue failed or its reply was lost. If policy publication was interrupted before its SQLite save, only the same reviewed enqueue can complete its exact next selection artifact; unrelated mismatches remain closed. **Stop backfill for session** narrows the current policy; it does not delete pending recovery events, original outputs, captured Markdown or live-capture settings. Removing selection before projection prevents that selected-backfill event from being admitted until explicitly reselected.

## CLI and configuration fragments

Build the crate with `cargo build -p mdreader-capture`. Invocation:

```text
mdreader-capture hook claude POLICY_JSON SPOOL_ROOT       # hook JSON on stdin
mdreader-capture hook codex POLICY_JSON SPOOL_ROOT        # hook JSON on stdin
mdreader-capture hook codex-notify POLICY_JSON SPOOL_ROOT JSON_ARGUMENT
mdreader-capture backfill claude POLICY_JSON SPOOL_ROOT   # selected sanitized JSON on stdin
```

The program emits only `{}` on stdout. Success/duplicate/excluded delivery exits 0. Persistence, input or capacity failure exits 1 with a fixed diagnostic containing no captured content or private path. It never exits 2 or returns blocking/continuation/context fields. Queue success means the enqueue event is durably published; it does not claim indexing, approval, installation or sync.

The following is a **reviewable fragment**, not a live installation. Merge only its event entries into the relevant project's existing configuration, preserving unrelated entries. Use `claude` for Claude Code's project settings and `codex` for Codex's project hooks. Replace executable/policy/spool paths with reviewed absolute paths:

```json
{
  "hooks": {
    "Stop": [{"hooks": [{
      "type": "command",
      "command": "\"/absolute/bin/mdreader-capture\" hook codex \"/absolute/policy.json\" \"/absolute/library/capture\"",
      "timeout": 3
    }]}],
    "SubagentStop": [{"hooks": [{
      "type": "command",
      "command": "\"/absolute/bin/mdreader-capture\" hook codex \"/absolute/policy.json\" \"/absolute/library/capture\"",
      "timeout": 3
    }]}]
  }
}
```

No configuration is installed, modified or removed by this crate. Use the synchronous bounded enqueue: background-hook termination may precede durable publication. Legacy Codex notification configuration is user-level; strict project policy still applies:

```toml
notify = ["/absolute/bin/mdreader-capture", "hook", "codex-notify", "/absolute/policy.json", "/absolute/library/capture"]
```

## Service integration and authority

The service creates `Spool::open(profile.join("capture"), Limits::default())`. It may call `recover_temporary()` during its recovery pass; this removes only incomplete temporary writes and never queued events or Markdown. Drain a bounded `pending(limit)` batch as follows:

1. `read_pending(id)` provides the bounded record and project ID for current-policy lookup.
2. `project(id, &current_policy)` revalidates opt-in and atomically writes the Markdown body with rich occurrence provenance.
3. Admit `markdown_root()` through the reader's private source grant, then commit capture identity/provenance and the indexing job to `library.sqlite` in the one service writer.
4. `acknowledge(id)` writes a durable receipt and then removes the pending event. It rejects an absent or invalid projection. Index/model availability is independent of this acknowledgment.

An error at any earlier step leaves the pending event intact. Restarting after projection repeats one occurrence. Restarting after receipt creation and before pending removal safely acknowledges again. Duplicate delivery revalidates the pending record or authoritative Markdown and confirms file/directory durability again; a receipt never masks a missing or damaged body. The drain consumer must finish its durable transaction before calling `acknowledge`; the capture crate cannot verify another component's SQLite transaction.

`capture/markdown/<content-id>.md` is content authority, never a disposable index. Frontmatter uses a JSON object valid as YAML, followed by the exact visible body. `content_revision` is BLAKE3 over body bytes. The reader's file revision additionally reflects provenance metadata changes; do not confuse those two revisions. Repository Git revision is explicitly null because these hooks provide no trustworthy value.

Body identity includes project and body bytes. Distinct agent/session/turn/subagent/worktree/cwd occurrences share a body without losing authorship; each is recorded in frontmatter. Exact occurrence replay is suppressed. Capture origin/timestamp do not create a new authorship occurrence. Histories inherited by a fork are never scanned; an identical visible output from a new live fork retains that new occurrence without duplicating its body. If an older agent supplies no turn identifier, repeated identical output in that session is conservatively deduplicated.

## Durability, budgets and incomplete host gates

On Unix, files are created with mode 0600 in mode 0700 directories. Temporary writes are synced, atomically renamed on the same filesystem, then both affected directories are synced before acknowledgment. The spool root identity is pinned and regular-file/symlink checks reject replaced roots and projection escapes. Mutations use an advisory nonblocking exclusive lock: busy returns immediately without a retry loop, model work or agent continuation. A failed hook must be retried explicitly; there is no fabricated success.

Defaults: 256 KiB input/event, 128 pending events, 8 MiB pending bytes, 2 MiB Markdown projection (including frontmatter, matching reader document admission), 1024 provenance occurrences per body. Exceeding a bound rejects the complete operation and retains prior durable state. Owner-selected limits are themselves bounded; the projection cap cannot exceed the reader's 2 MiB document limit. Immutable Markdown and receipts are retained after draining; storage-retention/export policy belongs to the library, not automatic index cleanup. A full provenance body remains readable while the next occurrence stays queued with an explicit capacity error.

**Windows durable publication is not implemented in this slice.** `Spool::open` fails with `Unsupported` before any filesystem mutation rather than treating directory syncing as a successful no-op. A correct private-ACL/reparse-safe, handle-pinned implementation with `FlushFileBuffers` and write-through publication, followed by real Windows crash/restart tests, is required for v1. Unix compilation or Windows cross-compilation is not Windows runtime acceptance.

Live hook trust/install/uninstall preservation, agent-version compatibility, real Claude→Codex/reader/MCP retrieval, host latency budgets, Linux/mobile runtime behavior, markerless paraphrase suppression and Windows durability remain separate acceptance gates. This crate's tests do not complete Task 3 or authorize publishing/merging/installing anything.

## Validation

```sh
CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/capture-target cargo test -p mdreader-capture
```

The synthetic temporary-filesystem suite covers enqueue/reopen/ack, corrupt events and receipts, body revisions, replay/fork/subagent authorship, hook/notify deduplication, policy revocation, source/exclusion boundaries, queue/provenance bounds, nonblocking lock failure, orphan recovery and symlink escapes. Real hook subprocesses prove both agents enqueue with the reader/model absent, JSON output does not request continuation, selected synthetic backfill is explicit, and a process killed after projection replays without losing the Markdown body or duplicating provenance.

Observed RED→GREEN includes the no-op CLI failing all three initial subprocess tests, corrupt pending files being incorrectly reported as duplicates, and hook/notify delivery creating two occurrences for the same Codex turn. Each behavior now has a passing regression.

The packaged/standalone hook retries only explicit queue-lock contention for at most one second, within the three-second client hook timeout. Full queues, invalid policies and persistence failures are not retried or reported as success. A permanently held lock still fails promptly; capture never requests agent continuation.
