# Native account startup and public registration

Desktop account operations run in the existing reader service process. The GUI
does not construct another sync factory, SQLite writer or private credential
store. `native_sync::setup` supplies the account coordinator's reviewed factory,
system-browser adapter and exact registered loopback callback. The provider and
sync authorizers share one bounded native browser launcher.

Native sync is unconfigured by default. Public build-time metadata is required
as one complete registration; a partial/invalid registration is a typed account
error while local reading stays available. No startup code discovers endpoints
from a callback/token or reads/writes an OS credential merely to construct a
registration. `disabled` explicitly leaves sync off.

| Build variable | Required value |
| --- | --- |
| `MDREADER_NATIVE_SYNC_STAGE` | `development`, `production` or `disabled` |
| `MDREADER_NATIVE_SYNC_ISSUER` | Exact reviewed HTTPS issuer |
| `MDREADER_NATIVE_SYNC_CLIENT_ID` | Public client ID, never a client secret |
| `MDREADER_NATIVE_SYNC_AUDIENCE` | Exact sync API audience |
| `MDREADER_NATIVE_SYNC_AUTHORIZATION_ENDPOINT` | Reviewed HTTPS authorization endpoint |
| `MDREADER_NATIVE_SYNC_TOKEN_ENDPOINT` | Reviewed HTTPS token endpoint |
| `MDREADER_NATIVE_SYNC_JWKS_ENDPOINT` | Reviewed HTTPS JWKS endpoint |
| `MDREADER_NATIVE_SYNC_ENDPOINT` | Exact HTTPS sync origin; no path/query |
| `MDREADER_NATIVE_SYNC_REDIRECT_URI` | Registered `http://127.0.0.1:PORT/mdreader-sync/callback` |

These fields are public binary configuration. Never put a management token,
client secret, access token, recovery secret or key in them. Build tracking lists
every field so configuration changes rebuild the native composition. An exact
IPv4 loopback callback is bound before opening the system browser. The accepted
authority layer owns PKCE, state, nonce, issuer/audience/signature verification,
enrollment and cancellation; native startup does not implement another flow.
See [Auth0's PKCE registration contract](https://auth0.com/docs/get-started/authentication-and-authorization-flow/authorization-code-flow-with-pkce/add-login-using-the-authorization-code-flow-with-pkce).

One Auth0 issuer remains the identity provider for browser and native clients.
The public issuer metadata was fetched on 2026-10-08 and reports `/authorize`,
`/oauth/token`, `/.well-known/jwks.json` and S256 support under
`https://dev-8athhld0zd5r55fv.us.auth0.com/`. A credential-free authorization
probe for the existing public development client and
`http://127.0.0.1:43821/mdreader-sync/callback` returned HTTP 403 with callback
mismatch. **This callback is not yet accepted by the provider.** Do not bake an
assumed registration into the binary or claim native live sign-in acceptance.
Provider registration and an actual sign-in are separate release gates.

The OS-vault namespace includes the app identifier and a digest of the exact
canonical reader-owner directory. It contains no raw profile path. Device
documents/assets for desktop use `device-markdown-v1` and `device-assets-v1`
inside that explicitly selected service directory. A mobile embedded owner must
supply its already selected roots separately; startup never guesses a parent
app-data directory or moves an installed library. Desktop discovery/asset RPC
composition must use the same reader owner and these actual roots.

The browser launcher uses direct OS commands, null streams, one dispatch permit,
a five-second deadline and kill/reap on timeout. Dropping the operation kills its
owned launcher child; it never leaves a detached blocking worker to open a late
authorization page. An already-open system-browser page is outside this child
lifetime; the authority layer cancels its pending callback/PKCE capability.

Native recovery-file/secure-secret adapters, mobile system authentication and
mobile secure custody remain separate product gates. Unsupported recovery must
be disabled in Settings. `NativeSetup.recovery` is currently `None`; no renderer
secret form or fake native recovery adapter is supplied.

## Verification checkpoint

Three exact native-module tests passed in an isolated harness (0.24s, 15.34s
warm compilation, jobs=2): registration failure/disabled behavior, exact native
profile/app vault namespacing, and actual owned launcher deadline kill/reap.
The harness imports the original source modules and uses the existing workspace
registry versions/checksums, audited against `Cargo.lock`; no source copies or
new registry versions. The absent-registration assertion was added to the
existing registration test afterward and its focused rerun passed (0.25s).
Strict all-target Clippy for the exact-module harness passed (8.28s).
Service/RPC integration, generated bindings, packaged desktop
startup and live Auth0 acceptance are pending. This is a source checkpoint,
not a release or installed-app claim.
