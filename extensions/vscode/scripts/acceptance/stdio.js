"use strict";
const { spawn } = require("node:child_process");
const { once } = require("node:events");
const { waitForExit } = require("./service-custody");

function cleanEnvironment() {
  return Object.fromEntries(Object.entries(process.env).filter(([name]) => !/(?:TOKEN|SECRET|PASSWORD|KEY|AUTH)/iu.test(name)));
}
function startRpc(command, args, options, trackProcess = () => {}) {
  const child = spawn(command, args, { ...options, stdio: ["pipe", "pipe", "ignore"] });
  trackProcess(child);
  const ended = once(child, "exit");
  let nextId = 1, bytes = "", broken = false;
  const waiting = new Map();
  function fail(error) {
    for (const entry of waiting.values()) { clearTimeout(entry.timer); entry.reject(error); }
    waiting.clear();
  }
  child.on("error", () => fail(new Error("client process could not start")));
  child.on("exit", () => fail(new Error("client process exited")));
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    if (broken) return;
    bytes += chunk;
    if (Buffer.byteLength(bytes) > 4 * 1024 * 1024) { broken = true; bytes = ""; fail(new Error("client frame bound")); child.stdin.end(); return; }
    while (bytes.includes("\n")) {
      const offset = bytes.indexOf("\n");
      const line = bytes.slice(0, offset); bytes = bytes.slice(offset + 1);
      let message;
      try { message = JSON.parse(line); } catch { broken = true; bytes = ""; fail(new Error("client protocol malformed")); child.stdin.end(); return; }
      const entry = waiting.get(message.id);
      if (!entry) continue;
      waiting.delete(message.id); clearTimeout(entry.timer);
      if (message.error) entry.reject(new Error(`client RPC rejected: ${message.error.code}`));
      else entry.resolve(message.result);
    }
  });
  return {
    request(method, params = {}) {
      const id = nextId++;
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => { waiting.delete(id); reject(new Error(`client RPC timeout: ${method}`)); }, 15000);
        waiting.set(id, { resolve, reject, timer });
        child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
      });
    },
    notify(method) { child.stdin.write(JSON.stringify({ jsonrpc: "2.0", method }) + "\n"); },
    async close() { child.stdin.end(); await waitForExit(ended); },
  };
}
module.exports = { startRpc, cleanEnvironment };
