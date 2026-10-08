/** Migration is explicit and offline. This panel cannot reconnect old SDK
 * accounts, enumerate private profiles, activate an archive or delete originals. */
export function MigrationSettings() {
  return <details className="settings-migration" open>
    <summary>Migration &amp; archived records</summary>
    <p>Your original profile, old session references and connection metadata are retained. The retired database client is no longer used to browse or edit them.</p>
    <h3>Preserve an offline snapshot</h3>
    <ol>
      <li>Close the old reader and prepare consistent copies of the files you explicitly choose. Do not select credentials or a live database with journal sidecars.</li>
      <li>Use the standalone <code>mdreader-legacy-export</code> tool with an explicit snapshot specification and a new private destination.</li>
      <li>Verify the completed archive before beginning migration. Keep the old profile and archive for rollback.</li>
    </ol>
    <pre aria-label="Offline archive commands"><code>{'mdreader-legacy-export export ABS_SPEC_JSON ABS_NEW_ARCHIVE\nmdreader-legacy-export verify ABS_NEW_ARCHIVE'}</code></pre>
    <p>The specification selects exact native file locators and asserts that the snapshots are offline and consistent. Nothing is discovered or uploaded by this panel. The tool must be available separately.</p>
    <h3>Read archived reviews</h3>
    <p>Stage reviews from a verified archive with the standalone tool. Then open a current local or selected-provider document and choose Archived reviews to select that exact stage and compare its recorded passage.</p>
    <p>Unknown records and unsupported schemas remain in the raw archive. A historical reference is not a grant to its original source, and comparison never replaces the current document.</p>
    <h3>Migration and rollback</h3>
    <p>Automatic installed-profile migration and rollback activation are not available here. Keep the original profile intact. A new Library uses only folders and provider selections you explicitly authorize.</p>
  </details>;
}
