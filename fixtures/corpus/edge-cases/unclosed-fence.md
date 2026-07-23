---
title: Unclosed Fence
tags: [edge-case, code]
---

# Unclosed Fence

The paragraph before the fence is normal. Below, a code fence opens and is never
closed — the rest of the file should be treated as code until EOF, and the
parser must not hang or crash.

```rust
fn main() {
    println!("this fence is never closed");
    // no closing backticks follow — EOF ends the block
    let x = 1;
    let y = 2;
