"use strict";
const assert = require("node:assert/strict");
const { startRpc, cleanEnvironment } = require("./stdio");

async function mcpProbe(fixture) {
  const rpc = startRpc(fixture.launcher, ["--mdreader-mcp", "--data-dir", fixture.data, "--source", fixture.selected], { cwd: fixture.workspace, env: cleanEnvironment() });
  try {
    const initialized = await rpc.request("initialize", { protocolVersion: "2025-11-25", capabilities: {}, clientInfo: { name: "synthetic_acceptance", version: "1.0.0" } });
    assert.equal(initialized.protocolVersion, "2025-11-25");
    rpc.notify("notifications/initialized");
    const names = ["list_sources", "index_status", "search", "read_document", "related", "resolve_reference"];
    const catalog = await rpc.request("tools/list");
    assert.deepEqual(catalog.tools.map((tool) => tool.name).sort(), [...names].sort());
    assert.ok(catalog.tools.every((tool) => tool.annotations.readOnlyHint === true));
    async function call(name, args) {
      const result = await rpc.request("tools/call", { name, arguments: args });
      assert.equal(result.isError, false);
      assert.ok(result.content[0].text.startsWith("<!-- mdreader-retrieval -->"));
      return result.structuredContent;
    }
    const sources = (await call("list_sources", {})).data;
    assert.deepEqual(sources.map((source) => source.id), [fixture.selected]);
    assert.ok(!JSON.stringify(sources).includes(fixture.workspace));
    assert.equal((await call("index_status", {})).data.documents, 2);
    const hits = (await call("search", { query: "NebulaAnchor", intent: "exact" })).data;
    assert.equal(hits.length, 1);
    assert.equal(hits[0].revision, fixture.reference.revision);
    assert.ok(hits[0].line_start > 0 && hits[0].line_end >= hits[0].line_start);
    const document = (await call("read_document", { source_id: fixture.selected, path: fixture.reference.path, revision: fixture.reference.revision })).data;
    assert.ok(document.markdown.includes("NebulaAnchor exact visible body."));
    assert.equal(document.line_start, 1);
    assert.equal((await call("related", { source_id: fixture.selected, path: fixture.reference.path })).data[0].path, "notes/next.md");
    assert.equal((await call("resolve_reference", { source_id: fixture.selected, from: fixture.reference.path, target: "next.md#destination" })).data[0].line, 1);
    const semantic = await call("search", { query: "NebulaAnchor", intent: "semantic" });
    assert.deepEqual(semantic.data, []);
    assert.ok(semantic._meta["mdreader/semanticUnavailable"]);
    for (const args of [{ source_id: fixture.denied, path: "hidden.md" }, { source_id: fixture.selected, path: fixture.reference.path, revision: "0".repeat(64) }]) {
      assert.equal((await rpc.request("tools/call", { name: "read_document", arguments: args })).isError, true);
    }
    return { protocolVersion: initialized.protocolVersion, sixReadOnlyToolsInvoked: true, revisionLineEvidence: true, unselectedAndStaleRejected: true, semanticUnavailableWithoutLexicalSubstitutes: true, retrievalMarkerPreserved: true, client: "direct_synthetic_protocol_client_not_agent_model" };
  } finally { await rpc.close(); }
}
module.exports = { mcpProbe };
