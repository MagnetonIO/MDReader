# Reader design review — 9 October 2026

Owner: root, with independent reviewer. Reviewed source: `bbfe4ea4f7fb5038d2b2f5934cce505bc1635235`.
This extends the earlier Settings usability review; it does not undo its fixes.

## Evidence and limits

This review combines the user's desktop screenshots with current desktop/mobile
source. Screenshots establish reported presentation problems, not the revision of
every running window. Source findings below are current at the reviewed commit.
No new device launch, iOS package, authenticated provider run or visual acceptance
is claimed. The existing status ledger remains authoritative for release gates.

The earlier iOS simulator package `69f03b5` passed four synthetic runtime cases
covering selected import, exact reads/search, pins, PNG/JPEG admission and restart
preservation. It predates current source. Latest reading-position restoration,
current visual layout, physical-device, provider OAuth and offline lifecycle
acceptance are still open. Android results do not establish iOS behavior.

## Assessment

The interface accumulates controls and status text without a consistent hierarchy.
Routine reading competes with provenance, recovery, import and sync operations.
Separate styling contexts make the same feature look different across hosts.
A component-library replacement alone would preserve these product problems.

Useful foundations already exist: categorized desktop Settings, explicit source
authority, compact provider sidebar controls, safe-area handling, touch-sized
mobile controls and a shared Markdown renderer. Build the redesign on these.

## Prioritized findings

| Priority | Finding and source evidence | Required change |
| --- | --- | --- |
| High | Mobile reader header renders Back, title, Pin, Evidence, Outline and Sync (`ui/src/mobile/DeviceAppShell.tsx:533`), but its grid defines five columns (`ui/src/mobile/mobile.css:75`). The sixth item enters an implicit row. | Keep Back, title and an overflow menu in the phone header. Put bookmark, outline, share/export, sync and document details in deliberate secondary controls. Do not squeeze a sixth column into the existing header. |
| High | File lookup scores the full absolute path (`packages/ui/src/search/fuzzy.ts:70`). Query `rad` therefore matches the shared workspace prefix in the user's screenshot, even for unrelated filenames. Rows print that full path (`packages/ui/src/search/search-palette.tsx:674`). | Preserve absolute paths for opening/identity; rank on filename plus workspace-relative path. Display a short relative parent path beneath the filename, retain full path in accessible details, and handle outside-root entries without widening source access. |
| High | File lookup and content search share a palette, but file mode removes the intent tabs (`packages/ui/src/search/search-palette.tsx:611`). Filename rows provide no evidence that document bodies are indexed. | Visibly label Files versus Content, show the active source scope, and expose actual index readiness in content mode. Never infer coverage from file-list length; lexical and semantic readiness must remain distinct. |
| High | Mobile mounts `AccountSyncView` directly (`ui/src/mobile/MobileSyncSettings.tsx:22`), while desktop typography/action styling relies on `.settings-content` and `#settings-category-account` ancestors (`ui/src/features/settings/settings.css`). Mobile adds partial overrides. | Give Settings rows, fields, status cards and actions shared presentation components or host-independent classes. Keep state/authority logic shared; supply phone presentation explicitly. Show the current account state and one relevant next step. Put public packets, identifiers and operation diagnostics under Advanced. |
| High | Imported-device reading consumes theme but does not apply the shared reading appearance wrapper (`DeviceAppShell.tsx:624–638`); cloud reading applies it through `CanonicalReader`. Mobile Settings offers theme only. | Share font and text-size controls across imported and cloud documents. Apply changes without remounting the reader or losing its saved position. Verify visible changes and persistence on both paths. |
| Medium | Mobile defines separate hardcoded light/dark palette variables (`ui/src/mobile/mobile.css:1`), while reused panels use `--color-*`. Header, body, captions and actions also use scattered sizes/weights. | Establish semantic color, type, spacing, radius, focus and action tokens. Keep document typography separate from interface typography. Resolve platform aliases through the same token contract, with deliberate density differences. |
| Medium | Multiple text actions, details panels and transfer controls can precede the document (`DeviceAppShell.tsx:533–624`). Each open panel may consume part of the reader height. | Give the document visual priority. Open outline/details as accessible sheets or panels with clear dismissal and restored focus. Normal reading must retain a useful viewport at small phone widths and enlarged text. |
| Medium | The user's desktop screenshots show equal emphasis for primary/secondary actions, long explanatory copy, duplicate-looking filenames and unclear distinctions between connected, enumerated, downloaded and searchable. Several have since received source fixes; current runtime must be rechecked. | Use one primary action per setup state, quiet secondary actions, icon refresh with an accessible name, concise status, and Settings-only search participation. Keep required permission explanations at the decision point. Verify the actual running package. |
| Medium | Reused provider fields retain 14 px input styling outside the mobile field's 16 px protection; mobile tabs are small text-only buttons, Evidence/Outline lack expanded state, and cloud reader height subtracts a fixed 100 px (`ui/src/mobile/mobile.css`, `DeviceAppShell.tsx`). | Apply phone input sizing to every reused field, expose accessible toggle state, and use available flex height rather than an assumed toolbar height. Check iOS focus zoom, safe areas, keyboard, landscape and tablet touch layouts. |

## Design direction

Use a calm reading-first MDReader identity with shared semantics and tokens.
For iOS/macOS, follow Apple navigation, typography and grouped Settings conventions.
For Android, use Material 3 navigation, action hierarchy and adaptive layout
principles. Web/Windows should retain familiar keyboard, focus and desktop density.
These are presentation guidelines, not a decision to replace React/Tauri, introduce
a new UI dependency or rewrite the native shell.

Start with three representative surfaces: Library, Reader and Settings. Phone
Library should show concise document rows and source context. Reader should have
a compact header and a clear overflow menu. Settings should show connected account
name/status, concise choices and guided next steps. Preserve the user's requested
compact cloud sidebar; do not reintroduce About, loaded counts or search checkboxes.

Primary references:

- [Apple layout](https://developer.apple.com/design/human-interface-guidelines/layout)
- [Apple tab bars](https://developer.apple.com/design/human-interface-guidelines/tab-bars)
- [Apple typography](https://developer.apple.com/design/human-interface-guidelines/typography)
- [Material 3 typography](https://m3.material.io/styles/typography/applying-type)
- [Material 3 app bars](https://m3.material.io/components/app-bars/)

Independent reviewer confirmed the header and path-ranking findings, and added
the imported-reader appearance, reused-input sizing, toggle state and fixed-height
findings above. This is a completed source/design critique, not rendered-device
acceptance or an implementation sign-off.

## Delivery and acceptance

1. Frontend owns a scoped design branch from the agreed integration commit;
   backend supports honest readiness/coverage only where existing DTOs cannot.
   Preserve source identity, private configuration and profile custody.
2. Produce Library/Reader/Settings examples using public fixtures, including
   repeated README names, long account names, loading/error/connected states and
   unavailable sync. Review visual hierarchy before adding more controls.
3. Check light/dark, 320–430 px phones, tablet split view and desktop. Check enlarged
   text, safe areas, keyboard focus, VoiceOver/TalkBack labels and touch targets.
   Choose sufficient contrast from measured rendered colors, not token names.
4. Verify file lookup with a workspace whose name matches the query: unrelated
   README rows must not match solely through that prefix; relative paths must
   distinguish repeated names; opening must retain the exact authorized target.
5. Verify content-only fixture terms, pending/failed indexing and lexical-only
   readiness on synthetic real SQLite. Include provider documents whose metadata
   is known but bodies have not been fetched. Never show them as searchable.
6. Run one coherent affected frontend/source review and regression checkpoint,
   then inspect the packaged iOS and Android screens separately. Current fixes
   cannot be called visually accepted from unit tests or a web preview.

This document records review findings and implementation order. It is not a
completed redesign, successful indexing audit, iOS release or permission to
discard preserved migration data.
