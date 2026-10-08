"use strict";
const fs = require("node:fs/promises");
const path = require("node:path");
const os = require("node:os");
const crypto = require("node:crypto");
const { createFixture } = require("./fixture");
const { claudeProbe, codexProbe, execute } = require("./clients");
const { hostProbe } = require("./host");
const { mcpProbe } = require("./mcp");
const { captureProbe } = require("./capture");

async function digest(filename) { return crypto.createHash("sha256").update(await fs.readFile(filename)).digest("hex"); }
async function main() {
  if (process.platform !== "darwin") throw new Error("This acceptance driver currently requires the installed macOS clients.");
  const args = process.argv.slice(2);
  const options = {};
  for (let index = 0; index < args.length; index += 2) {
    if (!["--app", "--service", "--mcp", "--capture", "--vsix", "--output", "--clients"].includes(args[index]) || !args[index + 1] || options[args[index]]) throw new Error("Supply --app ABS_EXECUTABLE or --service/--mcp/--capture, plus --output and --vsix for vscode.");
    if (args[index] === "--app" && !path.isAbsolute(args[index + 1])) throw new Error("--app must be an absolute packaged executable path.");
    options[args[index]] = args[index] === "--clients" ? args[index + 1] : path.resolve(args[index + 1]);
  }
  if (!options["--output"]) throw new Error("An explicit --output receipt path is required.");
  const standalone = ["--service", "--mcp", "--capture"];
  if (options["--app"] ? standalone.some((key) => options[key]) : !standalone.every((key) => options[key])) throw new Error("Choose one packaged --app or all three standalone executable paths.");
  if (options["--app"]) {
    const stat = await fs.stat(options["--app"]);
    if (!stat.isFile()) throw new Error("--app must name the packaged executable, not an app directory.");
    await fs.access(options["--app"], require("node:fs").constants.X_OK);
  }
  const clients = (options["--clients"] ?? "claude,codex,vscode,mcp").split(",");
  if (!clients.length || new Set(clients).size !== clients.length || clients.some((client) => !["claude", "codex", "vscode", "mcp", "capture"].includes(client))) throw new Error("Select only claude,codex,vscode,mcp,capture acceptance phases.");
  if (clients.includes("vscode") && !options["--vsix"]) throw new Error("The vscode phase requires --vsix.");
  const baselineNames = [".claude.json", ".codex/config.toml", "Library/Application Support/Code/User/settings.json"];
  const baseline = new Map();
  for (const name of baselineNames) {
    try { baseline.set(name, await digest(path.join(os.homedir(), name))); }
    catch (error) { if (error.code !== "ENOENT") throw error; baseline.set(name, null); }
  }
  const receipt = { syntheticOnly: true, platform: process.platform, architecture: process.arch, checkedAt: new Date().toISOString(), executableMode: options["--app"] ? "packaged_app" : "standalone", artifactHashes: {}, checks: {}, limitations: [...(options["--app"] ? [] : ["synthetic launcher routes packaged flags to standalone binaries; packaged-app entrypoint is not accepted"]), "no model invocation or credentials copied", "interactive agent workspace trust dialogs not exercised", "VS Code search-input/quick-pick interactive command flow not exercised"] };
  for (const name of ["app", "service", "mcp", "capture", "vsix"]) if (options[`--${name}`]) receipt.artifactHashes[name] = await digest(options[`--${name}`]);
  receipt.sourceBase = (await execute("git", ["rev-parse", "HEAD"], { cwd: path.resolve(__dirname, "../../../..") })).stdout.trim();
  const fixture = await createFixture({ app: options["--app"], service: options["--service"], mcp: options["--mcp"], capture: options["--capture"] });
  const failed = [];
  try {
    for (const [name, operation] of [["claude", () => claudeProbe(fixture)], ["codex", () => codexProbe(fixture)], ["vscode", () => hostProbe(fixture, options["--vsix"])], ["mcp", () => mcpProbe(fixture)], ["capture", () => captureProbe(fixture)]]) {
      if (!clients.includes(name)) continue;
      try { receipt.checks[name] = { status: "passed", ...await operation() }; }
      catch (error) { failed.push(name); receipt.checks[name] = { status: "failed", diagnostic: error.message.slice(0, 256) }; }
    }
  } finally {
    for (const [name, hash] of baseline) {
      let after = null;
      try { after = await digest(path.join(os.homedir(), name)); } catch (error) { if (error.code !== "ENOENT") throw error; }
      if (after !== hash) failed.push("global_configuration_changed");
    }
    receipt.globalConfigurationsUnchanged = !failed.includes("global_configuration_changed");
    receipt.fixtureRetainedForDiagnostics = failed.length > 0;
    if (failed.length) { await fixture.stop(); console.error(`Synthetic diagnostics retained at ${fixture.root}`); }
    else await fixture.cleanup();
    await fs.writeFile(options["--output"], JSON.stringify(receipt, null, 2) + "\n", { mode: 0o600 });
  }
  console.log(JSON.stringify({ checks: Object.fromEntries(Object.entries(receipt.checks).map(([name, check]) => [name, check.status])), globalConfigurationsUnchanged: receipt.globalConfigurationsUnchanged }));
  if (failed.length) process.exitCode = 1;
}
main().catch((error) => { console.error(error.message); process.exitCode = 1; });
