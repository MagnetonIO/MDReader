---
title: Code Fences
tags: [guide, code]
---

# Code Fences

Fenced code in several languages. Each closes properly (the unclosed case lives
in `edge-cases/unclosed-fence.md`).

## Rust

```rust
pub fn embed(text: &str) -> Vec<f32> {
    text.split_whitespace().map(|w| w.len() as f32).collect()
}
```

## Python

```python
def chunk(md: str, size: int = 512) -> list[str]:
    return [md[i : i + size] for i in range(0, len(md), size)]
```

## TypeScript

```typescript
export function toBindings(cmds: string[]): string {
  return cmds.map((c) => `export const ${c}: Command;`).join("\n");
}
```

## SQL

```sql
create index notes_trgm on notes using gin (body gin_trgm_ops);
select id from notes order by embedding <-> $1 limit 10;
```

## Bash

```bash
#!/usr/bin/env bash
set -euo pipefail
cargo test --workspace
```

## JSON

```json
{ "path": "index.md", "features": ["hub", "image", "wikilink"] }
```

## TOML

```toml
[db]
port = 54322
major_version = 17
```

## Indented (no language)

    plain indented code block
    still code

An inline `code span` and a fence with a tilde delimiter:

~~~text
tilde-fenced block
~~~
