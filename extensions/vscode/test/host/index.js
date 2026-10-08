"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const vscode = require("vscode");
const { selectionUri, parseSelectionUri, ReaderClient } = require("../../src/reader-client");

async function run() {
  const fixture = JSON.parse(await fs.readFile(process.env.MDREADER_ACCEPTANCE_FIXTURE, "utf8"));
  const evidence = { vscodeVersion: vscode.version, uiKind: vscode.env.uiKind, remoteName: vscode.env.remoteName ?? null, checks: [] };
  const extension = vscode.extensions.getExtension("mdreader.mdreader");
  assert.ok(extension, "the packaged MDReader extension must be installed");
  const installedPath = path.relative(path.join(fixture.root, "extensions"), extension.extensionPath);
  assert.ok(installedPath && !installedPath.startsWith("..") && !path.isAbsolute(installedPath), "test driver must activate the installed VSIX, not a development copy");
  await extension.activate();
  assert.ok(extension.isActive);
  evidence.extensionVersion = extension.packageJSON.version;
  evidence.loadOrigin = "isolated_installed_vsix";
  evidence.checks.push("installed_vsix_activated_in_real_local_extension_host");
  const commands = await vscode.commands.getCommands(true);
  for (const name of ["search", "open", "preview", "backlinks", "status", "openReference"]) assert.ok(commands.includes(`mdreader.${name}`));

  const uri = vscode.Uri.parse(selectionUri(fixture.reference));
  const document = await vscode.workspace.openTextDocument(uri);
  assert.equal(document.getText(), "# Acceptance Plan\n\nNebulaAnchor exact visible body.\n\n[Next](next.md#destination)\n");
  assert.ok(document.isUntitled === false && document.uri.scheme === "mdreader");
  await vscode.window.showTextDocument(document);
  evidence.checks.push("real_content_provider_returned_exact_revision_bound_service_body");
  evidence.reference = { sourceId: fixture.selected, path: fixture.reference.path, revision: fixture.reference.revision, line: fixture.reference.line };
  await vscode.commands.executeCommand("mdreader.preview");
  let previewVisible = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    if (vscode.window.tabGroups.all.some((group) => group.tabs.some((tab) => tab.input instanceof vscode.TabInputWebview))) { previewVisible = true; break; }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  assert.ok(previewVisible, "the created preview must appear in the host tab model");
  evidence.checks.push("actual_preview_command_created_webview");
  await vscode.commands.executeCommand("mdreader.openReference", { ...fixture.reference, target: "next.md#destination" });
  const linked = vscode.workspace.textDocuments.find((item) => item.uri.scheme === "mdreader" && parseSelectionUri(item.uri.toString()).path === "notes/next.md");
  assert.ok(linked);
  assert.equal(linked.getText(), "# Destination\n\nSelected linked target.\n");
  evidence.checks.push("actual_reference_command_opened_selected_target_at_parser_line");

  const denied = vscode.Uri.parse(selectionUri({ source_id: fixture.denied, path: "hidden.md", revision: fixture.reference.revision, line: 1 }));
  await assert.rejects(() => vscode.workspace.openTextDocument(denied));
  evidence.checks.push("content_provider_rejected_unselected_source");
  const stale = vscode.Uri.parse(selectionUri({ ...fixture.reference, revision: "0".repeat(64) }));
  await assert.rejects(() => vscode.workspace.openTextDocument(stale));
  evidence.checks.push("content_provider_rejected_stale_revision");

  // Search/status/related use real transport here; the UI search-input and
  // quick-pick command flow remains a separate interactive acceptance gate.
  const client = new ReaderClient({ dataDirectory: fixture.data, sourceIds: [fixture.selected] });
  const hits = await client.search("NebulaAnchor");
  assert.equal(hits.length, 1);
  assert.equal(hits[0].source_id, fixture.selected);
  assert.equal(hits[0].revision, fixture.reference.revision);
  assert.deepEqual(await client.search("ForbiddenSentinel"), []);
  assert.equal((await client.status()).documents, 2);
  assert.equal((await client.related(fixture.reference))[0].path, "notes/next.md");
  evidence.checks.push("extension_host_service_transport_search_status_related_scoped");
  await fs.writeFile(process.env.MDREADER_ACCEPTANCE_HOST_RECEIPT, JSON.stringify(evidence, null, 2), { mode: 0o600 });
}
module.exports = { run };
