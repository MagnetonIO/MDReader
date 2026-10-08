"use strict";
const fs = require("node:fs/promises");
const path = require("node:path");
const os = require("node:os");
const net = require("node:net");
const { spawn } = require("node:child_process");
const { once } = require("node:events");
const { cleanEnvironment } = require("./stdio");
const { ReaderClient } = require("../../src/reader-client");

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function request(directory, request) {
  const endpoint = JSON.parse(await fs.readFile(path.join(directory, "reader-endpoint.json"), "utf8"));
  return new Promise((resolve, reject) => {
    const socket = net.createConnection({ host: "127.0.0.1", port: endpoint.port });
    let bytes = "";
    socket.setTimeout(5000, () => socket.destroy(new Error("fixture transport timeout")));
    socket.on("connect", () => socket.end(JSON.stringify({ protocol: 1, token: endpoint.token, request }) + "\n"));
    socket.on("data", (chunk) => {
      bytes += chunk;
      if (bytes.length > 1024 * 1024) return socket.destroy(new Error("fixture reply bound"));
      if (!bytes.includes("\n")) return;
      socket.destroy();
      try {
        const reply = JSON.parse(bytes.split("\n")[0]);
        if (reply.error) reject(new Error(`fixture RPC rejected: ${reply.error.code ?? "unavailable"}`));
        else resolve(reply.result);
      } catch (error) { reject(error); }
    });
    socket.on("error", reject);
  });
}
function quote(value) { return `'${value.replaceAll("'", "'\"'\"'")}'`; }
async function createFixture({ app, service, mcp, capture }) {
  const root = await fs.realpath(await fs.mkdtemp(path.join(process.platform === "darwin" ? "/tmp" : os.tmpdir(), "mdreader-clients-")));
  await fs.chmod(root, 0o700);
  const workspace = path.join(root, "workspace");
  const notes = path.join(workspace, "notes");
  const hidden = path.join(root, "denied");
  const data = path.join(root, "data");
  await fs.mkdir(notes, { recursive: true });
  await fs.mkdir(hidden);
  await fs.writeFile(path.join(notes, "plan.md"), "# Acceptance Plan\n\nNebulaAnchor exact visible body.\n\n[Next](next.md#destination)\n");
  await fs.writeFile(path.join(notes, "next.md"), "# Destination\n\nSelected linked target.\n");
  await fs.writeFile(path.join(hidden, "hidden.md"), "# Hidden\nForbiddenSentinel outside selected scope.\n");
  await fs.mkdir(path.join(workspace, ".codex"));
  await fs.writeFile(path.join(workspace, ".codex/config.toml"), "# synthetic unrelated comment must survive\nmodel_reasoning_effort = \"low\"\n");
  await fs.writeFile(path.join(workspace, ".mcp.json"), "{ \"mcpServers\": {} }\n");
  const { execFileSync } = require("node:child_process");
  execFileSync("git", ["init", "--quiet", workspace]);
  const launcher = app ?? path.join(root, "synthetic-entrypoint");
  if (!app) await fs.writeFile(launcher, `#!/bin/sh\ncase "$1" in\n--mdreader-mcp) shift; exec ${quote(mcp)} "$@" ;;\n--mdreader-capture) shift; exec ${quote(capture)} "$@" ;;\n*) exit 2 ;;\nesac\n`, { mode: 0o700 });
  const child = spawn(app ?? service, [...(app ? ["--mdreader-service"] : []), "--no-model", data], { stdio: "ignore", env: cleanEnvironment() });
  const ended = once(child, "exit");
  let fixture;
  async function stop() { if (child.exitCode === null && child.signalCode === null) child.kill(); await ended; }
  try {
    let ready = false;
    for (let attempt = 0; attempt < 200; attempt++) {
      try { await request(data, { method: "ping" }); ready = true; break; }
      catch { if (child.exitCode !== null) break; await delay(20); }
    }
    if (!ready) throw new Error("synthetic service did not start");
    async function admit(directory, name) {
      const canonical = await fs.realpath(directory);
      const stat = await fs.stat(canonical);
      const source = await request(data, { method: "admit", params: { name, path: canonical, identity: { platform: "unix", device: stat.dev, inode: stat.ino } } });
      await request(data, { method: "index", params: { source_id: source.id } });
      return source.id;
    }
    const selected = await admit(workspace, "Synthetic selected workspace");
    const denied = await admit(hidden, "Synthetic denied notes");
    const policy = { enabled: false, project_id: "synthetic-client-acceptance", project_root: workspace, repository_root: workspace, worktree_roots: [workspace], capture_subagents: false, excluded_paths: [], excluded_sessions: [], selected_backfill_sessions: [] };
    await request(data, { method: "configure_capture", params: { policy } });
    const client = new ReaderClient({ dataDirectory: data, sourceIds: [selected] });
    const [hit] = await client.search("NebulaAnchor");
    if (!hit || hit.source_id !== selected || hit.path !== "notes/plan.md") throw new Error("fixture evidence missing");
    fixture = { root, workspace, notes, data, selected, denied, launcher, executableMode: app ? "packaged_app" : "standalone", reference: { ...hit, line: hit.line_start }, policy };
    await fs.writeFile(path.join(root, "fixture.json"), JSON.stringify(fixture), { mode: 0o600 });
    return { ...fixture, request: (value) => request(data, value), stop, cleanup: async () => { await stop(); await fs.rm(root, { recursive: true, force: true }); } };
  } catch (error) { await stop(); await fs.rm(root, { recursive: true, force: true }); throw error; }
}
module.exports = { createFixture, request, delay };
