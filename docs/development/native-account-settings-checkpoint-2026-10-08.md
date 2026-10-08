# Native Account & sync source checkpoint

The native startup feature branch composes the account coordinator, wire bridge
`3ab167b`, RPC/UI corrections `dcb7836`, reviewed registration and desktop command
collector. The combined artifact has generated actual Rust-to-TypeScript bindings
and passed the focused checks below. This is source and browser-layout acceptance,
not an installed desktop, live authentication or release claim.

## UI review outcomes

The original long sidebar mixed normal reading preferences, account state and
raw security operations. The categorized modal now gives Appearance, Library &
sources, Account & sync, Backup & storage and Advanced their own view. Mobile
uses a labeled section selector and a fullscreen sheet.

Device sync comes first in Account & sync so its status is not pushed below
provider setup. Technical custody evidence is behind Security details; device
pairing, recovery and troubleshooting have separate disclosures. Unsupported
actions remain disabled with a reason. Account controls use theme-aware borders,
text, visible keyboard focus and 44-pixel targets instead of browser-default
buttons. Cloud-source selection explains that it does not enable device sync.

The design-critique skill supplied the hierarchy, consistency and accessibility
review framework. Browser geometry checks isolate presentation; they do not
establish native authentication or real provider acceptance.

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

## Combined validation

- One warm native app library-test build, locked/offline, jobs=2, completed in
  1m33s. Only the exact binding generator ran (0.04s); bindings were not edited
  manually. The build reports seven existing legacy dead-code warnings; whole-app
  strict Clippy is not claimed.
- The same native executable ran four exact tests without recompilation: closed
  native input and precise pending counters; absent/invalid public registration;
  exact profile-bound vault namespace; actual browser-launcher kill/reap.
- Service `--features sync-native --lib accounts`: five tests passed, including
  closed RPC, typed configuration-failure isolation, retained custody/schema and
  cancellation fences (0.30s; 12.26s warm compilation).
- Service all-target strict Clippy passed (6.67s).
- Account panel plus affected Settings suite: 28 tests passed (1.81s). A broad
  diagnostic-copy query initially matched hidden account copy; its diagnostic
  assertion is now scoped to the actual diagnostic message.
- UI TypeScript and scoped ESLint passed. The new Settings browser config is
  included in TypeScript checking.
- Chromium and WebKit checked category navigation, 320x640 fullscreen layout,
  unavailable action state, theme, log confirmation, shortcut, Escape and focus
  restoration. The layout cases passed after fixing stale flat-layout assertions,
  an exact-text punctuation mismatch and an ambiguous mobile selector. Current
  screenshot-driven button styling and account-first layout passed their
  affected rerun in both engines (3.7s), including expanded unconfigured provider
  controls and no horizontal overflow. Three affected Settings tests then passed
  (0.18s); UI typecheck and scoped lint passed again.

Run `pnpm --filter @mdreader/app test:settings-browser` for these two UI scenarios
in both engines, with one worker. This config overrides the broad harness setup,
so it starts only Vite and uses explicit IPC fixtures; it does not start a CatDB
server or compile Rust. Fixture status is unconfigured with recovery unavailable;
no mocked authentication or sync result is presented as native acceptance.

## Product gates

Sync apply/ack counters are shown as actual evidence, never a generic success or
proof of desktop discoverability. Service-owned incoming documents still need a
same-owner device catalog/read/asset adapter before claiming a useful complete
synced-document reader workflow. No implicit folder grants or provider reads may
bridge that gap. Live native OIDC registration, secure custody/recovery dialogs,
packaged Settings geometry and actual multi-device acceptance remain separate.

## Reviewed source correction checkpoint

The public wire bridge `3ab167b` was cherry-picked as dependency `886bb6b`.
Empty Status/Disconnect RPC commands and all empty native input actions now use
empty struct variants, retaining extra-field rejection fixtures. The projection
includes actual recoveryAvailable and typed pending sequence/reason evidence;
all counters and sequences preserve precision as decimal strings.

Account category activation reads metadata immediately, without authorization.
There is no extra management accordion before connection state. Set up sync
contains separate Devices, Recovery and Troubleshooting disclosures. Recovery
controls are disabled with an explanation when the native adapter is absent.
Background observation retires when the category becomes inactive; it never
cancels the service actor implicitly.

The focused cases cover four UI lifecycle/evidence tests, one native closed
input/precision projection test, one RPC bounds/closed-input/unconfigured test
and one runtime failure-isolation test. The first UI case also proves an actually
configured account cannot offer recovery without its native capability. The
native precision fixture uses actual HeadConflict pending evidence. These source
corrections passed the combined checks recorded above; packaged native lifecycle
and actual remote authentication remain separate gates.
