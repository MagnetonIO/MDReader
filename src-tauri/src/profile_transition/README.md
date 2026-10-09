# Native profile preservation and separate-profile migration

Unix desktop startup composes the session/settings JSON owner, reader daemon,
capture and account custody, provider mutations, unsaved editor and workspace
participants. The mounted renderer bridges archive current buffers and strictly
flush JSON. `native_profile_backup` exports a verified copy through these actual
owners; it does not switch profiles or transfer OS-vault credentials.

Every participating writer retains its effect lease until its issued work and
accepted or uncertain receipts settle. Preservation denies new admission, drains
issued work and holds exact native owner/root/operation capabilities through
flush, inventory, SQLite backup and verified publication. Cross-process SQLite
backup stays inside the service owner. Diagnostics classification and its real
lock are acquired after owner flush. Failed or uncertain proof cannot be replaced
by a renderer readiness flag, elapsed timeout or cancellation request. Invalid
owner proof quarantines admission; release notices follow actual owner release.

`offline/` owns the separate import lifecycle exposed by
`native_profile_migration`: preview, export originals, prepare selected copies,
resume, activate, rollback and close. A verified archive can be imported directly.
Direct installed legacy import retains recognized application identities before
the user closes the original normally, then requires observed termination, any
modern lifetime-owner locks and a consistent retained source snapshot. Format
recognition includes cloud and local workspace stores; it is not closure proof.
The native adapter never forces termination or accepts PID absence as proof.

Preparation preserves complete raw originals and unknown records alongside usable
Markdown, graph/review projections, relationships and supported preferences. True
modern device bodies, ready selected images, pins and reading positions remain
usable copies. Legacy source references without bodies remain explicitly
unresolved until the user grants a folder and binds an exact current copy.
Imported history retains individual occurrences and raw pins/anchors/receipts;
old anchors do not become current reading positions or source authorization.
Credentials, provider grants and replayable writes are never restored.

Activation records a verified candidate and updates the private native default
selection by exact compare-and-swap, so later ordinary launches select the new
profile. It is separate from backup's writer-resumption callback. Resume reconciles
retained durable state rather than guessing success from a lost reply. Legacy
rollback first preserves candidate changes, verifies all original content and the
retained original application identity, then clears the exact default selection
and requests launch of that original application. Originals and candidate copies
are retained. Closing an import releases volatile handles, not prepared data.

Combined synthetic SQLite, filesystem, startup, binding and UI checks have passed.
Actual installed-profile closure, full conversion, candidate launch and rollback
acceptance remain pending; compiled source and synthetic checks do not establish
that outcome. Direct retained legacy application closure is currently macOS-only.
Windows/mobile persistence and transfer features do not establish equivalent
installed-profile preservation authority.
