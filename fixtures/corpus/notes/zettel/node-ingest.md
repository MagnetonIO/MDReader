---
title: Ingest
tags: [zettel]
---

# Ingest

Ingest walks a directory, respects `.gitignore` files it finds, parses each
markdown document, and writes the results into [[node-storage]]. The sample
project under `sample-project/` exercises the ignore rules.

Every ingested chunk carries [[node-provenance]] so we can trace it back to a
file and line. Similar text becomes embeddings via [[node-embeddings]].
Home: [[graph-hub]].
