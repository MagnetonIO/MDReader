"use strict";
const fs = require("node:fs/promises");
const path = require("node:path");
const assert = require("node:assert/strict");
const { execFile } = require("node:child_process");
const { promisify } = require("node:util");
const { startRpc, cleanEnvironment } = require("./stdio");
const execute = promisify(execFile);

async function install(fixture, kind, clientVersion) {
  const before = await fs.readFile(path.join(fixture.workspace, kind === "claude_mcp" ? ".mcp.json" : ".codex/config.toml"));
  const review = await fixture.request({ method: "prepare_integration", params: { selection: { kind, project_id: fixture.policy.project_id, worktree_root: fixture.workspace, executable: fixture.launcher, source_ids: [fixture.selected], capabilities: { local_commands: true, claude_exec_hooks: false, codex_lifecycle_hooks: false, codex_notify: false, client_version: clientVersion } } } });
  assert.equal(review.capabilitiesVerified, false);
  await fixture.request({ method: "apply_integration", params: { preparation_id: review.preparationId } });
  return { preparationId: review.preparationId, before, path: review.configPath };
}
async function uninstall(fixture, installed) {
  await fixture.request({ method: "uninstall_integration", params: { preparation_id: installed.preparationId } });
  assert.deepEqual(await fs.readFile(installed.path), installed.before);
}
async function claudeProbe(fixture) {
  const profile = path.join(fixture.root, "claude-profile");
  await fs.mkdir(profile, { mode: 0o700 });
  const env = { ...cleanEnvironment(), CLAUDE_CONFIG_DIR: profile };
  const version = (await execute("claude", ["--version"], { env })).stdout.trim();
  const installed = await install(fixture, "claude_mcp", version);
  try {
    const pending = (await execute("claude", ["mcp", "get", "mdreader"], { cwd: fixture.workspace, env, timeout: 20000, maxBuffer: 65536 })).stdout;
    assert.match(pending, /Pending approval/iu);
    // Documented user approval, scoped to this disposable native client profile.
    // No bypass-permissions flag, global config edit or credential copy occurs.
    await fs.writeFile(path.join(profile, "settings.json"), JSON.stringify({ enabledMcpjsonServers: ["mdreader"] }), { mode: 0o600 });
    const approved = (await execute("claude", ["mcp", "get", "mdreader"], { cwd: fixture.workspace, env, timeout: 20000, maxBuffer: 65536 })).stdout;
    assert.match(approved, /Connected/iu);
    return { version, projectConfigInstalled: true, pendingBeforeExplicitApproval: true, explicitIsolatedUserApprovalConnected: true, interactiveWorkspaceTrust: "not_exercised", modelInvocation: "not_exercised_no_credentials_copied" };
  } finally { await uninstall(fixture, installed); }
}
async function codexSession(fixture, profile, discover = false) {
  // CODEX_HOME is used only for its documented child-client profile purpose;
  // the parent process environment and existing native profile are untouched.
  const env = { ...cleanEnvironment(), CODEX_HOME: profile };
  const rpc = startRpc("codex", ["app-server", "--stdio"], { cwd: fixture.workspace, env });
  try {
    await rpc.request("initialize", { clientInfo: { name: "mdreader_synthetic_acceptance", version: "1.0.0" }, capabilities: { experimentalApi: true } });
    rpc.notify("initialized");
    const config = await rpc.request("config/read", { cwd: fixture.workspace, includeLayers: true });
    if (!discover) return config;
    const started = await rpc.request("thread/start", { cwd: fixture.workspace, ephemeral: true, sandbox: "read-only", approvalPolicy: "never" });
    const threadId = started.thread.id;
    const catalog = await rpc.request("mcpServerStatus/list", { threadId, serverName: "mdreader", limit: 10, detail: "toolsAndAuthOnly" });
    const server = catalog.data.find((item) => item.name === "mdreader");
    assert.ok(server);
    const toolNames = Object.values(server.tools).map((tool) => tool.name).sort();
    assert.deepEqual(toolNames, ["index_status", "list_sources", "read_document", "related", "resolve_reference", "search"]);
    const result = await rpc.request("mcpServer/tool/call", { threadId, server: "mdreader", tool: "read_document", arguments: { source_id: fixture.selected, path: fixture.reference.path, revision: fixture.reference.revision } });
    assert.equal(result.isError, false);
    assert.equal(result.structuredContent.data.revision, fixture.reference.revision);
    assert.ok(result.structuredContent.data.markdown.includes("NebulaAnchor exact visible body."));
    assert.ok(result.content[0].text.startsWith("<!-- mdreader-retrieval -->"));
    return { ...config, discovery: { sixToolsDiscovered: true, actualCodexMcpReadReturnedExactRevisionBody: true, retrievalMarkerPreserved: true } };
  } finally { await rpc.close(); }
}
async function codexProbe(fixture) {
  const profile = path.join(fixture.root, "codex-profile");
  await fs.mkdir(profile, { mode: 0o700 });
  const version = (await execute("codex", ["--version"], { env: cleanEnvironment() })).stdout.trim();
  const installed = await install(fixture, "codex_mcp", version);
  try {
    const untrusted = await codexSession(fixture, profile);
    const untrustedHasServer = Boolean(untrusted.config?.mcp_servers?.mdreader);
    assert.equal(untrustedHasServer, false);
    await fs.writeFile(path.join(profile, "config.toml"), `[projects.${JSON.stringify(fixture.workspace)}]\ntrust_level = "trusted"\n`, { mode: 0o600 });
    const trusted = await codexSession(fixture, profile, true);
    assert.ok(trusted.config?.mcp_servers?.mdreader);
    return { version, projectConfigInstalled: true, untrustedProjectIgnored: true, explicitIsolatedProjectTrustLoadedServer: true, ...trusted.discovery, modelInvocation: "not_exercised_no_credentials_copied" };
  } finally { await uninstall(fixture, installed); }
}
module.exports = { claudeProbe, codexProbe, execute, install, uninstall };
