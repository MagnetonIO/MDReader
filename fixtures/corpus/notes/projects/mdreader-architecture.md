---
title: MDReader Architecture
tags: [project, app]
status: draft
---

# MDReader Architecture

MDReader is the desktop reader that sits on top of [CatDB](catdb-overview.md).
It is a Tauri 2 app: a Rust core, a web view, and generated IPC bindings.

## Layers

1. `mdreader-domain` — pure types.
2. `mdreader-ingest` — markdown parsing tuned for the reader.
3. `mdreader-presentation` — render models for the UI.
4. `mdreader-app` (`src-tauri`) — the shell that ties it together.

The TypeScript bindings are generated from the Rust command surface and
committed; a guard script fails CI if they drift.

## Diagram

The rendering pipeline is drawn in
[the mermaid flowchart](../../docs/guides/mermaid-flowchart.md).

![architecture sketch](../../assets/diagram.png)

Concepts referenced: [[node-ingest]], [[node-graph-view]], [[node-search]].
