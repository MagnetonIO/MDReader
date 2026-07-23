---
title: Embeddings
tags: [zettel]
---

# Embeddings

Embeddings turn text into vectors so we can measure similarity. In CatDB they
are produced by `catdb-embed` and stored via [[node-storage]] using pgvector.

They feed [[node-search]] (semantic half) and are planned by
[[node-query-engine]]. Back to the [[graph-hub]].

Dimensionality is a tradeoff: bigger vectors capture more nuance but cost more
to index and compare.
