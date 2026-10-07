"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const net = require("node:net");
const {
  ReaderClient,
  ReaderFailure,
  ensureLocalHost,
  validateDocumentPath,
  selectionUri,
  parseSelectionUri,
} = require("../src/reader-client");

const revision = "a".repeat(64);
async function fixture(t, responder) {
  const directory = await fs.mkdtemp(
    path.join(os.tmpdir(), "mdreader-vscode-"),
  );
  const sockets = new Set();
  const server = net.createServer({ allowHalfOpen: true }, (socket) => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
    let input = "";
    socket.on("data", (chunk) => {
      input += chunk;
      if (input.includes("\n"))
        responder(socket, JSON.parse(input.split("\n")[0]));
    });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  await fs.writeFile(
    path.join(directory, "reader-endpoint.json"),
    JSON.stringify({
      protocol: 1,
      port: server.address().port,
      token: "synthetic-token",
      pid: 123,
    }),
    { mode: 0o600 },
  );
  t.after(async () => {
    for (const socket of sockets) socket.destroy();
    await new Promise((resolve) => server.close(resolve));
    await fs.rm(directory, { recursive: true, force: true });
  });
  return {
    directory,
    client: new ReaderClient({
      dataDirectory: directory,
      sourceIds: ["selected"],
      timeoutMs: 100,
    }),
  };
}
function reply(socket, result) {
  socket.end(JSON.stringify({ result, error: null }) + "\n");
}

test("only the local desktop host and explicit selected profile are supported", () => {
  ensureLocalHost({ uiKind: 1, remoteName: undefined });
  for (const host of [{ uiKind: 2 }, { uiKind: 1, remoteName: "ssh-remote" }])
    assert.throws(() => ensureLocalHost(host), ReaderFailure);
  for (const config of [
    { dataDirectory: "/tmp", sourceIds: [] },
    { dataDirectory: "relative", sourceIds: ["selected"] },
    { dataDirectory: "/tmp", sourceIds: ["bad/source"] },
  ])
    assert.throws(() => new ReaderClient(config), ReaderFailure);
});
test("virtual URI paths and line evidence cannot escape their source", () => {
  for (const target of [
    "../secret.md",
    "/absolute.md",
    "a/../../secret.md",
    "C:\\secret.md",
    "a\\b.md",
    "a\0b.md",
  ])
    assert.throws(() => validateDocumentPath(target), ReaderFailure);
  const selection = {
    source_id: "selected",
    path: "notes/space #é.md",
    revision,
    line: 4,
  };
  assert.deepEqual(parseSelectionUri(selectionUri(selection)), selection);
  assert.deepEqual(parseSelectionUri(`mdreader://selected/notes/space%20%23%C3%A9.md?revision=${revision}#line=4`), selection);
  for (const uri of [
    "file:///etc/passwd",
    "mdreader://selected/../secret.md",
    "mdreader://selected/%2e%2e/secret.md",
    "mdreader://selected/%2Fprivate.md",
    "mdreader://selected/ok.md?revision=bad#line=0",
  ])
    assert.throws(() => parseSelectionUri(uri), ReaderFailure);
});
test("provider source identities survive virtual URI roundtrip without granting another binding", () => {
  const source = `provider:${"a".repeat(64)}`;
  const selected = { source_id: source, path: "selected/space #é.md", revision, line: 3 };
  assert.deepEqual(parseSelectionUri(selectionUri(selected)), selected);
  assert.equal(new URL(selectionUri(selected)).host, "document");
  const declaredPattern = require("../package.json").contributes.configuration.properties["mdreader.sourceIds"].items.pattern;
  assert.ok(new RegExp(declaredPattern).test(source));
  const client = new ReaderClient({ dataDirectory: "/tmp", sourceIds: [source] });
  assert.equal(client.selected(selected).source_id, source);
  assert.throws(() => client.selected({ ...selected, source_id: `provider:${"b".repeat(64)}` }), ReaderFailure);
  for (const forged of ["provider:abc", `other:${"a".repeat(64)}`, "provider:../escape", "selected:123"]) {
    assert.throws(() => new ReaderClient({ dataDirectory: "/tmp", sourceIds: [forged] }), ReaderFailure);
  }
});
test("loopback client sends authenticated read-only requests and filters sources", async (t) => {
  const { client } = await fixture(t, (socket, envelope) => {
    assert.equal(envelope.protocol, 1);
    assert.equal(envelope.token, "synthetic-token");
    if (envelope.request.method === "list_sources")
      reply(socket, [
        { id: "hidden", name: "PRIVATE_NAME", enabled: true },
        { id: "selected", name: "Notes", enabled: true, path: "PRIVATE_ROOT" },
      ]);
    else {
      assert.deepEqual(envelope.request, {
        method: "search",
        params: { query: "Plan", sources: ["selected"], limit: 20 },
      });
      reply(socket, [
        {
          source_id: "selected",
          path: "plan.md",
          revision,
          title: "Plan",
          heading: "Plan",
          line_start: 2,
          line_end: 4,
          text: "Evidence",
          score: 1,
          lexical_rank: 1,
        },
      ]);
    }
  });
  assert.deepEqual(await client.listSources(), [
    { id: "selected", name: "Notes", enabled: true },
  ]);
  assert.equal((await client.search("Plan"))[0].line_start, 2);
  await assert.rejects(
    () => client.read({ source_id: "hidden", path: "secret.md" }),
    ReaderFailure,
  );
  await assert.rejects(() => client.search("Plan", []), ReaderFailure);
});
test("wrong-source and unsafe output paths fail closed", async (t) => {
  let count = 0;
  const { client } = await fixture(t, (socket) =>
    reply(
      socket,
      count++ === 0
        ? [{ source_id: "hidden", path: "secret.md", revision, line: 1 }]
        : {
            source_id: "selected",
            path: "../secret.md",
            revision,
            markdown: "PRIVATE_CONTENT",
          },
    ),
  );
  await assert.rejects(
    () => client.related({ source_id: "selected", path: "plan.md" }),
    ReaderFailure,
  );
  await assert.rejects(
    () => client.read({ source_id: "selected", path: "plan.md" }),
    ReaderFailure,
  );
});
test("stale revisions and service errors are useful without exposing server messages", async (t) => {
  const { client } = await fixture(t, (socket) =>
    socket.end(
      JSON.stringify({
        result: null,
        error: {
          code: "stale_revision",
          message: "PRIVATE_TOKEN PRIVATE_ROOT",
        },
      }) + "\n",
    ),
  );
  await assert.rejects(
    () => client.read({ source_id: "selected", path: "plan.md", revision }),
    (error) =>
      error.code === "stale_revision" && !error.message.includes("PRIVATE_"),
  );
});
test("malformed, oversized, incomplete and stalled transport frames reject safely", async (t) => {
  let count = 0;
  const { client } = await fixture(t, (socket) => {
    if (count === 0) socket.end("broken\n");
    if (count === 1) socket.end('{"result":');
    if (count === 2) socket.write(Buffer.alloc(16 * 1024 * 1024 + 1, 120));
    count++;
  });
  for (let index = 0; index < 4; index++)
    await assert.rejects(() => client.listSources(), ReaderFailure);
});
test("endpoint restarts are discovered and cancellation closes the socket", async (t) => {
  const { client, directory } = await fixture(t, (socket) => reply(socket, []));
  assert.deepEqual(await client.listSources(), []);
  await fs.writeFile(
    path.join(directory, "reader-endpoint.json"),
    "PRIVATE_TOKEN bad endpoint",
  );
  await assert.rejects(
    () => client.listSources(),
    (error) =>
      error instanceof ReaderFailure && !error.message.includes("PRIVATE_"),
  );
  const waiting = await fixture(t, () => {});
  const controller = new AbortController();
  const operation = waiting.client.listSources(controller.signal);
  controller.abort();
  await assert.rejects(
    () => operation,
    (error) => error.code === "cancelled",
  );
});
