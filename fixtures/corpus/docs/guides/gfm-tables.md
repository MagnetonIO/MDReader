---
title: GFM Tables
tags: [guide, gfm]
---

# GFM Tables

A plain table:

| Backend  | Local | Server |
| -------- | :---: | -----: |
| SQLite   |  yes  |     no |
| Postgres |   no  |    yes |

Alignment above: centre, then right. A table with inline formatting and a
pipe escaped inside a cell:

| Feature      | Notes                                  |
| ------------ | -------------------------------------- |
| **bold**     | rendered strong                        |
| `code`       | monospace                              |
| a \| literal | the pipe is escaped, not a new column  |
| [link](gfm-tables.md#gfm-tables) | self reference             |

A ragged table (missing trailing cells) that parsers must tolerate:

| A | B | C |
| - | - | - |
| 1 | 2 |
| 3 |

Next up: [task lists](task-lists.md).
