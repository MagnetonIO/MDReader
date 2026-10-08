# Reader migration worktrees

The original MDReader and MDReader-Web checkouts remain on `dev`. Each repository
uses `dev-reader-next` as its retained migration integration branch. Major feature
branches may remain while their features evolve; minor feature/bug branches merge
into a major branch, which merges directly into `dev` or through integration.
Only the coordinator
edits integration metadata and merges reviewed feature commits there. Feature
implementation takes place in separate worktrees, including the coordinator's.

Paths below are relative to `/Users/mlong/Documents/Development/.worktrees/`.
The base is the commit at isolation; later merges/rebases must be recorded in the
feature's review or acceptance receipt.

| Owner | Repository | Worktree | Branch | Initial base | Dependency |
| --- | --- | --- | --- | --- | --- |
| Coordinator | Native | `MDReader-reader-next` | `dev-reader-next` | `21bc807` | Integration only |
| agent_capture | Native | `MDReader-profile-transition` | `feat/reader-profile-transition` | `165e180` | Writer quiescence, verified preservation and durable activation/rollback |
| reader_mcp | Native | `MDReader-reader-catalog` | `feat/reader-catalog` | `e3608e9` | Reader-owned paged catalog and primary-screen CatDB cutover |
| cloud_sync | Native | `MDReader-native-key-lifecycle` | `feat/reader-native-key-lifecycle` | `e3608e9` | Persistent custody v2 and standard JWE interoperability |
| Coordinator | Native | `MDReader-native-oidc` | `feat/reader-native-oidc` | `c2055ea`, then `fa3f577` | Auth0 access-token contract and bounded native loopback |
| Coordinator | Web | `MDReader-Web-reader-next` | `dev-reader-next` | `e2c86d5` | Integration only |
| cloud_sync | Web | `MDReader-Web-public-oidc` | `feat/reader-public-oidc` | `7f352a8` | One Auth0 issuer, API audience and separate public clients |
| Coordinator | Web | `MDReader-Web-upgrade-acceptance` | `fix/reader-offline-upgrade` | `092623d` | Deployed old-to-new cache failure; fix under acceptance |

Completed and merged into integration: capture/platform extraction (`72ecd9b`),
core incoming publication (`1ecf76f`), session and atomic cursor fence (`c13a6ad`),
default-feature lint fix (`0a1787f`) and deployed Preview acceptance (`1501489`).
Further accepted slices: new-content service incoming (`48c647f`), actual client/
installed extension acceptance (`554f450`), native auth-v2 (`20fbf70`) and companion
web auth-v2 (`65bf43f`), native bounded callback ownership (`a4607b4`) and the
configured development deployment receipt (`88abc58`). The client-acceptance worktree was removed after preserving
the tested VSIX and receipts; its major branch remains. Incoming work now starts
from `e3608e9` for service v2 transitions, accepted as `bc6ca6e`. Its clean,
inactive worktree was removed normally after ancestry/process checks; major
branch retained. Native custody/JWE checkpoint (a) is merged as `ce9ace2`; the
key-lifecycle major refreshed through `9abc10c` for checkpoint (b) code. Core state/retirement (`c54b959`)
and archived review presentation/schema hardening (`fb02b38`, `db1fc37`) are merged.
The archived-review worktree was removed cleanly; its major branch remains.
The web Connect fix (`5bb05b2`, receipt `c09c0cb`) merged through its owning major
branch into web integration; its clean minor worktree and branch were removed.
The enlarged empty-reader fix (`b25bc33`, receipt `5eedd56`) also merged via the
web major into integration `092623d`; its minor worktree and branch were removed.
Their clean, inactive worktrees and merged minor branches were removed after
ancestry checks. Private configuration and receipts were preserved; shared
caches were retained. The major feature branches in the table remain intentionally
while implementation and review continue. Neither integration has reached `dev`.

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
2. Commit reviewed minor changes and merge into their owning major feature or
   integration branch in dependency order. Run relevant combined-tree checks.
   For this migration the recorded destination is `dev-reader-next`.
3. Confirm the feature commits are ancestors of integration, the feature tree
   is clean (including untracked files), and no process is using it. Preserve
   private configuration and receipts, then use `git worktree remove` and
   `git branch -d`; never force-remove unfinished work.
4. Once release integration gates pass, merge `dev-reader-next` into `dev`.
   Retain this branch if further migration integration is planned; retire its
   clean worktree/merged branch when that work ends. Major feature branches may
   likewise remain intentionally, while unused worktrees are cleaned separately.
   Push or deploy only to the intended target; merge is not deployment acceptance.

Do not remove unrelated historical branches or worktrees during this cleanup.
