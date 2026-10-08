# Installed local client acceptance — 2026-10-08

The [bounded receipt](client-hosts-2026-10-08.json) records actual macOS arm64
installed-client checks against synthetic documents and private disposable
profiles. This accepts the isolated host/client boundary below, not global
user-project setup or the packaged desktop entrypoint.

| Boundary | Observed result |
| --- | --- |
| Claude Code 2.1.294 | Service-owned project MCP install; initially Pending approval; explicit isolated user approval changed health to Connected; uninstall restored original project bytes. |
| Codex CLI 0.161.0 | Untrusted project MCP ignored; explicit isolated project trust loaded server; actual app-server discovered all six tools and `mcpServer/tool/call` returned exact revision/body with retrieval marker; uninstall restored original bytes. |
| VS Code 1.141.0 arm64 | Current VSIX installed in isolated extensions directory and activated in the real local Extension Host; exact virtual body, preview webview and reference command worked; unselected/stale reads rejected. |
| Direct stdio MCP | All six read-only tools invoked; exact source/revision/line evidence; unselected/stale rejection; no-model semantic search explicitly unavailable without lexical substitutes. |

VS Code host commit was `2a59476c9bfcb90b3ddc372c36762471b7dfad1c`.
No credentials were copied and no model turn was submitted. Known global
Claude/Codex/VS Code configuration files were unchanged, and no global extension
was installed. The synthetic fixture had two selected linked Markdown documents
and a separately admitted denied source. Service ran with `--no-model`.

The first real Extension Host run exposed VS Code's URI serialization of query
and fragment `=` delimiters as `%3D`. The content provider rejected its own
serialized URI. The parser now accepts literal or once-encoded structural
delimiters, preserves single path decoding, and rejects double-encoded delimiters
and additional query parameters. A focused test was observed RED then GREEN;
the installed fixed VSIX subsequently passed the real host checks.

Warm packaging also exposed npm's `extraneous` classification when reusing an
external symlinked dependency cache. License collection now follows only the
installed production dependency graph, including transitives and export-hidden
manifests. Its synthetic symlink fixture passed without a new install or lock
change. The final package succeeded; 15 extension tests and syntax checks passed.

## Reproduce

See the [driver instructions](../../../extensions/vscode/README.md#explicit-installed-client-acceptance).
The measured warm native build was:

```sh
CARGO_BUILD_JOBS=2 cargo build --locked --offline -p mdreader-service -p mdreader-mcp -p mdreader-capture
```

It finished in 10.97s. From `extensions/vscode`, `npm test`, `npm run check` and
`npm run package` passed. Actual phase commands used absolute paths to those
warm binaries and the packaged VSIX:

```sh
node scripts/acceptance/run.js --service /ABS/TARGET/debug/mdreader-service \
  --mcp /ABS/TARGET/debug/mdreader-mcp --capture /ABS/TARGET/debug/mdreader-capture \
  --vsix /ABS/REPO/extensions/vscode/artifacts/mdreader-1.0.0.vsix \
  --output /ABS/PRIVATE/host-receipt.json --clients vscode
node scripts/acceptance/run.js --service /ABS/TARGET/debug/mdreader-service \
  --mcp /ABS/TARGET/debug/mdreader-mcp --capture /ABS/TARGET/debug/mdreader-capture \
  --vsix /ABS/REPO/extensions/vscode/artifacts/mdreader-1.0.0.vsix \
  --output /ABS/PRIVATE/codex-receipt.json --clients codex,mcp
```

Claude passed in an earlier combined run whose VS Code phase failed; only that
Claude phase is included in the consolidated receipt. Successful fixtures were
removed. Failed synthetic fixtures were retained for diagnosis, then removed
after this bounded evidence was preserved. Exact binary and VSIX hashes identify
the tested artifacts; source base alone does not identify the later URI fix.
The tested VSIX predates this documentation update; a later release repack needs
its own artifact receipt.

## Remaining gates

- Synthetic launcher routes app flags to standalone binaries; the packaged
  desktop application entrypoint remains unverified here.
- Claude model-driven tool calls, authenticated agent turns and real visible
  final-output hooks remain unverified. Codex tool-call evidence used its real
  app-server API without a model turn.
- Agent approval/trust used explicit isolated user settings. Interactive trust
  dialogs, managed policy and real user-project setup remain separate gates.
- VS Code workspace trust was disabled only for this explicit synthetic fixture.
  Actual command registration/content/preview/reference ran in the installed
  extension; search/status/related transport ran in the real host. Search input
  and quick-pick selection UX were not driven.
- Linux/Windows, remote and web client runtime acceptance is not established.

Primary client contracts: [Claude MCP](https://code.claude.com/docs/en/mcp),
[Codex app-server](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md),
[VS Code Extension Host tests](https://code.visualstudio.com/api/working-with-extensions/testing-extension).
Codex request shapes were also checked against JSON schemas generated by the
actual installed CLI; version compatibility is claimed only for observed hosts.
