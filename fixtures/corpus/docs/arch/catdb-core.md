---
type: crate
name: catdb-core
layer: platform
---

# catdb-core

The categorical kernel. Schemas, mappings, instances, and the two functors
between them. No IO, ever.

## Interfaces

### Schema Validation

Checks a schema's arrows, attributes, and equations before anything is
allowed to depend on it.

### Path Prover

Decides whether two paths through a schema are equal.

#### Limitations

Undecidable in general. The egg-backed prover is a decision procedure only
for the fragment CatDB's schemas actually use, and reports `Unknown`
otherwise rather than guessing.
