# Demo container image policy

The public demo image is a reproducible input to the CatDB evidence surface,
not an automatically floating runtime.

- Builder and runtime bases are pinned to multi-platform OCI index digests.
- The Rust builder version matches `rust-toolchain.toml`.
- Dependency resolution is locked; compilation runs offline after the
  explicit locked fetch.
- The runtime uses an unprivileged account and the baked corpus/workspace are
  read-only to that account.
- The build records the actual SQLite byte digest at
  `/app/metadata/workspace.sha256`. The server remains latched off until the
  reviewed catalog-revision authority can expose that digest together with
  the exact source commit and verified catalog revision.

Update a base only in a reviewed dependency-maintenance change: resolve the
new manifest-list digest from the official image registry, update the tag and
digest together, rebuild twice, run the logical-state and route-absence gates,
and record the resulting workspace and image digests. A tag-only update is
not accepted.
