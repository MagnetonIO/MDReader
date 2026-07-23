---
title: Strikethrough and Autolinks
tags: [guide, gfm]
---

# Strikethrough and Autolinks

## Strikethrough

We ~~use RocksDB~~ use SQLite and Postgres. The ~~old plan~~ is gone.
Double tildes are required: ~this~ is not struck through in strict GFM.

## Autolinks

Bare URL, should linkify: https://supabase.com/docs

Bare URL with path and query:
https://github.com/catdb-project/catdb/issues?q=is%3Aopen

Email autolink: mlong168@gmail.com

Angle-bracket autolink: <https://example.com/angle>

Not a link, just text with a dot: example dot com.

Compare with the explicit links back to [tables](gfm-tables.md) and
[task lists](task-lists.md).
