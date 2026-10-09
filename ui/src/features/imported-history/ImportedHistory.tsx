import { useState } from 'react';
import type { CanonicalReaderReference } from '../reader/contracts';
import { ExistingDocumentPicker } from '../profile-import/ExistingDocumentPicker';
import { useImportedHistory } from './useImportedHistory';
import type { ImportedHistoryBackend, ImportedHistoryEntry } from './backend';
import './imported-history.css';

const contextLabel = {
  tab: 'Saved tab', tab_history: 'Earlier in a saved tab', recent: 'Recent document',
  source_document: 'Original Markdown reference', unknown: 'Historical reference',
} as const;

/** Historical references remain metadata until an explicit native-verified match. */
export function ImportedHistory({ onOpen, onReconnectFolder, backend }: {
  onOpen(reference: CanonicalReaderReference): void;
  onReconnectFolder?(): Promise<void>;
  backend?: ImportedHistoryBackend;
}) {
  const history = useImportedHistory(backend);
  const [filter, setFilter] = useState('');
  const [reconnecting, setReconnecting] = useState(false);
  const [folderError, setFolderError] = useState<string | null>(null);
  if (!history.snapshot?.entries.length && !history.error) return null;
  const entries = history.snapshot?.entries ?? [];
  const query = filter.trim().toLocaleLowerCase();
  const visible = entries.filter((entry) => entry.title.toLocaleLowerCase().includes(query));
  const originals = visible.filter((entry) => entry.context === 'source_document');
  const reading = visible.filter((entry) => entry.context !== 'source_document');
  const disabled = history.busy || history.stale || reconnecting;
  async function reconnect() {
    if (!onReconnectFolder || reconnecting) return;
    setReconnecting(true); setFolderError(null);
    try { await onReconnectFolder(); }
    catch { setFolderError('The folder connection could not be confirmed. Your references were kept.'); }
    finally { setReconnecting(false); }
  }
  function rows(items: ImportedHistoryEntry[]) {
    return <ul>{items.map((entry) => <li key={entry.entryId}>
      <strong>{entry.title || 'Historical document'}</strong>
      <small>{contextLabel[entry.context]}{entry.archivedPosition ? ' · Old position archived' : ''}</small>
      {entry.reference ? <button type="button" disabled={disabled} onClick={() => onOpen(entry.reference!)}>Open matched document</button>
        : <button type="button" disabled={disabled} onClick={() => history.match(entry)}>Match a document…</button>}
    </li>)}</ul>;
  }
  return <section className="imported-history" aria-label="Imported reading history" aria-busy={history.busy || reconnecting}>
    <header><h3>Imported history</h3><button type="button" disabled={history.busy || reconnecting} onClick={() => void history.refresh()}>Refresh</button></header>
    <p>Original-document references and older reading history are kept separately. Metadata copies do not contain missing original Markdown.</p>
    {onReconnectFolder ? <>
      <button type="button" disabled={disabled} onClick={() => void reconnect()}>{reconnecting ? 'Choosing folder…' : 'Reconnect a folder…'}</button>
      <p>Choose a folder explicitly, then use Match a document to select its current copy. Old folder permissions and reading positions are not restored.</p>
    </> : null}
    {entries.length > 8 ? <label className="imported-history__filter">Find in loaded references
      <input type="search" value={filter} onChange={(event) => setFilter(event.target.value)} />
    </label> : null}
    {originals.length ? <details open className="imported-history__group"><summary>Original documents ({originals.length})</summary>{rows(originals)}</details> : null}
    {reading.length ? <details open className="imported-history__group"><summary>Tabs and recent documents ({reading.length})</summary>{rows(reading)}</details> : null}
    {!visible.length && entries.length ? <p>No loaded references match. {history.snapshot?.nextAfterEntryId ? 'Load more references to continue looking.' : ''}</p> : null}
    {history.snapshot?.nextAfterEntryId ? <button type="button" disabled={disabled || entries.length >= 1000}
      onClick={() => void history.more()}>Load more references</button> : null}
    {history.error || folderError ? <p role="alert">{history.error ?? folderError}</p> : null}
    {history.matching ? <ExistingDocumentPicker key={history.matching.entryId} title={history.matching.title}
      onChoose={history.bind} onClose={history.closeMatch} /> : null}
  </section>;
}
