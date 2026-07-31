# Demo container image policy

The public demo image is a reproducible input to the CatDB evidence surface,
not an automatically floating runtime.

- Builder and runtime bases are pinned to multi-platform OCI index digests.
- Local image validation uses the Docker host's native architecture. Vercel
  builds `Dockerfile.vercel` as a first-class container Function; no
  Cloudflare Worker, Container, D1, or Durable Object owns this runtime.
- The Rust builder version matches `rust-toolchain.toml`.
- Dependency resolution is locked; compilation runs offline after the
  explicit locked fetch.
- The build requires Vercel's exact lowercase `VERCEL_GIT_COMMIT_SHA` and
  passes it to Rust as `CATDB_DEMO_SOURCE_COMMIT`; startup rechecks the
  platform commit against the compiled identity.
- The runtime uses an unprivileged account and the baked corpus/workspace are
  read-only to that account.
- Image preparation writes `/app/metadata/deployment.json`, binding source
  commit, authoritative catalog revision, corpus digest, normalized logical
  workspace digest, SQLite byte digest, and a deployment fingerprint.
- Startup verifies the retained no-follow image descriptor before copying its
  exact bytes to one create-new private directory under `/tmp`; only that
  ephemeral copy may acquire SQLite sidecars.

Update a base only in a reviewed dependency-maintenance change: resolve the
new manifest-list digest from the official image registry, update the tag and
digest together, rebuild twice, run the logical-state and route-absence gates,
and record the resulting workspace and image digests. A tag-only update is
not accepted.
