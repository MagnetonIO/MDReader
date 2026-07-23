---
title: Query Engine
tags: [zettel]
---

# Query Engine

The query engine takes a parsed query and, with help from [[node-planner]],
decides how to combine lexical and vector search over [[node-storage]].

Results are ranked and returned with [[node-provenance]] attached. It leans on
[[node-embeddings]] for the semantic leg and [[node-search]] for the lexical
leg. Anchor: [[graph-hub]].
