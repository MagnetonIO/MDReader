---
title: Schema-first applications
kind: guide
---

# Schema-first applications

An application should be able to ask for a `Document`, `Customer`, `Claim`, or
`Contract` without binding every feature to the layout of one database.

CatDB keeps the versioned semantic schema and mappings. Existing systems remain
authoritative for their records.

## Read-only federation

Version one plans source-specific reads, pushes supported filters down, and
reconciles typed results. It does not write changes back to customer systems.

## Sources

The first connector set targets PostgreSQL, Snowflake, Google Drive, and
Dropbox. The embedded MDReader runtime also uses filesystem, Git, frontmatter,
SQLite, full-text search, and vectors.
