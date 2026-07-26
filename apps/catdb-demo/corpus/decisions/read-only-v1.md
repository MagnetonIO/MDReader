---
title: Read-only federation for V1
kind: decision
status: accepted
---

# Read-only federation for V1

CatDB is authoritative for semantic meaning, validation, mappings, evolution,
policy, and provenance. PostgreSQL, Snowflake, Drive, and Dropbox remain
authoritative for their stored records.

External connectors are read-only in V1. CatDB may write only to its own
catalog, indexes, provenance ledger, cache, and materializations.

Controlled external writes require a separate command model: propose,
validate, preview, authorize, apply, verify, and compensate.
