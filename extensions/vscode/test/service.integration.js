"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const os = require("node:os");
const net = require("node:net");
const { spawn } = require("node:child_process");
const { once } = require("node:events");
const {
  ReaderClient,
  selectionUri,
  parseSelectionUri,
} = require("../src/reader-client");
async function setup(directory, request) {
  const endpoint = JSON.parse(
    await fs.readFile(path.join(directory, "reader-endpoint.json"), "utf8"),
  );
  return await new Promise((resolve, reject) => {
    const socket = net.createConnection({
      host: "127.0.0.1",
      port: endpoint.port,
    });
    let bytes = "";
    socket.on("connect", () =>
      socket.end(
        JSON.stringify({ protocol: 1, token: endpoint.token, request }) + "\n",
      ),
    );
    socket.on("data", (chunk) => {
      bytes += chunk;
      if (bytes.includes("\n")) {
        socket.destroy();
        const reply = JSON.parse(bytes.split("\n")[0]);
        if (reply.error) reject(new Error("synthetic fixture failed"));
        else resolve(reply.result);
      }
    });
    socket.on("error", reject);
  });
}
test("real headless service supports extension source/open/search/link/restart flows without a reader window", async (t) => {
  assert.ok(
    process.env.MDREADER_TEST_SERVICE_BIN,
    "Set MDREADER_TEST_SERVICE_BIN to a freshly built service executable.",
  );
  assert.notEqual(
    process.platform,
    "win32",
    "This disposable admission fixture currently supports Unix; Windows device acceptance is separate.",
  );
  const directory = await fs.mkdtemp(
    path.join(os.tmpdir(), "mdreader-extension-real-"),
  );
  const data = path.join(directory, "data");
  const notes = path.join(directory, "notes");
  const hidden = path.join(directory, "hidden");
  await fs.mkdir(notes);
  await fs.mkdir(hidden);
  await fs.writeFile(
    path.join(notes, "plan.md"),
    "# Plan\nDurable extension evidence.\n\n[Next](next.md#next)\n",
  );
  await fs.writeFile(path.join(notes, "next.md"), "# Next\nNext document.\n");
  await fs.writeFile(
    path.join(hidden, "secret.md"),
    "# Secret\nDurable PRIVATE_CONTENT.\n",
  );
  let processHandle;
  async function start() {
    processHandle = spawn(process.env.MDREADER_TEST_SERVICE_BIN, [data], {
      stdio: "ignore",
    });
    for (let attempt = 0; attempt < 200; attempt++) {
      try {
        await setup(data, { method: "ping" });
        return;
      } catch {
        await new Promise((resolve) => setTimeout(resolve, 10));
      }
    }
    throw new Error("synthetic service did not start");
  }
  async function stop() {
    if (processHandle) {
      const ended = once(processHandle, "exit");
      processHandle.kill();
      await ended;
      processHandle = undefined;
    }
  }
  t.after(async () => {
    await stop();
    await fs.rm(directory, { recursive: true, force: true });
  });
  await start();
  async function admit(root, name) {
    const canonical = await fs.realpath(root);
    const stat = await fs.stat(canonical);
    const source = await setup(data, {
      method: "admit",
      params: {
        name,
        path: canonical,
        identity: { platform: "unix", device: stat.dev, inode: stat.ino },
      },
    });
    await setup(data, { method: "index", params: { source_id: source.id } });
    return source.id;
  }
  const selected = await admit(notes, "Selected");
  const denied = await admit(hidden, "PRIVATE_NAME");
  const client = new ReaderClient({
    dataDirectory: data,
    sourceIds: [selected],
    timeoutMs: 2000,
  });
  assert.equal((await client.listSources()).length, 1);
  assert.equal((await client.status()).documents, 2);
  const results = await client.search("Durable");
  assert.equal(results.length, 1);
  assert.equal(results[0].path, "plan.md");
  const selection = parseSelectionUri(selectionUri(results[0]));
  const document = await client.read(selection);
  assert.ok(document.markdown.includes("extension evidence"));
  const references = await client.resolve(document, "next.md#next");
  assert.equal(references[0].path, "next.md");
  assert.equal(references[0].line, 1);
  assert.equal((await client.related(references[0]))[0].path, "plan.md");
  await assert.rejects(() =>
    client.read({ source_id: denied, path: "secret.md" }),
  );
  await fs.writeFile(
    path.join(notes, "plan.md"),
    "# Plan\nChanged extension revision.\n",
  );
  await assert.rejects(
    () => client.read(selection),
    (error) => error.code === "stale_revision",
  );
  await stop();
  await assert.rejects(() => client.listSources());
  await start();
  assert.equal((await client.listSources())[0].id, selected);
});
