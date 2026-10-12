# Agent setup and local retrieval

Open **Settings → Agents & integrations**, choose Claude Code or Codex, and press
**Install integration**. Install the agent CLI first. Setup defaults to your user
account for use across projects. For one saved project folder, choose **One
project** under **Installation options**. Setup writes only the client's known
configuration and MDReader skill location; it does not scan your home folder.

Then select the sources the agent may read and press **Enable MCP**. Installation
alone enables neither retrieval nor response capture. Restart the agent to load
its configuration and follow its normal trust/MCP approval prompts.
Configuration status is not proof of a client connection.

## Public skill installer

Advanced installation options shows a public command for the selected agent:

```sh
npx skills add MagnetonIO/MDReader --skill mdreader --agent codex --copy --global
npx skills add MagnetonIO/MDReader --skill mdreader --agent claude-code --copy --global
```

For one project, run the corresponding command from that project's folder and
omit `--global`. The skill lives at `skills/mdreader/SKILL.md` in this public
repository. `--copy` produces a regular file compatible with MDReader's guarded
installer; no developer executable or temporary library path appears in the
command.

This installs the skill instructions. Press **Install integration** in MDReader
to configure the installed app's local MCP launcher, then explicitly choose
sources and **Enable MCP**. The public command does not grant source access,
enable capture, or register agent sessions. Identical externally installed skills
are retained; conflicting edits are preserved and reported.

Choose a project using the native folder picker. The selected folder is saved in
Library & sources without changing the current workspace. Newly added folders
use normal background indexing and watching; choosing an existing paused folder
does not unpause it. Installation scope describes where client configuration is
stored, independently of which folders are indexed or shared through MCP.

## Source access and removal

**Disable MCP** immediately revokes the selected installation's retrieval grant,
including future calls from existing sessions. The integration remains installed.
Removed or disabled sources cannot be retrieved. No HTTP listener is added for MCP;
the agent launches the read-only stdio frontend and the existing authenticated
MDReader owner performs reads.

An agent writing `.md` or `.markdown` inside a chosen indexed folder uses normal
folder indexing. Files outside those folders require an explicitly added source.
Optional final-response capture remains under **Advanced**, disabled unless the
user enables it. Installing the skill does not import histories.

Use **Uninstall integration** in Settings to remove MDReader-owned configuration.
Unrelated client configuration and conflicting user edits are retained. Interrupted
or partial setup can be retried with **Install integration**. If a conflicting
skill or configuration was edited, inspect it before retrying; MDReader does not
overwrite it.

Claude uses `.mcp.json` for project MCP or `.claude.json` for user MCP; its skill is
`.claude/skills/mdreader/SKILL.md`. Codex uses `.codex/config.toml` and
`.agents/skills/mdreader/SKILL.md`, relative to the selected project or user home.
Both scopes use the same `mdreader` registration name so normal client scope
precedence applies. Sources are explicit per installation; adding a source does
not silently share it with an agent. Existing advanced integrations using static
source arguments retain their original permissions; reinstall through this setup
to adopt its revocable Settings control.
