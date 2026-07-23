---
title: Sample API Reference
tags: [sample, reference]
---

# API Reference

| Function        | Returns | Notes                    |
| --------------- | ------- | ------------------------ |
| `Parser::parse` | `Ast`   | Entry point              |
| `Ast::render`   | `Html`  | Lossy; drops frontmatter |

See the [parser notes](../src/lib/parser.md) for the design behind these.
