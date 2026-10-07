"use strict";
const vscode = require("vscode");
const {
  ReaderClient,
  ReaderFailure,
  ensureLocalHost,
  validateSelection,
  selectionUri,
  parseSelectionUri,
} = require("./reader-client");
const { renderPreview } = require("./preview");

function activate(context) {
  const panels = new Set();
  const status = vscode.window.createStatusBarItem(
    vscode.StatusBarAlignment.Left,
    20,
  );
  status.text = "$(book) MDReader";
  status.tooltip = "Read-only selected local sources";
  status.command = "mdreader.status";
  status.show();
  context.subscriptions.push(status);
  let generation = 0;
  function client() {
    ensureLocalHost(vscode.env);
    const config = vscode.workspace.getConfiguration("mdreader");
    // Workspace settings are never a source grant or service endpoint selector.
    const userValue = (key) => {
      const value = config.inspect(key);
      return value?.globalValue ?? value?.defaultValue;
    };
    return new ReaderClient({
      dataDirectory: userValue("dataDirectory"),
      sourceIds: userValue("sourceIds"),
    });
  }
  function operation() {
    const service = client();
    const current = generation;
    return {
      service,
      assertCurrent() {
        if (generation !== current)
          throw new ReaderFailure("source_not_allowed");
      },
    };
  }
  function command(id, handler) {
    context.subscriptions.push(
      vscode.commands.registerCommand(id, async (...args) => {
        try {
          return await handler(operation(), ...args);
        } catch (error) {
          if (error instanceof ReaderFailure && error.code === "cancelled")
            return;
          status.text = "$(warning) MDReader";
          await vscode.window.showErrorMessage(
            error instanceof ReaderFailure
              ? error.message
              : new ReaderFailure().message,
          );
        }
      }),
    );
  }
  async function chooseReference(references, title) {
    if (!references.length) {
      await vscode.window.showInformationMessage(
        "No matching documents in the selected source.",
      );
      return;
    }
    if (references.length === 1) return references[0];
    const picked = await vscode.window.showQuickPick(
      references.map((item) => ({
        label: item.path,
        description: `Line ${item.line ?? item.line_start ?? 1}`,
        detail: `${item.source_id} · ${item.revision}`,
        reference: item,
      })),
      { title, matchOnDescription: true, matchOnDetail: true },
    );
    return picked?.reference;
  }
  async function open(op, reference) {
    const selected = validateSelection(reference, true);
    const checked = await op.service.read(selected);
    op.assertCurrent();
    const uri = vscode.Uri.parse(
      selectionUri({ ...checked, line: selected.line }),
    );
    const document = await vscode.workspace.openTextDocument(uri);
    op.assertCurrent();
    const editor = await vscode.window.showTextDocument(document, {
      preview: true,
      preserveFocus: false,
    });
    op.assertCurrent();
    const line = Math.min(
      selected.line - 1,
      Math.max(0, document.lineCount - 1),
    );
    const range = new vscode.Range(line, 0, line, 0);
    editor.selection = new vscode.Selection(range.start, range.end);
    editor.revealRange(
      range,
      vscode.TextEditorRevealType.InCenterIfOutsideViewport,
    );
    return { ...checked, line: selected.line };
  }
  function active() {
    const uri = vscode.window.activeTextEditor?.document.uri;
    if (!uri || uri.scheme !== "mdreader")
      throw new ReaderFailure("invalid_request");
    return parseSelectionUri(uri.toString());
  }
  async function preview(op, selection) {
    const document = await op.service.read(selection);
    op.assertCurrent();
    const panel = vscode.window.createWebviewPanel(
      "mdreader.preview",
      `MDReader: ${document.path}`,
      vscode.ViewColumn.Beside,
      {
        enableScripts: false,
        enableForms: false,
        enableCommandUris: ["mdreader.openReference"],
        localResourceRoots: [],
      },
    );
    panels.add(panel);
    panel.onDidDispose(
      () => panels.delete(panel),
      undefined,
      context.subscriptions,
    );
    panel.webview.html = renderPreview(document);
  }
  context.subscriptions.push(
    vscode.workspace.registerTextDocumentContentProvider("mdreader", {
      async provideTextDocumentContent(uri, token) {
        const op = operation();
        const selection = parseSelectionUri(uri.toString());
        const abort = new AbortController();
        const cancelled = token.onCancellationRequested(() => abort.abort());
        try {
          const document = await op.service.read(selection, abort.signal);
          op.assertCurrent();
          return document.markdown;
        } finally {
          cancelled.dispose();
        }
      },
    }),
  );
  command("mdreader.search", async (op) => {
    const query = await vscode.window.showInputBox({
      title: "Search selected MDReader sources",
      prompt: "Search returns revision-bound source and line evidence",
      validateInput: (value) =>
        Buffer.byteLength(value) > 1024 ? "Use at most 1024 bytes." : undefined,
    });
    if (!query?.trim()) return;
    const results = await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: "Searching MDReader",
        cancellable: true,
      },
      async (_progress, token) => {
        const abort = new AbortController();
        const cancelled = token.onCancellationRequested(() => abort.abort());
        try {
          return await op.service.search(query, undefined, abort.signal);
        } finally {
          cancelled.dispose();
        }
      },
    );
    op.assertCurrent();
    const selected = await chooseReference(
      results.map((item) => ({ ...item, line: item.line_start })),
      `Search: ${query}`,
    );
    op.assertCurrent();
    if (selected) await open(op, selected);
  });
  command("mdreader.open", async (op) => {
    const sources = await op.service.listSources();
    op.assertCurrent();
    const source = await vscode.window.showQuickPick(
      sources.map((source) => ({
        label: source.name,
        description: source.id,
        source,
      })),
      { title: "Open from selected source" },
    );
    op.assertCurrent();
    if (!source) return;
    const path = await vscode.window.showInputBox({
      title: `Document in ${source.source.name}`,
      prompt: "Source-relative Markdown path, for example notes/plan.md",
    });
    if (!path) return;
    const document = await op.service.read({
      source_id: source.source.id,
      path,
    });
    op.assertCurrent();
    await open(op, document);
  });
  command("mdreader.preview", async (op) => preview(op, active()));
  command("mdreader.backlinks", async (op) => {
    const selection = active();
    await op.service.read(selection);
    op.assertCurrent();
    const related = await op.service.related(selection);
    op.assertCurrent();
    const chosen = await chooseReference(
      related,
      "Related links (incoming and outgoing)",
    );
    op.assertCurrent();
    if (chosen) await open(op, chosen);
  });
  command("mdreader.openReference", async (op, argument) => {
    const selection = validateSelection(argument, true);
    if (typeof argument.target !== "string")
      throw new ReaderFailure("invalid_request");
    await op.service.read(selection);
    op.assertCurrent();
    const references = await op.service.resolve(selection, argument.target);
    op.assertCurrent();
    const chosen = await chooseReference(
      references,
      "Resolve document reference",
    );
    op.assertCurrent();
    if (chosen) {
      const opened = await open(op, chosen);
      await preview(op, opened);
    }
  });
  command("mdreader.status", async (op) => {
    const [sources, index] = await Promise.all([
      op.service.listSources(),
      op.service.status(),
    ]);
    op.assertCurrent();
    status.text = `$(book) MDReader ${index.documents}`;
    status.tooltip = `${sources.length} selected sources · ${index.chunks} chunks · ${index.embedded} embedded`;
    await vscode.window.showInformationMessage(
      `${sources.length} selected sources · ${index.documents} documents · ${index.chunks} chunks · ${index.embedded} embedded · ${index.model}. Lexical reading remains available without model embeddings.`,
    );
  });
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("mdreader")) {
        generation++;
        status.text = "$(book) MDReader";
        for (const panel of panels) panel.dispose();
      }
    }),
  );
  context.subscriptions.push({
    dispose() {
      for (const panel of panels) panel.dispose();
    },
  });
}
module.exports = { activate };
