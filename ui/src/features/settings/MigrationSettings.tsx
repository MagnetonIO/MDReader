import { DropboxIcon, GoogleDriveIcon } from '../../shell/icons.js';
import type { SettingsController } from './useSettingsController.js';

/** Retained migration inputs only. New enrollment belongs to native selected
 * providers and encrypted device sync, never the retired generic client. */
export function MigrationSettings({ controller }: { controller: SettingsController }) {
  const { cloudControls, cloudSources, failure, busy, setCloudEnabled, disconnectCloud,
    connectedCatdb, connectedCatdbControls, library, disconnectCatdb, setCatdbSourceEnabled } = controller;
  const retainedCloud = cloudSources?.sources.filter((source) => source.connection !== 'disconnected');
  return <details className="settings-migration" onToggle={(event) => {
    if (event.currentTarget.open) controller.openLegacy();
  }}>
    <summary>Migration / legacy connections</summary>
    <p>Existing records and connections are retained for migration. Add new selected cloud sources in Account &amp; sync.</p>
    <p>Keep the old profile until its document bodies, history and edit receipts have been preserved. Opening this section does not migrate or reconnect it.</p>
    {cloudControls ? <div className="cloud-sources">
      <h3>Retained cloud connections</h3>
      {!cloudSources && !failure ? <p role="status">Reading retained connections…</p> : null}
      {cloudSources && !retainedCloud?.length ? <p>No retained cloud connection.</p> : null}
      <div className="cloud-sources__list" aria-live="polite">
        {retainedCloud?.map((source) => <article className="cloud-source cloud-source--provider" key={source.provider}>
          <div className="cloud-source__icon">
            {source.provider === 'google-drive' ? <GoogleDriveIcon /> : <DropboxIcon />}
          </div>
          <div className="cloud-source__copy">
            <div className="cloud-source__title">
              <h4>{source.label}</h4>
              <span>{source.connection === 'reauthorize' ? 'Authorization unavailable' : source.enabled ? 'Included' : 'Paused'}</span>
            </div>
            <p>{source.connection === 'reauthorize'
              ? 'This legacy connection cannot be renewed here. Use a new selected source in Account & sync; retained records have not been deleted.'
              : 'Available through the explicit Legacy records entry in the Library.'}</p>
          </div>
          <div className="cloud-source__actions">
            {source.connection !== 'reauthorize' ? <button type="button" className="source-switch" role="switch"
              aria-label={`Include ${source.label} in Library`} aria-checked={source.enabled} disabled={busy !== null}
              onClick={() => setCloudEnabled(source.provider, !source.enabled)}><span aria-hidden="true" /></button> : null}
            <button type="button" className="settings-button--quiet" disabled={busy !== null}
              aria-label={`Disconnect ${source.label}`} onClick={() => disconnectCloud(source.provider)}>
              {busy === `disconnect:${source.provider}` ? 'Disconnecting…' : 'Disconnect'}
            </button>
          </div>
        </article>)}
      </div>
      {retainedCloud?.length ? <p className="settings-footnote">Pausing retains credentials. Disconnecting removes this connection’s saved credentials; provider originals are unchanged.</p> : null}
    </div> : null}
    {connectedCatdb && connectedCatdbControls ? <div className="cloud-sources">
      <h3>Retained legacy workspace</h3>
      {library?.connected_catdb_rediscovery_required ? <p className="settings-state--warning" role="status">
        This connection lacks a verified schema version. It remains unavailable for current queries; nothing has been reconnected automatically.
      </p> : null}
      {connectedCatdb.state === 'disconnected' ? <p>No retained workspace connection. New generic database connections are retired.</p> : <>
        <article className="cloud-source">
          <div className="cloud-source__copy">
            <div className="cloud-source__title"><h4>{connectedCatdb.label}</h4><span>Retained</span></div>
            <p>Workspace {connectedCatdb.workspace_id} · catalog {connectedCatdb.catalog_revision_id}</p>
          </div>
          <div className="cloud-source__actions">
            <button type="button" className="settings-button--quiet" disabled={busy !== null}
              aria-label={`Disconnect ${connectedCatdb.label}`} onClick={() => disconnectCatdb(connectedCatdb.connection_id)}>
              {busy === 'connected-catdb:disconnect' ? 'Disconnecting…' : 'Disconnect'}
            </button>
          </div>
        </article>
        <div className="cloud-sources__list" aria-live="polite">
          {!connectedCatdb.sources.length ? <p>No retained permitted sources.</p> : null}
          {connectedCatdb.sources.map((source) => {
            const label = source.connector_id ?? source.source_id;
            return <article className="cloud-source" key={source.source_id}>
              <div className="cloud-source__copy">
                <div className="cloud-source__title"><h4>{label}</h4>
                  <span>{!source.runtime_bound ? 'Unavailable' : source.enabled ? 'Included' : 'Paused'}</span>
                </div>
              </div>
              <div className="cloud-source__actions">
                <button type="button" className="source-switch" role="switch"
                  aria-label={`Include ${label} in Connected CatDB queries`} aria-checked={source.enabled}
                  disabled={!source.runtime_bound || busy !== null}
                  onClick={() => setCatdbSourceEnabled(connectedCatdb.connection_id, source.source_id, !source.enabled)}>
                  <span aria-hidden="true" />
                </button>
              </div>
            </article>;
          })}
        </div>
      </>}
    </div> : null}
  </details>;
}
