# Standalone native smoke harness layout

Root owns `fix/native-smoke-layout`, based on reader integration `5429193`.
Destination: `dev-reader-next`; clean up the minor after acceptance.

The desktop build wrapper, mobile identity handoff, artifact validator and
macOS/desktop runner still resolved three parents above `e2e-native`, from the
former monorepo. In this standalone checkout that dispatched pnpm/Git in an
unrelated ancestor and staged the mobile source identity under nonexistent
`apps/mdreader/src-tauri`. This blocked the documented smoke startup path before
any actual GUI behavior could be checked.

`e2e-native/repository.mjs` now owns the exact shared repository/harness/handoff
paths relative to its module. The consumers use those paths regardless of the
caller's directory; the diagnostic points to `pnpm --dir e2e-native run build:e2e`.
No discovery fallback searches other repositories. Existing isolated bundle
identity, exact source provenance and refusal to overwrite/remove a changed
handoff are preserved.

## Evidence

- Eleven affected build-wrapper/identity-handoff cases pass in 179 ms. A real
  child process runs the desktop wrapper from an unrelated temporary directory
  with a recording pnpm executable: both commands use this checkout and the
  exact configured source commit. No build or installation is performed.
- A separate real Git subprocess obtains this checkout's HEAD from that same
  unrelated directory. The default mobile handoff stages in this worktree's
  `src-tauri` and cleans its exact bytes; a preexisting handoff is not overwritten.
- Restoring the former three-parent root makes the new wrapper case fail at
  the actual dispatched working directory. Restoring the fix makes it pass.
- Three affected existing desktop artifact/runner binding cases pass in 73 ms.
- Changed JavaScript/TypeScript entrypoints pass Node syntax checks;
  `bash -n scripts/macos-smoke.sh` and `git diff --check` pass.
- No Cargo process, package install, Appium server, GUI interaction, profile
  clearing or packaging was performed. Full harness typecheck was not run:
  the independent Appium/WebdriverIO toolchain is not installed in this tree.

This fixes path resolution only. The current unified legacy flow still includes
CatDB-era review/engine scenarios; reader-first release acceptance must exercise
the actual final product. This does not establish fresh-clone readiness,
packaged GUI lifecycle, physical mobile acceptance or CatDB removal. The
machine-local CatDB SDK dependency remains the principal build portability gate.
