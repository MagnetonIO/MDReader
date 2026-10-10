"use strict";
const fs = require("node:fs/promises");
const { constants } = require("node:fs");
const path = require("node:path");
const { randomUUID } = require("node:crypto");

function sameIdentity(a, b) { return a.dev === b.dev && a.ino === b.ino; }
function sameFile(a, b) {
  return sameIdentity(a, b) && a.size === b.size && a.mtimeMs === b.mtimeMs && a.ctimeMs === b.ctimeMs;
}
async function endpointFile(filename) {
  const file = await fs.open(filename, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const before = await file.stat();
    if (!before.isFile() || before.nlink !== 1 || before.size > 4096 || (before.mode & 0o077)) throw new Error("fixture endpoint ownership unavailable");
    const bytes = Buffer.alloc(4097);
    const { bytesRead } = await file.read(bytes, 0, bytes.length, 0);
    const after = await file.stat();
    if (!sameFile(before, after) || bytesRead !== before.size) throw new Error("fixture endpoint changed");
    return { stat: before, bytes: bytes.subarray(0, bytesRead) };
  } finally { await file.close(); }
}
async function retainDirectory(directory) {
  const canonical = await fs.realpath(directory);
  const retained = await fs.lstat(directory);
  if (canonical !== directory || !retained.isDirectory() || (retained.mode & 0o077)) throw new Error("fixture directory ownership unavailable");
  return async () => {
    const current = await fs.lstat(directory);
    if (!current.isDirectory() || !sameIdentity(retained, current) || await fs.realpath(directory) !== directory) throw new Error("fixture directory replaced; contents retained");
  };
}
async function bindService(directory, child, request) {
  if (process.platform === "win32") throw new Error("fixture preservation requires Unix identity");
  const root = await fs.realpath(directory);
  if (root !== directory) throw new Error("fixture root changed");
  const identity = await fs.lstat(root);
  if (!identity.isDirectory() || (identity.mode & 0o077)) throw new Error("fixture root ownership unavailable");
  const filename = path.join(root, "reader-endpoint.json");
  const retained = await endpointFile(filename);
  const endpoint = JSON.parse(retained.bytes);
  if (endpoint.protocol !== 1 || endpoint.pid !== child.pid || !Number.isInteger(endpoint.port)
      || endpoint.port < 1 || endpoint.port > 65535 || typeof endpoint.token !== "string" || !endpoint.token) {
    throw new Error("fixture endpoint does not belong to spawned service");
  }
  async function verify() {
    if (child.exitCode !== null || child.signalCode !== null) throw new Error("fixture service already exited");
    const currentRoot = await fs.lstat(root);
    if (!currentRoot.isDirectory() || !sameIdentity(identity, currentRoot)) throw new Error("fixture root changed");
    const current = await endpointFile(filename);
    if (!sameFile(retained.stat, current.stat) || !retained.bytes.equals(current.bytes)) throw new Error("fixture endpoint replaced");
  }
  async function control(value, final = false) {
    await verify();
    const reply = await request(directory, { method: "preservation", params: { request: value } }, { endpoint, timeoutMs: 32000 });
    if (!final) await verify();
    return reply;
  }
  return {
    async shutdown() {
      const selected = { directory: root, identity: { platform: "unix", device: identity.dev, inode: identity.ino } };
      const hello = await control({ operation: "inspect", root: selected });
      if (hello.state !== "hello") throw new Error("fixture preservation unavailable");
      const scope = { operation: randomUUID(), owner_epoch: randomUUID(), boot: hello.boot, root: selected };
      let reply = await control({ operation: "begin", scope, duration_ms: 30000 });
      const deadline = Date.now() + 25000;
      while (reply.state === "status" && reply.status.phase === "acquiring" && Date.now() < deadline) {
        await new Promise((resolve) => setTimeout(resolve, 100));
        reply = await control({ operation: "reconcile", scope });
      }
      if (reply.state !== "status" || reply.status.phase !== "held" || !reply.status.lease) throw new Error("fixture service did not drain; retained without termination");
      const lease = reply.status.lease;
      const checked = await control({ operation: "revalidate", scope, lease });
      if (checked.state !== "status" || checked.status.phase !== "held" || checked.status.lease !== lease) throw new Error("fixture drain changed");
      const ack = await control({ operation: "shutdown", scope, lease }, true);
      if (ack.state !== "shutdown_requested" || ack.boot !== scope.boot) throw new Error("fixture shutdown unconfirmed");
    },
  };
}
async function waitForExit(ended, timeoutMs = 10000) {
  let timer;
  try {
    return await Promise.race([ended, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error("fixture process still running; retained without termination")), timeoutMs);
    })]);
  } finally { clearTimeout(timer); }
}
module.exports = { bindService, waitForExit, retainDirectory };
