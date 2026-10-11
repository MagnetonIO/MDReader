# Agent setup and local retrieval

Open **Settings → Agents & integrations**, choose Claude Code or Codex, then choose
**Project** or **User**. Project setup uses a saved local folder. User setup writes
only the client's known user configuration and MDReader skill location; it does
not scan the home folder. Install the client CLI first.

Copy and run the command shown in Settings. The command invokes the installed
MDReader executable directly, so no global MDReader command or npm package is
required. Its shape is:

```text
ABS_MDREADER_EXECUTABLE --mdreader-agent install --agent codex --scope project --data-dir ABS_READER_PROFILE --project ABS_SAVED_FOLDER
```

For user installation select `--scope user` and omit `--project`. Project
installation without `--project` uses the command's working directory. The exact
profile and quoted executable are supplied by Settings; do not substitute an
installed legacy library or another application's cache.

The installer adds the versioned `mdreader` skill and a local stdio MCP launcher.
It preserves unrelated client configuration and refuses conflicting user edits.
Installation does not enable retrieval or response capture. After installation,
refresh Settings, select the sources the agent may search, and press **Enable MCP**.
Restart the agent to load newly installed configuration and follow its normal
project trust/MCP approval prompts. Configuration status is not proof of a client
connection.

**Disable MCP** immediately revokes the selected installation's retrieval grant,
including future calls from existing sessions. The plugin remains installed.
Removed or disabled sources cannot be retrieved. No HTTP listener is added for MCP;
the agent launches the read-only stdio frontend and the existing authenticated
MDReader owner performs reads.

An agent writing `.md` or `.markdown` inside a chosen indexed folder uses normal
folder indexing. Files outside those folders require an explicitly added source.
Optional final-response capture remains under **Advanced**, disabled unless the
user enables it. Installing the skill does not import histories.

Use **Uninstall** in Settings, or replace `install` with `uninstall` in the copied
command. Only MDReader-owned configuration and an unchanged owned skill are
removed; user changes cause a conflict and are retained. Interrupted or partial
setup can be retried with the same command. If a conflicting skill or configuration
was edited, inspect it before retrying; MDReader does not overwrite it.

Claude uses `.mcp.json` for project MCP or `.claude.json` for user MCP; its skill is
`.claude/skills/mdreader/SKILL.md`. Codex uses `.codex/config.toml` and
`.agents/skills/mdreader/SKILL.md`, relative to the selected project or user home.
Both scopes use the same `mdreader` registration name so normal client scope
precedence applies. Sources are explicit per installation; adding a source does
not silently share it with an agent. Existing advanced integrations using static
source arguments retain their original permissions; reinstall through this setup
to adopt its revocable Settings control.
