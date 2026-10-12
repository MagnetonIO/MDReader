# Agent projects and document access

Open **Settings → Agents & integrations**. Use **Choose project folder** to select
folders with a real picker, then select **Projects to organize** in the Codex or
Claude Code card and press **Connect**. Install that agent CLI first. MDReader
indexes chosen folders and subfolders in the background; paused folders can be
resumed in Library & sources. Adding a folder does not share its contents with
an agent or scan the rest of your home folder.

Setup defaults to all projects. **Advanced → Installation options → One project**
changes where client configuration is installed. Scope is separate from folder
permissions. Restart the agent to load its configuration and follow its usual
trust/MCP prompts. The service shows **Waiting for agent** until a handshake or
report arrives, then **Connected** with last-seen time. Configuration alone is
not proof of a connection.

## Portable skill command

Advanced shows the command for the selected agent and scope:

```sh
npx skills add MagnetonIO/MDReader --skill mdreader --agent codex --copy --global
npx skills add MagnetonIO/MDReader --skill mdreader --agent claude-code --copy --global
```

For one project, run inside that folder and omit `--global`. These commands copy
`skills/mdreader/SKILL.md` from the public repository. They install instructions;
they do not install client MCP configuration or enable any permissions. MDReader's
Connect action uses the installed application internally; terminal instructions
contain no developer executable or temporary profile path.

## Two directions, separate permissions

**Projects to organize** permits reporting project/session metadata and existing
project-relative Markdown paths. Folder watching also finds unreported files.
Projects → All documents shows every indexed Markdown file; session lists show
reported associations. A file may appear in several sessions. Attribution is
reported metadata, not proof of authorship. No transcript, prompt, reasoning,
credential, or raw tool payload is retained by session reporting.

**Optional source access** separately permits **Read** and **Create/edit** for
selected documents. Captures remain read-only. MCP create refuses existing
files; update requires the exact revision. Conflicts preserve the original and
proposed body. Retain the operation UUID and use `inspect_operation` after a
pending or interrupted save; do not resend with a new UUID. There is no force
update or delete tool. Provider permissions remain authoritative.

Supported client hooks report SessionStart, SessionEnd and recognized file tools.
They do not attribute arbitrary shell writes. Folder watching remains the fallback,
and the public skill can explicitly report Markdown associations. Response capture
is a separate advanced option and remains off unless explicitly enabled.

## Disable, update and uninstall

Turn **Agent service** off to revoke reporting, reading and editing, including
future calls from existing sessions. Unrelated source-folder indexing continues.
Removed/disabled roots cannot be registered or read; paused projects reject new
session reporting. No HTTP listener is added for MCP: the agent launches a stdio
frontend and the existing authenticated native owner handles its operations.

**Uninstall integration** removes only receipt-owned configuration and skill
files. Unrelated settings and changed user files are retained. Interrupted setup
can be retried. Equivalent JSON reformatting is accepted; an owned launcher can
be updated while preserving the same library/grant binding. A conflicting owned
value remains an error, with existing files kept.

Claude uses `.mcp.json` and `.claude/settings.local.json` for a project, or
`.claude.json` and `.claude/settings.json` for user setup. Its skill is
`.claude/skills/mdreader/SKILL.md`. Codex uses `.codex/config.toml`,
`.codex/hooks.json`, and `.agents/skills/mdreader/SKILL.md`, relative to the selected
project or user home. Existing static-source integrations retain their original
permissions; connecting through Settings creates revocable controls.

Native packaging, notarization, Windows/Linux GUI and mobile acceptance remain
separate release gates. Current evidence is in the
[development status ledger](sqlite-reader-next-status.md).
