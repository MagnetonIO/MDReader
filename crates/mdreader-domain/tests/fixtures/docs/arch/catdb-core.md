---
type: crate
name: catdb-core
layer: platform
---

# catdb-core

The categorical kernel.

## Interfaces

### Schema Validation

Checks a schema's arrows, attributes, and equations.

### Path Prover

Decides whether two paths are equal.

#### Limitations

Undecidable in general. The egg-backed prover is a decision procedure only
for the fragment CatDB's schemas actually use, and reports `Unknown`
otherwise rather than guessing.
