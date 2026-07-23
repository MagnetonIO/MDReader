---
title: Storage
tags: [zettel]
---

# Storage

Storage is pluggable: a SQLite backend for local single-user work and a Postgres
backend for the server. Both implement the same trait so [[node-query-engine]]
does not care which is underneath.

The Postgres path uses the `vector` and `pg_trgm` extensions (see the
`0001_extensions` migration). Written to by [[node-ingest]], read by
[[node-search]]. Return to [[graph-hub]].
