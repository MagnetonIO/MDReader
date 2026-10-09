import type { CanonicalReaderReference } from '../reader/contracts';
import { ExistingDocumentPicker } from '../profile-import/ExistingDocumentPicker';
import { useImportedHistory } from './useImportedHistory';
import type { ImportedHistoryBackend } from './backend';
import './imported-history.css';

/** Historical references remain metadata until an explicit native-verified match. */
export function ImportedHistory({ onOpen, backend }: {
  onOpen(reference: CanonicalReaderReference): void;
  backend?: ImportedHistoryBackend;
}) {
  const history = useImportedHistory(backend);
  if (!history.snapshot?.entries.length && !history.error) return null;
  return <section className="imported-history" aria-label="Imported reading history" aria-busy={history.busy}>
    <header><h3>Imported history</h3><button type="button" disabled={history.busy} onClick={() => void history.refresh()}>Refresh</button></header>
    <p>Your older references are kept. Matching a copy does not restore folder or account access.</p>
    <ul>{history.snapshot?.entries.map((entry) => <li key={entry.entryId}>
      <strong>{entry.title || 'Historical document'}</strong>
      {entry.archivedPosition ? <small>Original reading position preserved</small> : null}
      {entry.reference ? <button type="button" disabled={history.busy || history.stale} onClick={() => onOpen(entry.reference!)}>Open matched copy</button>
        : <button type="button" disabled={history.busy || history.stale} onClick={() => history.match(entry)}>Match to a saved copy…</button>}
    </li>)}</ul>
    {history.snapshot?.nextAfterEntryId ? <button type="button" disabled={history.busy || history.stale || history.snapshot.entries.length >= 1000}
      onClick={() => void history.more()}>More history</button> : null}
    {history.error ? <p role="alert">{history.error}</p> : null}
    {history.matching ? <ExistingDocumentPicker key={history.matching.entryId} title={history.matching.title}
      onChoose={history.bind} onClose={history.closeMatch} /> : null}
  </section>;
}
