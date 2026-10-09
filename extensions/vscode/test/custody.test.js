"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const os = require("node:os");
const { randomUUID } = require("node:crypto");
const { bindService, retainDirectory } = require("../scripts/acceptance/service-custody");
const { startRpc } = require("../scripts/acceptance/stdio");

async function selected(t) {
  const root = await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(), "mdreader-custody-test-")));
  await fs.chmod(root, 0o700);
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const child = { pid: 12345, exitCode: null, signalCode: null };
  const endpoint = path.join(root, "reader-endpoint.json");
  await fs.writeFile(endpoint, JSON.stringify({ protocol: 1, pid: child.pid, port: 1234, token: "synthetic-private-token" }), { mode: 0o600 });
  return { root, child, endpoint };
}
test("preservation shutdown requires retained identity, held lease and matching boot", { skip: process.platform === "win32" }, async (t) => {
  const { root, child } = await selected(t);
  const boot = randomUUID(), lease = randomUUID();
  let scope;
  const operations = [];
  const bound = await bindService(root, child, async (directory, envelope, options) => {
    assert.equal(directory, root);
    assert.equal(options.endpoint.pid, child.pid);
    const value = envelope.params.request;
    operations.push(value.operation);
    if (value.operation === "inspect") return { state: "hello", boot };
    scope ??= value.scope;
    assert.deepEqual(value.scope, scope);
    assert.equal(scope.boot, boot);
    if (value.operation === "begin") return { state: "status", status: { phase: "acquiring" } };
    if (value.operation === "reconcile") return { state: "status", status: { phase: "held", lease } };
    assert.equal(value.lease, lease);
    if (value.operation === "revalidate") return { state: "status", status: { phase: "held", lease } };
    return { state: "shutdown_requested", boot };
  });
  await bound.shutdown();
  assert.deepEqual(operations, ["inspect", "begin", "reconcile", "revalidate", "shutdown"]);
});
test("endpoint replacement and links cannot authorize shutdown of a different owner", { skip: process.platform === "win32" }, async (t) => {
  const { root, child, endpoint } = await selected(t);
  let calls = 0;
  const bound = await bindService(root, child, () => { calls += 1; });
  const original = path.join(root, "original-endpoint");
  await fs.rename(endpoint, original);
  await fs.copyFile(original, endpoint);
  await assert.rejects(() => bound.shutdown(), /endpoint replaced/);
  await fs.unlink(endpoint);
  await fs.symlink(original, endpoint);
  await assert.rejects(() => bound.shutdown());
  assert.equal(calls, 0);
});
test("failed drain retains the owner and never issues shutdown", { skip: process.platform === "win32" }, async (t) => {
  const { root, child } = await selected(t);
  const operations = [];
  const bound = await bindService(root, child, async (_root, envelope) => {
    const value = envelope.params.request;
    operations.push(value.operation);
    return value.operation === "inspect" ? { state: "hello", boot: randomUUID() } : { state: "failure", error: "busy" };
  });
  await assert.rejects(() => bound.shutdown(), /did not drain/);
  assert.deepEqual(operations, ["inspect", "begin"]);
});
test("stdio acceptance clients finish on EOF without a termination signal", async () => {
  let child;
  const rpc = startRpc(process.execPath, ["-e", "process.stdin.resume(); process.stdin.on('end', () => process.exit(0));"], {}, (value) => { child = value; });
  await rpc.close();
  assert.equal(child.exitCode, 0);
  assert.equal(child.signalCode, null);
});
test("outer fixture replacement cannot authorize recursive cleanup", async (t) => {
  const { root } = await selected(t);
  const verify = await retainDirectory(root);
  const original = `${root}-retained`;
  await fs.rename(root, original);
  t.after(() => fs.rm(original, { recursive: true, force: true }));
  await fs.mkdir(root, { mode: 0o700 });
  const sentinel = path.join(root, "replacement.md");
  await fs.writeFile(sentinel, "Replacement belongs to another owner");
  await assert.rejects(verify, /directory replaced/);
  assert.equal(await fs.readFile(sentinel, "utf8"), "Replacement belongs to another owner");
});
