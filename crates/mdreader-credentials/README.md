# Native provider custody

This crate accepts only fresh `native-provider-v3-<UUIDv4>` credential references.
`KeyringVault::new(app_identifier)` selects the separate
`<identifier>.native-provider-credentials-v3` namespace and does no keychain I/O.
It never imports or enumerates legacy broad grants. Tests use an injected memory
vault, never a live keychain.

`ProviderGrant` combines a validated native registration, verified stable
provider account, explicit exact-file or folder selection, and zeroized secret
bundle. It has no public serialization. Private keychain encoding is bounded,
versioned, rejects unknown schemas and validates construction again on load.
Debug output redacts grants, references, selected IDs and tokens. Request body
and header buffers are zeroized when dropped; HTTP/OS libraries still own copies
while performing their operations.

`VaultClient::new(Arc<dyn Vault>)` owns one persistent OS worker, one queued
operation, and bounded async deadline/cancellation waits. A hung OS call pins
that single worker; further calls fail or time out without creating replacement
threads. Restart is required to recover a permanently hung OS API. A credential
mutation can finish after its caller times out. Authority must be revalidated
separately before and after every provider effect; never infer source admission
or revocation from a vault timeout.

OS keychains do not offer cross-process compare-and-swap. Generation checks are
serialized under a process-wide custody mutex. The service is the sole mutator
of existing references; the native OAuth owner may create a fresh reference.
An uncertain source-admission reply must retain that reference and reconcile
against the registry rather than deleting it. Source disable commits its core
and registry authority gates before any optional credential cleanup.

Desktop keychain backends are macOS, Windows and Linux. Mobile vault operations
currently return `Unsupported`; this does not claim mobile OAuth or credential
custody acceptance. Provider private-cache authority remains separately gated on
supported hosts.

`Registration::dropbox_app_folder` takes native-owner reviewed registration
metadata's digest. OAuth scopes or a renderer checkbox cannot establish Dropbox
App Folder access. Google registrations support the exact `drive.file` Picker
flow only. No registration migration expands old permissions.

`Registration::dropbox_selected_folder` supports an existing scoped Dropbox app
without claiming provider-enforced App Folder isolation. Fresh PKCE consent,
verified scopes/account, and the native exact-path membership fence are required.
Its distinct encoding/digest cannot silently upgrade an old App Folder grant.
