# Settings usability follow-up

Owner: root. Feature `feat/settings-usability`, based on native integration
`49e21b4`; merges to `dev-reader-next` after focused review. Companion web work
uses the same branch name, based on web integration `d1603ba`.

## Review findings and changes

- High: the open browser was still running the previous offline shell. Its
  downloaded update message had no adjacent action; reload lived deep in the
  old sidebar. New web versions expose Reload downloaded update in the notice
  banner and Settings feedback. Older already-running shells still need their
  existing update control once. This does not claim universal first-click
  service-worker upgrade acceptance.
- Medium: provider connections lived under Account, although they supply
  library documents independently of device sync. Native Google Drive/Dropbox
  controls now live in Library & sources, retaining their mounted owner and
  explicit connect/Picker authority. Backup links directly to source management.
- Medium: first-device setup required generating and handling a UUID, and
  pairing/recovery/troubleshooting were nested inside setup. Native setup now
  offers first-device versus existing-library choices. Creating a library
  generates its UUID only on the explicit action; retained libraries cannot
  create a replacement. Pairing, recovery and troubleshooting are separate
  collapsed sections. Existing-library IDs remain necessary until a supported
  discovery/pairing flow replaces them.
- Medium: account status competed with irrelevant actions and build limitations
  were unexplained. A compact native status card emphasizes available actions;
  unconfigured builds explain local reading and omit unavailable setup forms.
- Medium: native font/size changes lacked immediate feedback and some controls
  were smaller than 44 px. Appearance now has a live reading preview, labelled
  control groups and 44 px theme/font/size controls with keyboard focus.
- Medium: web mobile navigation consumed four rows of horizontal controls under
  enlarged text. A labelled section selector matches native mobile navigation.
  Desktop sections gain concise descriptions; Cmd/Ctrl+, opens web Settings.
  Inputs, selects and textareas gain explicit focus indicators.

## Focused validation

Native: existing affected Settings/account tests pass (31 distinct cases),
TypeScript and scoped ESLint pass. Chromium/WebKit Settings navigation and
keyboard checks pass (four scenarios), followed only by the two affected layout
scenarios after compact-spacing changes. Final screenshot inspection includes
320 x 640 mobile status and desktop appearance. IPC fixtures establish frontend
behavior, not live native account authentication or packaged-app acceptance.

Web component navigation/owner-lifetime regression, TypeScript and scoped ESLint
pass. Deployed Preview acceptance is recorded in the companion receipt. No Rust
build, model download, full test suite, dependency upgrade or lockfile change
is part of this UI slice.

## Remaining product gaps

Native recovery dialogs, installed-library preservation/migration, real provider
OAuth/refresh, native live enrollment and packaged UI acceptance remain separate
release gates. This change does not enable missing backend functionality or
present it as available. Browser recovery/pairing still requires explicit
verification and manual transfer; moving it behind a disclosure does not make
that workflow consumer-ready. A future device list and guided recovery flow must
be built on actual supported account capabilities.
