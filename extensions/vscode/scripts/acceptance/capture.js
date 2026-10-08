"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { createHash } = require("node:crypto");
const { spawn } = require("node:child_process");
const { ReaderClient } = require("../../src/reader-client");
const { cleanEnvironment } = require("./stdio");
const { delay } = require("./fixture");

function submit(fixture, policy, event) {
  return new Promise((resolve, reject) => {
    const child = spawn(fixture.launcher, ["--mdreader-capture", "hook", "claude", policy, path.join(fixture.data, "captures")], {
      cwd: fixture.workspace, env: cleanEnvironment(), stdio: ["pipe", "pipe", "ignore"],
    });
    let output = "";
    let failure;
    const timer = setTimeout(() => { failure = new Error("capture entrypoint timed out"); child.kill("SIGKILL"); }, 10000);
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (bytes) => {
      output += bytes;
      if (Buffer.byteLength(output) > 65536) { failure = new Error("capture reply bound"); child.kill("SIGKILL"); }
    });
    child.stdin.on("error", () => { failure ??= new Error("capture input unavailable"); });
    child.once("error", () => { clearTimeout(timer); reject(new Error("capture entrypoint unavailable")); });
    child.once("close", (code) => {
      clearTimeout(timer);
      if (failure || code !== 0) reject(failure ?? new Error("capture entrypoint rejected synthetic event"));
      else {
        try { assert.deepEqual(JSON.parse(output), {}); resolve(); }
        catch { reject(new Error("capture reply malformed")); }
      }
    });
    child.stdin.end(JSON.stringify(event));
  });
}

// Observe the settled, synthetic Unix spool directly: an erroneously admitted
// event remains visible either as pending work or as its immutable receipt,
// even when projection runs before the following search RPC.
async function spoolSnapshot(fixture) {
  const snapshot = {};
  for (const directory of ["pending", "receipts", "markdown"]) {
    const root = path.join(fixture.data, "captures", directory);
    const names = (await fs.readdir(root)).sort();
    assert.ok(names.length <= 16, "synthetic spool file bound");
    snapshot[directory] = {};
    for (const name of names) {
      const pattern = directory === "pending" ? /^[a-f0-9]{64}\.json$/u
        : directory === "markdown" ? /^[a-f0-9]{64}\.md$/u : /^[a-f0-9]{64}$/u;
      assert.match(name, pattern);
      const file = path.join(root, name);
      const stat = await fs.lstat(file);
      assert.ok(stat.isFile() && stat.size <= 1048576, "synthetic spool content bound");
      const bytes = await fs.readFile(file);
      assert.equal(bytes.length, stat.size);
      snapshot[directory][name] = createHash("sha256").update(bytes).digest("hex");
    }
  }
  return snapshot;
}
async function settledSpool(fixture) {
  for (let attempt = 0; attempt < 100; attempt++) {
    const snapshot = await spoolSnapshot(fixture);
    if (Object.keys(snapshot.pending).length === 0
      && Object.keys(snapshot.receipts).length === 1
      && Object.keys(snapshot.markdown).length === 1) return snapshot;
    await delay(50);
  }
  throw new Error("legitimate synthetic capture did not settle durably");
}

async function captureProbe(fixture) {
  const policy = { ...fixture.policy, enabled: true };
  await fixture.request({ method: "configure_capture", params: { policy } });
  try {
    const directory = path.join(fixture.data, "captures", "policies");
    const files = await fs.readdir(directory);
    assert.equal(files.length, 1);
    assert.match(files[0], /^[a-f0-9-]{36}\.json$/u);
    const policyPath = path.join(directory, files[0]);
    const event = {
      hook_event_name: "Stop", session_id: "synthetic-packaged-session", prompt_id: "synthetic-turn",
      cwd: fixture.workspace, last_assistant_message: "# Synthetic visible output\n\nCapturedStarfieldVisible exact body.\n",
      prompt: "ExcludedPromptSentinel", reasoning: "ExcludedReasoningSentinel", tool_output: "ExcludedToolSentinel",
    };
    await submit(fixture, policyPath, event);
    const sources = await fixture.request({ method: "capture_reader_sources" });
    assert.equal(sources.length, 1);
    const client = new ReaderClient({ dataDirectory: fixture.data, sourceIds: [sources[0].id] });
    let hit;
    for (let attempt = 0; attempt < 100; attempt++) {
      [hit] = await client.search("CapturedStarfieldVisible");
      if (hit) break;
      await delay(50);
    }
    assert.ok(hit, "captured output must reach the real owner index");
    const document = await client.read({ ...hit, line: hit.line_start });
    assert.ok(document.markdown.includes(event.last_assistant_message));
    assert.doesNotMatch(document.markdown, /Excluded(?:Prompt|Reasoning|Tool)Sentinel/u);
    assert.equal(document.revision, hit.revision);
    const beforeExcluded = await settledSpool(fixture);
    await submit(fixture, policyPath, { ...event, session_id: "outside-selected-project", cwd: path.join(fixture.root, "denied"), last_assistant_message: "OutsideCaptureSentinel" });
    assert.deepEqual(await spoolSnapshot(fixture), beforeExcluded, "excluded event must not enter durable spool or projection");
    assert.deepEqual(await client.search("OutsideCaptureSentinel"), []);
    return { executableMode: fixture.executableMode, syntheticHookEntrypoint: true, explicitProjectOptIn: true,
      realOwnerProjectionAndIndex: true, exactRevisionRead: true, excludedFieldsAbsent: true,
      outsideSelectedProjectSkipped: true, outsideSelectedProjectDurableSpoolUnchanged: true, modelDrivenAgentHook: "not_exercised" };
  } finally {
    await fixture.request({ method: "configure_capture", params: { policy: fixture.policy } });
  }
}
module.exports = { captureProbe };
