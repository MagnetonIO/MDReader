# MDReader project integrations

This crate prepares and installs narrowly owned project configuration entries for the packaged MDReader binary. It never searches home directories, discovers repositories, reads histories, runs an agent, modifies client trust, or changes the service's capture grants. Crate tests use synthetic temporary configuration files. The separate
[installed-client driver](../../extensions/vscode/README.md#explicit-installed-client-acceptance)
has also installed and uninstalled project MCP entries in isolated synthetic
profiles using real installed clients. Existing global user settings were unchanged.

## Supported contracts

Official documentation checked on 2026-10-07:

- [Claude Code hooks](https://code.claude.com/docs/en/hooks) documents project `.claude/settings.local.json`, `Stop`/`SubagentStop` visible final messages, and exec-form `command` plus `args`. Generated Claude hooks use that argument-vector form, a three-second timeout and no asynchronous/continuation flags.
- [Codex hooks](https://learn.chatgpt.com/docs/hooks) documents project `.codex/hooks.json`, `Stop`/`SubagentStop`, visible final text and exact-definition trust. Generated Codex commands POSIX-quote every argument. The installer neither trusts hooks nor changes feature/security settings. Local command hooks do not support cloud orchestration.
- [Codex advanced configuration](https://learn.chatgpt.com/docs/config-file/config-advanced) documents `notify` as an argument-vector command receiving one appended JSON argument for `agent-turn-complete`. The optional fallback owns only an absent project-level `notify` key; an existing notifier is a conflict, never overwritten or wrapped. It captures no subagent lifecycle event.
- [Claude MCP](https://code.claude.com/docs/en/mcp) documents project `.mcp.json` and stdio command/argument configuration. [Codex MCP](https://learn.chatgpt.com/docs/extend/mcp) documents trusted project `.codex/config.toml` and `mcp_servers.<name>` stdio entries. Generated MCP configuration uses the single server name `mdreader` and only explicitly selected source IDs.

These current contracts do not prove compatibility with an older installed client. `Capabilities` defaults to unsupported. The owner must supply a measured/probed supported client version and the relevant capability flags before installation. This crate does not infer capabilities from a version string or silently fall back between hooks and notifications. Client hook review, project trust, restart and managed-policy restrictions remain client-controlled acceptance gates.

| Host | Installer status |
| --- | --- |
| macOS local Claude Code/Codex | Synthetic tests pass; Claude 2.1.294 isolated approval/connection and Codex 0.161.0 isolated trust/tool-call observed. User-project/hook acceptance pending. |
| Linux local Claude Code/Codex | POSIX implementation present; runtime acceptance pending. |
| Native Windows | Canonical SQLite install/removal intents, protected native state, full NTFS identity and selected-root replacement implemented; actual Windows client/runtime acceptance remains pending. |
| Cloud orchestration, remote command host, web/mobile agent UI | Unsupported for these local command integrations. No install is attempted when `local_commands` is false. |

Historical Windows GNU compilation predates the installer implementation; the current Windows installer still needs target compilation and runtime acceptance. Mobile readers may consume library content independently; this crate installs no mobile hooks.

## Service-owned authority and packaged entrypoints

`InstallRequest` explicitly supplies `worktree_root`, packaged `executable`, `data_dir`, `policy_path`, `spool_root`, selected `source_ids` and client capabilities. Recommended executable on macOS:

```text
/Applications/MDReader.app/Contents/MacOS/mdreader-app
```

The service first grants the exact project/worktree and durably publishes its current bounded policy at `DATA_DIRECTORY/captures/policies/<projectUUID>.json`. It supplies that path and `DATA_DIRECTORY/captures` as the spool root. `prepare` and `apply` parse the already-existing service artifact, validate it, and compare its complete policy to the current service grant. Pretty and compact JSON are equivalent; no missing, malformed, oversized, revoked or mismatched artifact is adopted. The installer **never writes or deletes a policy artifact**. Opt-out updates and policy revocation remain service authority.

Capture hook/notify installation requires `enabled=true`. Read-only MCP installation can use a capture-disabled policy while still requiring explicit worktree admission and a nonempty bounded selection of library source IDs. The caller must obtain those IDs from the user's explicit library selection and verify source authority in the service. There is no all-library default, inferred sibling worktree or provider credential in generated configuration.

Generated hook arguments are exactly:

```text
mdreader-app --mdreader-capture hook claude POLICY_JSON SPOOL_ROOT
mdreader-app --mdreader-capture hook codex POLICY_JSON SPOOL_ROOT
mdreader-app --mdreader-capture hook codex-notify POLICY_JSON SPOOL_ROOT [appended JSON argument]
```

Only `Stop` is generated when `capture_subagents=false`; otherwise both supported stop events are included. Backfill is an explicit separate sanitized-event operation through the packaged capture entrypoint, never an installed history scanner.

Generated MCP arguments are exactly:

```text
mdreader-app --mdreader-mcp --data-dir ABSOLUTE_DATA_DIRECTORY --source SELECTED_ID [--source SELECTED_ID ...]
```

MCP and notify use argument arrays; Claude hooks use exec form. Codex lifecycle hooks use single-quoted POSIX arguments with embedded apostrophes safely escaped. Paths containing spaces, dollar signs, backticks, semicolons and apostrophes are tested through a real synthetic executable. Control characters, relative paths and `${...}` placeholders are rejected, since clients may expand placeholders before execution. This is not a PowerShell quoting implementation.

## API and preservation

```text
Installer::open(explicit_private_state_directory)
  .prepare(&request, &current_service_policy) -> PreparedChange
PreparedChange::{config_path, policy_path, before_bytes, after_bytes}
Installer::apply(change, &current_service_policy) -> installed_path
Installer::{uninstall, rollback}(&request)
```

The owner can display `after_bytes` for review before applying the same immutable prepared change. Apply revalidates policy, the exact file baseline, receipt state and the worktree's directory identity. Changes after preview yield `Conflict`; the owner must prepare a fresh review. Paths are derived only from the explicit worktree:

| Kind | Target |
| --- | --- |
| `ClaudeHooks` | `.claude/settings.local.json` |
| `ClaudeMcp` | `.mcp.json` |
| `CodexHooks` | `.codex/hooks.json` |
| `CodexMcp`, `CodexNotify` | `.codex/config.toml` |

JSON editing preserves every untouched token, including whitespace, numeric spellings and escape sequences. It appends distinct event groups or inserts only the absent named server. Duplicate keys and malformed/unsupported target containers or hook handler kinds fail closed. Unrelated top-level settings and existing hooks/servers remain unchanged.

TOML is parsed with [toml_edit](https://docs.rs/toml_edit/latest/toml_edit/) for schema/collision validation, but the existing document is never reserialized. New server configuration is appended as a distinct table; `notify` is prepended as a root assignment. This preserves existing comments, dotted keys, byte values, order and missing trailing newline. Sealed inline-table collisions and duplicate/conflicting keys are rejected. Every generated result is parsed again before publication.

The private receipt records exactly which fragments this installation owns. An identical preexisting entry without a receipt is an unmanaged conflict: uninstall cannot adopt or delete it. Reinstall verifies the owned entries and returns the existing configuration unchanged. Changes to an owned fragment, including added fields, cause a conflict and preserve the entire current file. Uninstall removes only exact recorded fragments from a subsequently edited document. It restores the original bytes only when the complete current document still equals the recorded installed bytes. A new unchanged file may be removed; files with unrelated subsequent edits remain. Empty generated JSON containers may remain after surgical removal to preserve those edits. Missing/edited ownership requires explicit review rather than guessing who owns a similar entry.

Each target has its own durable receipt. Installing several targets is a sequence of these operations; it is not a cross-file atomic transaction. The service can call rollback for already-installed targets if a later target fails. Existing Codex inline hooks and other configuration layers are outside this exact target edit and must be included in client capability/trust review; the installer does not rewrite or disable them.

## Persistence and limits

State directories are private (0700), new files are 0600, existing configuration permissions are preserved, and hard links/symlinks/special files are rejected. Worktree/root identity checks and descriptor-relative `NOFOLLOW` operations anchor publication. Temporary bytes are synced, renamed within the parent directory and followed by directory syncing. The journal is durable before configuration mutation; install and removal can resume after interruption. Rollback is the same ownership-preserving removal operation, not a blind backup restore. Receipts may contain copies of configuration values, including unrelated secrets, so they must remain private and never be logged or synced.

A nonblocking advisory lock serializes this installer. Final baseline comparisons detect edits before publication and stale prepared changes. **Portable Unix rename is not an atomic compare-and-swap against noncooperating editors.** A write in the final compare/rename interval remains an incomplete concurrent-editor acceptance gate. Descriptor anchoring prevents path redirection, but does not turn advisory locking into mandatory editor coordination. The product must coordinate editing or provide a host-specific conflict-safe publication mechanism before claiming all concurrent edits are protected.

Bounds are 512 KiB per configuration, 128 KiB per service policy, 4 MiB per receipt, 256 retained receipts and 32 MiB total receipts. Over-limit inputs and storage fail without overwriting the prior configuration. Interrupted temporary files and receipts are retained for explicit owner recovery; no unrelated settings, user histories or policy authority are garbage-collected.

## Verification and incomplete gates

```sh
CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/integrations-target cargo test -p mdreader-integrations
CARGO_TARGET_DIR=/Users/mlong/Library/Caches/MDReader/integrations-target cargo clippy -p mdreader-integrations --all-targets -- -D warnings
```

The suite covers all five adapters, repeated installs/removal/rollback, byte preservation, subsequent/concurrent edits, unmanaged identical entries, changed owned entries, invalid schemas, opt-out, selected-source bounds, current service artifact validation, quoted paths, nonblocking lock failure and replaced directory/symlink boundaries. Real subprocesses are killed before and after install/remove publication, then reopened to verify durable recovery and preservation of a user edit after interrupted publication. Initial no-op installation and unknown hook-group schema tests were observed RED before their implementation/fix.

Observed final checks: 16 test entries pass (including one subprocess helper), Clippy with warnings denied passes, Windows GNU cross-compilation passes, and `git diff --check` passes.

The [bounded client receipt](../mdreader-mcp/validation/client-hosts-2026-10-08.md)
records isolated project MCP install/uninstall byte preservation, measured installed
versions, explicit client approval/trust and real Codex MCP retrieval. Interactive
trust/restart UX, actual user-project setup, model turns/final-output hooks, managed
policy behavior, noncooperating concurrent-editor safety, Linux/Windows runtime
behavior and packaged application wiring remain separate gates. No Task 3
completion or publication is claimed by this crate.
