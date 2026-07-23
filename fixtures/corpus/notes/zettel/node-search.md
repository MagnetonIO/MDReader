---
title: Search
tags: [zettel]
---

# Search

Search has two halves: lexical (trigram / full-text over [[node-storage]]) and
semantic (nearest-neighbour over [[node-embeddings]]). The
[[node-query-engine]] blends them.

Every hit links back to its source through [[node-provenance]], and the
[[node-graph-view]] can visualise the neighbourhood of a result. Hub:
[[graph-hub]].
