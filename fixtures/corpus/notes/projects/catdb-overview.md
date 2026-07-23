---
title: CatDB Overview
tags: [project, platform]
status: draft
---

# CatDB

CatDB is a content-addressed knowledge platform. It ingests documents, stores
them with provenance, and answers questions over them with hybrid search.

## Crates

| Crate | Responsibility |
| --- | --- |
| `catdb-ingest`* | Walk + parse sources |
| `catdb-storage` | Trait + backends (SQLite, Postgres) |
| `catdb-embed` | Text → vectors |
| `catdb-query` | Parse queries |
| `catdb-planner` | Plan execution |
| `catdb-provenance` | Track origins |

\* ingest for documents lives app-side in MDReader; the table is illustrative.

## How it fits together

Ingest feeds storage; the query engine reads storage; embeddings power the
semantic leg. The detailed note web starts at [[graph-hub]].

See also the app that sits on top: [MDReader](mdreader-architecture.md).

[^1]: The platform must never depend on any application crate — a boundary the
    CI guard enforces.

There is exactly one footnote here[^1].
