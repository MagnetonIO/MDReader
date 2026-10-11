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

## Portable public skill — publication pending

The proposed public repository is `MagnetonIO/mdreader-agent`; the main MDReader
repository is private. Publication awaits explicit approval and availability
verification. **The examples below are not currently available installer
commands.** Settings shows the pending status rather than a broken command or a
machine-specific executable/profile path. **Install integration** works directly
in MDReader.

After publication is verified, the planned user-level skill commands are:

```sh
npx skills add MagnetonIO/mdreader-agent --skill mdreader --agent codex --global
npx skills add MagnetonIO/mdreader-agent --skill mdreader --agent claude-code --global
```

For project-level installation, run the corresponding command from that project's
working directory and omit `--global`.

The public installer installs the skill file only. It does not register MDReader's
local MCP launcher, grant source access, or enable capture. MDReader's native
**Install integration** registers the local MCP configuration and retains an
identical externally installed skill; conflicting skill edits are preserved and
reported. Select sources and explicitly **Enable MCP** afterward. A local launcher
belongs to that MDReader installation and library; it is not a portable public
package command.

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
