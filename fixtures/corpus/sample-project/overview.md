---
title: Sample Project
tags: [sample, project]
---

# Sample Project

A miniature source tree used to test `.gitignore`-aware ingest. It has its own
`.gitignore` that excludes `generated/`, so anything under that directory should
be skipped by the walker even though it lives in the corpus.

## Layout

- `src/lib/parser.md` — a doc that lives next to code.
- `docs/api.md` — reference docs.
- `generated/build-report.md` — machine output; **must be ignored**.

Start reading at [the parser doc](src/lib/parser.md).
