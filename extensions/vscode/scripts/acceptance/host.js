"use strict";
const fs = require("node:fs/promises");
const path = require("node:path");
const assert = require("node:assert/strict");
const { spawn } = require("node:child_process");
const { execute } = require("./clients");
const { cleanEnvironment } = require("./stdio");

async function hostProbe(fixture, vsix) {
  const userData = path.join(fixture.root, "vscode-user");
  const extensions = path.join(fixture.root, "extensions");
  const driver = path.join(fixture.root, "test-driver");
  await fs.mkdir(path.join(userData, "User"), { recursive: true });
  await fs.mkdir(driver);
  await fs.mkdir(path.join(fixture.workspace, ".vscode"));
  await fs.writeFile(path.join(driver, "package.json"), JSON.stringify({ name: "synthetic-acceptance-driver", publisher: "mdreader-acceptance", version: "1.0.0", engines: { vscode: "^1.85.0" } }));
  await fs.writeFile(path.join(userData, "User/settings.json"), JSON.stringify({ "mdreader.dataDirectory": fixture.data, "mdreader.sourceIds": [fixture.selected], "telemetry.telemetryLevel": "off", "extensions.autoUpdate": false, "extensions.autoCheckUpdates": false, "update.mode": "none", "workbench.startupEditor": "none", "chat.disableAIFeatures": true, "git.enabled": false, "github.gitAuthentication": false, "window.newWindowProfile": "Default" }));
  // A workspace cannot replace machine/user grants even in this trusted fixture.
  await fs.writeFile(path.join(fixture.workspace, ".vscode/settings.json"), JSON.stringify({ "mdreader.dataDirectory": "/WORKSPACE_OVERRIDE_MUST_NOT_WIN", "mdreader.sourceIds": [fixture.denied] }));
  const env = cleanEnvironment();
  for (const key of Object.keys(env)) if (key.startsWith("VSCODE_") || key === "ELECTRON_RUN_AS_NODE") delete env[key];
  const common = ["--user-data-dir", userData, "--extensions-dir", extensions];
  await execute("code", [...common, "--install-extension", vsix, "--force"], { env, timeout: 30000, maxBuffer: 65536 });
  const installed = (await execute("code", [...common, "--list-extensions", "--show-versions"], { env, timeout: 15000 })).stdout;
  assert.match(installed, /^mdreader\.mdreader@1\.0\.0$/mu);
  const receipt = path.join(fixture.root, "host-receipt.json");
  const electron = "/Applications/Visual Studio Code.app/Contents/MacOS/Code";
  const log = await fs.open(path.join(fixture.root, "extension-host.log"), "w", 0o600);
  const child = spawn(electron, [...common, "--new-window", "--skip-welcome", "--skip-release-notes", "--disable-workspace-trust", "--disable-gpu", `--extensionDevelopmentPath=${driver}`, `--extensionTestsPath=${path.resolve(__dirname, "../../test/host/index.js")}`, fixture.workspace], { env: { ...env, MDREADER_ACCEPTANCE_FIXTURE: path.join(fixture.root, "fixture.json"), MDREADER_ACCEPTANCE_HOST_RECEIPT: receipt }, stdio: ["ignore", log.fd, log.fd], detached: true });
  fixture.trackProcess(child);
  try {
    await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        reject(new Error("installed Extension Host probe timed out; isolated profile retained until normal exit"));
      }, 45000);
      child.once("error", () => { clearTimeout(timeout); reject(new Error("installed VS Code could not start")); });
      child.once("exit", (code) => { clearTimeout(timeout); if (code === 0) resolve(); else reject(new Error(`installed Extension Host probe exited ${code}`)); });
    });
  } finally { await log.close(); }
  return { ...JSON.parse(await fs.readFile(receipt, "utf8")), workspaceTrust: "disabled_for_explicit_synthetic_test_workspace", globalExtensionInstall: false };
}
module.exports = { hostProbe };
