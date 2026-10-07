"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const Module = require("node:module");
const transport = require("../src/reader-client");
function activateMock({ remoteName, statusPromise } = {}) {
  const commands = new Map();
  const messages = [];
  const contexts = [];
  let configurationChanged;
  const dispose = () => ({ dispose() {} });
  const vscode = {
    env: { uiKind: 1, remoteName },
    StatusBarAlignment: { Left: 1 },
    window: {
      createStatusBarItem: () => ({ show() {}, dispose() {} }),
      showErrorMessage: async (message) => messages.push({ error: message }),
      showInformationMessage: async (message) =>
        messages.push({ info: message }),
    },
    workspace: {
      getConfiguration: () => ({
        get: () => "/WORKSPACE_MUST_NOT_WIN",
        inspect: (key) => ({
          globalValue:
            key === "dataDirectory"
              ? "/synthetic-reader-profile"
              : ["selected"],
          workspaceValue:
            key === "dataDirectory" ? "/WORKSPACE_MUST_NOT_WIN" : ["hidden"],
        }),
      }),
      registerTextDocumentContentProvider: (scheme, provider) => {
        contexts.push({ scheme, provider });
        return dispose();
      },
      onDidChangeConfiguration: (handler) => {
        configurationChanged = handler;
        return dispose();
      },
    },
    commands: {
      registerCommand: (name, handler) => {
        commands.set(name, handler);
        return dispose();
      },
    },
  };
  const clients = [];
  class FakeClient {
    constructor(config) {
      clients.push(config);
    }
    async listSources() {
      return [{ id: "selected", name: "Notes", enabled: true }];
    }
    async status() {
      return statusPromise
        ? await statusPromise
        : { documents: 2, chunks: 3, embedded: 0, model: "unavailable" };
    }
  }
  const load = Module._load;
  Module._load = function (name, parent, ...rest) {
    if (name === "vscode") return vscode;
    if (name === "./reader-client" && parent.filename.endsWith("extension.js"))
      return { ...transport, ReaderClient: FakeClient };
    return load.call(this, name, parent, ...rest);
  };
  const filename = require.resolve("../src/extension");
  delete require.cache[filename];
  try {
    require(filename).activate({ subscriptions: [] });
  } finally {
    Module._load = load;
    delete require.cache[filename];
  }
  return {
    commands,
    messages,
    clients,
    contexts,
    change: () => configurationChanged({ affectsConfiguration: () => true }),
  };
}
test("activation registers six commands and a readonly virtual provider using only user grants", async () => {
  const extension = activateMock();
  assert.equal(extension.commands.size, 6);
  assert.equal(extension.contexts[0].scheme, "mdreader");
  await extension.commands.get("mdreader.status")();
  assert.deepEqual(extension.clients, [
    { dataDirectory: "/synthetic-reader-profile", sourceIds: ["selected"] },
  ]);
  assert.equal(extension.messages.length, 1);
  assert.ok(extension.messages[0].info.includes("2 documents"));
});
test("remote windows reject commands before reading endpoint or creating a client", async () => {
  const extension = activateMock({ remoteName: "ssh-remote" });
  await extension.commands.get("mdreader.status")();
  assert.deepEqual(extension.clients, []);
  assert.ok(extension.messages[0].error.includes("local desktop"));
});
test("a scope change suppresses stale asynchronous status results", async () => {
  let finish;
  const statusPromise = new Promise((resolve) => {
    finish = resolve;
  });
  const extension = activateMock({ statusPromise });
  const pending = extension.commands.get("mdreader.status")();
  extension.change();
  finish({ documents: 99, chunks: 99, embedded: 0, model: "unavailable" });
  await pending;
  assert.equal(extension.messages.filter((message) => message.info).length, 0);
  assert.ok(extension.messages[0].error.includes("outside the selected"));
});
