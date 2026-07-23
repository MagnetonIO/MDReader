---
title: Mermaid Flowchart
tags: [guide, diagram, mermaid]
---

# Mermaid Flowchart

The ingest-to-answer pipeline:

```mermaid
flowchart LR
    A[Markdown files] --> B[Ingest]
    B --> C[(Storage)]
    B --> D[Embed]
    D --> C
    C --> E{Query engine}
    E --> F[Lexical search]
    E --> G[Semantic search]
    F --> H[Ranked results]
    G --> H
    H --> I[Provenance-attached answer]
```

A second, tiny diagram in the same file:

```mermaid
flowchart TD
    Start --> Stop
```

Prose after the diagrams so it is not the last block. Related:
[sequence diagram](mermaid-sequence.md).
