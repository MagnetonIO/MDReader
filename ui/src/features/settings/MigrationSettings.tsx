import { DropboxIcon, GoogleDriveIcon } from '../../shell/icons.js';
import type { SettingsController } from './useSettingsController.js';

export function MigrationSettings({ controller }: { controller: SettingsController }) {
  const {
    cloudControls,
    cloudSources,
    failure,
    busy,
    connectCloud,
    setCloudEnabled,
    disconnectCloud,
    connectedCatdb,
    connectedCatdbControls,
    library,
    connectedCatdbOrigin,
    connectedCatdbBearer,
    setConnectedCatdbOrigin,
    setConnectedCatdbBearer,
    enrollCatdb,
    disconnectCatdb,
    setCatdbSourceEnabled,
  } = controller;
  return <details className="settings-migration">
    <summary>Migration / legacy connections</summary>
    <p>
      These are existing legacy connections. Use Account &amp; sync for selected cloud sources.
      Legacy records remain available through the Library migration entry.
    </p>
    {cloudControls ? (
    <div className="cloud-sources">
      <div className="cloud-sources__heading">
        <div>
          <h3>Connected accounts</h3>
          <p>
            Sign in through MDReader’s registered apps. Credentials stay in the
            operating system’s secure storage.
          </p>
        </div>
      </div>
      <div className="cloud-sources__list" aria-live="polite">
        {!cloudSources && !failure ? (
          <div className="settings-empty">Reading connected accounts…</div>
        ) : null}
        {cloudSources?.sources.map((source) => {
          const connected = source.connection !== 'disconnected';
          const reconnect = source.connection === 'reauthorize';
          return (
            <article
              className="cloud-source cloud-source--provider"
              key={source.provider}
            >
              <div className="cloud-source__icon">
                {source.provider === 'google-drive' ? (
                  <GoogleDriveIcon />
                ) : (
                  <DropboxIcon />
                )}
              </div>
              <div className="cloud-source__copy">
                <div className="cloud-source__title">
                  <h4>{source.label}</h4>
                  {connected ? (
                    <span className={reconnect ? 'settings-state--warning' : ''}>
                      {reconnect ? 'Sign in again' : source.enabled ? 'Included' : 'Paused'}
                    </span>
                  ) : null}
                </div>
                <p>
                  {source.provider === 'google-drive'
                    ? 'Choose the Drive folders or Markdown files that belong in this Library.'
                    : 'Read Markdown files from your Dropbox account.'}
                </p>
              </div>
              <div className="cloud-source__actions">
                {connected ? (
                  <>
                    {!reconnect ? (
                      <button
                        type="button"
                        className="source-switch"
                        role="switch"
                        aria-label={`Include ${source.label} in Library`}
                        aria-checked={source.enabled}
                        disabled={busy !== null}
                        onClick={() =>
                          setCloudEnabled(source.provider, !source.enabled)
                        }
                      >
                        <span aria-hidden="true" />
                      </button>
                    ) : (
                      <button
                        type="button"
                        disabled={busy !== null}
                        aria-label={`Reconnect ${source.label}`}
                        onClick={() => connectCloud(source.provider)}
                      >
                        Reconnect
                      </button>
                    )}
                    <button
                      type="button"
                      className="settings-button--quiet"
                      disabled={busy !== null}
                      aria-label={`Disconnect ${source.label}`}
                      onClick={() => disconnectCloud(source.provider)}
                    >
                      {busy === `disconnect:${source.provider}`
                        ? 'Disconnecting…'
                        : 'Disconnect'}
                    </button>
                  </>
                ) : (
                  <button
                    type="button"
                    disabled={busy !== null}
                    aria-label={`Connect ${source.label}`}
                    onClick={() => connectCloud(source.provider)}
                  >
                    {busy === `connect:${source.provider}` ? 'Connecting…' : 'Connect'}
                  </button>
                )}
              </div>
            </article>
          );
        })}
      </div>
      <p className="settings-footnote">
        Pausing keeps the account connected. Disconnecting removes its saved credentials.
        Neither action changes files in Drive or Dropbox.
      </p>
    </div>
  ) : null}
    {connectedCatdb && connectedCatdbControls ? (
      <div className="cloud-sources">
        <div className="cloud-sources__heading">
          <div>
            <h3>Connected CatDB</h3>
            <p>
              Sign in to a CatDB workspace to choose the database and warehouse sources
              your account can read.
            </p>
          </div>
        </div>
        {library?.connected_catdb_rediscovery_required ? (
          <p className="settings-state--warning" role="status">
            This Library’s previous CatDB connection did not record a verified schema
            version. Connect it again to rediscover and pin the current schema before
            querying.
          </p>
        ) : null}
        {connectedCatdb.state === 'disconnected' ? (
          <article className="cloud-source connected-catdb-card">
            <div className="connected-catdb-form">
              <label>
                <span>CatDB server URL</span>
                <input
                  type="url"
                  inputMode="url"
                  autoComplete="url"
                  value={connectedCatdbOrigin}
                  placeholder="https://catdb.example.com"
                  disabled={busy !== null}
                  onChange={(event) => setConnectedCatdbOrigin(event.currentTarget.value)}
                />
              </label>
              <label>
                <span>CatDB bearer token</span>
                <input
                  type="password"
                  autoComplete="off"
                  value={connectedCatdbBearer}
                  disabled={busy !== null}
                  onChange={(event) => setConnectedCatdbBearer(event.currentTarget.value)}
                />
              </label>
              <p>
                Direct database credentials are never stored in MDReader. The bearer is
                written once to the operating system credential store and cleared here.
              </p>
            </div>
            <div className="cloud-source__actions">
              <button
                type="button"
                disabled={
                  busy !== null ||
                  connectedCatdbOrigin.trim().length === 0 ||
                  connectedCatdbBearer.length === 0
                }
                aria-label="Connect CatDB workspace"
                onClick={enrollCatdb}
              >
                {busy === 'connected-catdb:enroll' ? 'Connecting…' : 'Connect'}
              </button>
            </div>
          </article>
        ) : (
          <>
            <article className="cloud-source">
              <div className="cloud-source__copy">
                <div className="cloud-source__title">
                  <h4>{connectedCatdb.label}</h4>
                  <span>Connected</span>
                </div>
                <p>
                  Workspace {connectedCatdb.workspace_id} · catalog{' '}
                  {connectedCatdb.catalog_revision_id}
                </p>
              </div>
              <div className="cloud-source__actions">
                <button
                  type="button"
                  className="settings-button--quiet"
                  disabled={busy !== null}
                  aria-label={`Disconnect ${connectedCatdb.label}`}
                  onClick={() => disconnectCatdb(connectedCatdb.connection_id)}
                >
                  {busy === 'connected-catdb:disconnect'
                    ? 'Disconnecting…'
                    : 'Disconnect'}
                </button>
              </div>
            </article>
            <div className="cloud-sources__list" aria-live="polite">
              {connectedCatdb.sources.length === 0 ? (
                <div className="settings-empty">
                  This workspace exposes no permitted sources.
                </div>
              ) : null}
              {connectedCatdb.sources.map((source) => {
                const label = source.connector_id ?? source.source_id;
                return (
                  <article className="cloud-source" key={source.source_id}>
                    <div className="cloud-source__copy">
                      <div className="cloud-source__title">
                        <h4>{label}</h4>
                        {!source.runtime_bound ? (
                          <span className="settings-state--warning">Unavailable</span>
                        ) : source.enabled ? (
                          <span>Included</span>
                        ) : (
                          <span>Paused</span>
                        )}
                      </div>
                      <p>Source {source.source_id}</p>
                    </div>
                    <div className="cloud-source__actions">
                      <button
                        type="button"
                        className="source-switch"
                        role="switch"
                        aria-label={`Include ${label} in Connected CatDB queries`}
                        aria-checked={source.enabled}
                        disabled={!source.runtime_bound || busy !== null}
                        onClick={() =>
                          setCatdbSourceEnabled(
                            connectedCatdb.connection_id,
                            source.source_id,
                            !source.enabled,
                          )
                        }
                      >
                        <span aria-hidden="true" />
                      </button>
                    </div>
                  </article>
                );
              })}
            </div>
            <p className="settings-footnote">
              Queries are pinned natively to this workspace, catalog revision, and the
              enabled sources above. Direct database credentials are never stored in
              MDReader; they remain server-managed.
            </p>
          </>
        )}
      </div>
    ) : null}
  </details>;
}
