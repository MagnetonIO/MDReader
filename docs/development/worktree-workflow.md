# Reader migration worktrees

The original MDReader and MDReader-Web checkouts remain on `dev`. Each repository
uses `dev-reader-next` as its migration integration branch. Only the coordinator
edits integration metadata and merges reviewed feature commits there. Feature
implementation takes place in separate worktrees, including the coordinator's.

Paths below are relative to `/Users/mlong/Documents/Development/.worktrees/`.
The base is the commit at isolation; later merges/rebases must be recorded in the
feature's review or acceptance receipt.

| Owner | Repository | Worktree | Branch | Initial base | Dependency |
| --- | --- | --- | --- | --- | --- |
| Coordinator | Native | `MDReader-reader-next` | `dev-reader-next` | `21bc807` | Integration only |
| agent_capture | Native | `MDReader-incoming-publication` | `feat/reader-incoming-publication` | `21bc807` | Existing portable device and asset owner |
| reader_mcp | Native | `MDReader-capture-platform` | `feat/reader-capture-platform` | `21bc807` | Independent capture extraction/platform work |
| Coordinator | Native | `MDReader-sync-application` | `feat/reader-sync-application` | `21bc807` | Incoming owner commit required before service composition |
| Coordinator | Web | `MDReader-Web-reader-next` | `dev-reader-next` | `e2c86d5` | Integration only |
| cloud_sync | Web | `MDReader-Web-preview-acceptance` | `fix/reader-preview-acceptance` | `e2c86d5` | Committed Preview manifest parser fix `0b28dab` |

Existing unfinished edits were backed up, copied to their owned worktree and
verified file by file with SHA-256 before removing/restoring the original copies.
Both integration worktrees were clean after isolation. No implementation was
accepted or merged into `dev` as part of this move.

One heavyweight check runs across the whole team at a time. Native worktrees
reuse the existing Cargo target cache with serialized writers. Preview checks
reuse the existing compatible pnpm dependencies; no extra cold build or install
is required merely to isolate source. Keep all private configuration ignored.

For each feature or bug:

1. Finish scoped changes, focused tests and review in its owned worktree.
2. Commit the reviewed paths and merge that branch into `dev-reader-next` in
   dependency order. Run relevant combined-tree checks before accepting it.
3. Confirm the feature commits are ancestors of integration, the feature tree
   is clean (including untracked files), and no process is using it. Preserve
   private configuration and receipts, then use `git worktree remove` and
   `git branch -d`; never force-remove unfinished work.
4. Once the release integration gates pass, merge `dev-reader-next` into `dev`
   and remove the clean integration worktree/merged branch by the same checks.
   Push or deploy only to the intended target; merge is not deployment acceptance.

Do not remove unrelated historical branches or worktrees during this cleanup.
