import { useId } from 'react';
import { AddSourceMenu, AddSourcePanel } from '../features/settings/AddSourcePanel.js';
import { ProviderSearchSelection } from '../features/providers/ProviderSearchSelection';
import { DriveFolderBrowser } from '../features/providers/DriveFolderBrowser';
import { SourceDocuments } from '../features/providers/SourceDocuments';
import { SourceRow } from '../features/providers/SourceRow';
import { useNativeProviders, type NativeProvidersController } from '../features/providers/useNativeProviders';
import '../features/settings/providers.css';
import { nativeProvidersBackend, providerErrorMessage, type NativeProvidersBackend, type ProviderReaderReference } from '../platform/providers.js';

export interface NativeProvidersPanelProps {
  backend?: NativeProvidersBackend;
  /** False while the containing settings category is hidden; status loads when first shown. */
  active?: boolean;
  /** Platforms without a guarded-write adapter cannot request write permission. */
  allowEditing?: boolean;
  selectedProviderSources?: readonly string[];
  onProviderSelectionChange?: (sources: string[]) => void;
  /** Changing the active library invalidates late replies and open callbacks. */
  profileKey?: string;
  onOpen?: (reference: ProviderReaderReference) => void | Promise<void>;
  onSourcesChanged?: () => void;
}
/** Native custody owns OAuth and account/selection authority. Opening this panel is read-only. */
export function NativeProvidersPanel({ backend = nativeProvidersBackend, profileKey = 'current', active = true, allowEditing = true, selectedProviderSources = [], onProviderSelectionChange, onOpen, onSourcesChanged }: NativeProvidersPanelProps) {
  const titleId = useId();
  const providers = useNativeProviders({ backend, profileKey, active, allowEditing, onOpen, onSourcesChanged });
  const { snapshot, busy, loading } = providers;
  return <section className="settings-section settings-cloud-sources" aria-labelledby={titleId}>
    <header className="provider-panel__header">
      <div className="settings-section__intro"><h2 id={titleId}>Cloud sources</h2><p>Read Markdown from Google Drive or Dropbox. Device sync is in Account &amp; sync.</p></div>
      <div className="provider-panel__header-actions">
        <button type="button" className="provider-button--quiet" disabled={busy || loading} onClick={providers.refreshStatus}>Refresh status</button>
        <AddSourceMenu snapshot={snapshot} canConnect={providers.canConnect} onChoose={providers.setAdding} />
      </div>
    </header>
    <Messages providers={providers} />
    {snapshot && providers.adding ? <AddSourcePanel key={providers.adding} kind={providers.adding} controller={providers} allowEditing={allowEditing} /> : null}
    {snapshot?.pendingAdmissions.length ? <section className="provider-pending" aria-label="Pending connections"><h3>Pending connections</h3>
      {snapshot.pendingAdmissions.map((attempt) => <div key={attempt.attemptId} className="provider-pending__item">
        <p>{attempt.name} — {attempt.phase === 'vault_pending' ? 'Credential custody outcome pending' : 'Source admission outcome pending'}</p>
        <button type="button" disabled={busy} onClick={() => providers.reconcile(attempt.attemptId)}>Reconcile connection</button>
      </div>)}</section> : null}
    {snapshot && !snapshot.error && !snapshot.sources.length && !providers.adding ? <p className="provider-panel__empty">No cloud sources yet. Choose Add source to connect Google Drive or Dropbox.</p> : null}
    {snapshot?.sources.length ? <ul className="provider-source-list">{snapshot.sources.map((source) => <SourceRow key={source.sourceId}
      source={source} allowEditing={allowEditing} busy={busy} canReconnect={providers.canConnect(source.kind)} folderOpen={providers.folderSource === source.sourceId}
      actions={{
        onBrowse: () => providers.browse(source), onRefresh: () => providers.refresh(source), onReconnect: () => providers.reconnect(source),
        onPause: () => providers.pause(source), onForget: () => providers.forget(source),
        onChooseFolder: providers.canChooseDriveFolder(source) ? () => providers.setFolderSource(value => value === source.sourceId ? null : source.sourceId) : undefined,
      }}
      search={onProviderSelectionChange ? <ProviderSearchSelection source={source} selected={selectedProviderSources} onChange={onProviderSelectionChange} onError={providers.setError} /> : null}>
      {providers.folderSource === source.sourceId && providers.canChooseDriveFolder(source) ? <DriveFolderBrowser key={`${profileKey}:${source.sourceId}`} sourceId={source.sourceId} backend={backend} busy={busy}
        recursive={providers.form.driveRecursive} allowEditing={allowEditing && providers.form.driveEditing} onChoose={providers.connectDriveFolder} /> : null}
      {providers.current?.sourceId === source.sourceId ? <SourceDocuments source={source} page={providers.page} loading={providers.documentsLoading} busy={busy} canOpen={!!onOpen}
        onOpen={(document, cached) => providers.open(source, document, cached)} onMore={providers.more} onClose={providers.closeDocuments} /> : null}
    </SourceRow>)}</ul> : null}
  </section>;
}

/** Status, errors and notices share one live area above the sources. */
function Messages({ providers }: { providers: NativeProvidersController }) {
  const { loading, error, connectionDiagnostic: diagnostic, snapshot, notice } = providers;
  if (!loading && !error && !snapshot?.error && !notice) return null;
  return <div className="provider-panel__messages">
    {loading ? <p role="status">Loading cloud sources…</p> : null}
    {error ? <p role="alert" className="provider-message provider-message--error">{error}</p> : null}
    {error && diagnostic ? <details className="provider-message__details"><summary>Connection details</summary>
      <p>Step: {diagnostic.phase.replaceAll('_', ' ')}{diagnostic.responseStatus !== null ? ` · HTTP ${diagnostic.responseStatus}` : ''}{diagnostic.rejection !== null ? ` · ${diagnostic.rejection.replaceAll('_', ' ')}` : ''}</p>
    </details> : null}
    {snapshot?.error ? <p role="alert" className="provider-message provider-message--error">{providerErrorMessage(snapshot.error)}</p> : null}
    {notice ? <p role="status" className="provider-message">{notice}</p> : null}
  </div>;
}
