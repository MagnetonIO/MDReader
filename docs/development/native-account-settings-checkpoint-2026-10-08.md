# Native Account & sync source checkpoint

This is an **unvalidated source checkpoint**, based on native major `2be0006`.
It is intended for composition in the native startup feature branch, not release
or accepted integration. No Cargo, frontend check, model, live authentication,
installed-profile access or provider request was run for this checkpoint.

## Boundaries

- The existing private service protocol gains feature-gated Account status,
  begin, poll, exact-job cancellation and disconnect requests. The same service
  coordinator owns the session, browser/network work and native custody.
- Native startup supplies reviewed setup as `Result<Option<NativeSetup>,
  AccountError>` and explicit CacheRoots. Setup failures stay typed and do not
  disable local reader startup. This slice never infers cache paths.
- A desktop-only `reader_accounts::reader_account` command projects safe DTOs.
  UUIDs, fixed errors, explicit public pairing packets and decimal counter strings
  cross the renderer boundary. Keys, tokens, recovery secrets, credential
  references and filesystem paths do not.
- The categorized Settings Account section gains explicit sync management.
  Enrollment/pairing/recovery are behind an advanced disclosure. No sign-in or
  upload selection happens on mount. Unknown/lost replies require explicit state
  refresh, never an automatic replacement authorization.
- Polling is single-flight, at most once per second for twelve minutes, and stops
  when hidden, outside its category or unmounted. Observation cleanup never
  implicitly cancels the service-owned operation. User cancellation/disconnect
  are explicit operations.

## Required composition before checks

1. Merge the account coordinator's public wire derives from the pump owner:
   AccountIntent Clone/Serialize; Deserialize on status/job/outcome public DTOs.
2. Add/project the pump's typed SyncPending evidence and recovery capability.
   The current UI must disable unavailable recovery, not infer it from enrollment.
3. Root wires desktop feature/dependencies, native setup/cache roots, command
   collector and module registration. Generate actual Specta bindings from that
   coherent native artifact; **do not hand-edit generated bindings**. The adapter
   currently references those future generated Account DTOs/readerAccount method.
4. Run the focused new service RPC/runtime cases, native input/counter projection
   case, four UI lifecycle/evidence cases, affected Settings behavior, scoped
   type/lint and affected strict Rust checks under the coordinated slot. These
   authored cases are not yet executed and no RED/GREEN evidence is claimed.

## Product gates

Sync apply/ack counters are shown as actual evidence, never a generic success or
proof of desktop discoverability. Service-owned incoming documents still need a
same-owner device catalog/read/asset adapter before claiming a useful complete
synced-document reader workflow. No implicit folder grants or provider reads may
bridge that gap. Live native OIDC registration, secure custody/recovery dialogs,
packaged Settings geometry and actual multi-device acceptance remain separate.
