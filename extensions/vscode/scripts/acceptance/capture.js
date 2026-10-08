"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
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
    await submit(fixture, policyPath, { ...event, session_id: "outside-selected-project", cwd: path.join(fixture.root, "denied"), last_assistant_message: "OutsideCaptureSentinel" });
    assert.deepEqual(await client.search("OutsideCaptureSentinel"), []);
    return { executableMode: fixture.executableMode, syntheticHookEntrypoint: true, explicitProjectOptIn: true,
      realOwnerProjectionAndIndex: true, exactRevisionRead: true, excludedFieldsAbsent: true,
      outsideSelectedProjectSkipped: true, modelDrivenAgentHook: "not_exercised" };
  } finally {
    await fixture.request({ method: "configure_capture", params: { policy: fixture.policy } });
  }
}
module.exports = { captureProbe };
