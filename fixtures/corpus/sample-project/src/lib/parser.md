---
title: Parser Module Notes
tags: [sample, code-adjacent]
---

# Parser Module

Design notes that live beside the code they describe (`src/lib/`). This is the
third directory level under the sample project, so it exercises deep-path
ingest.

```rust
pub struct Parser;

impl Parser {
    pub fn parse(input: &str) -> Ast {
        Ast::from(input)
    }
}
```

Up to the [project overview](../../overview.md); across to the
[API docs](../../docs/api.md).
