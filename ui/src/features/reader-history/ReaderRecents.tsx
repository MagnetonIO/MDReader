import { useReaderHistoryStore } from './store';
import { presentation } from './references';
import { referenceKey } from './model';
import type { CanonicalReaderReference } from '../reader/contracts';
import './reader-history.css';
export function ReaderRecents({ onOpen }: { onOpen(reference: CanonicalReaderReference): void }) {
  const { data, diagnostic, error } = useReaderHistoryStore();
  if (!data.recent.length && !diagnostic && !error) return null;
  return <section className="recent reader-recents" aria-label="Recent exact reads">
    {diagnostic ? <p role="status">{diagnostic}</p> : null}
    {error ? <p role="alert">{error}</p> : null}
    {data.recent.length ? <>
      <h3 className="recent__heading">Recent</h3>
      <ul aria-label="Recent documents">
        {data.recent.slice(0, 6).map((entry) => <li key={referenceKey(entry.reference)}>
          <button className="recent__item" type="button" onClick={() => onOpen(presentation(entry))}>
            {entry.title || 'Untitled document'}
          </button>
        </li>)}
      </ul>
      <details className="reader-recents__options">
        <summary>{data.recent.length > 6 ? 'All recent reads…' : 'Manage recent reads…'}</summary>
        <ul>{data.recent.map((entry) => <li key={referenceKey(entry.reference)}>
          <button className="recent__item" type="button" onClick={() => onOpen(presentation(entry))}>
            {entry.title || 'Untitled document'}
          </button>
          <button type="button" aria-label={`Remove recent ${entry.title}`}
            onClick={() => useReaderHistoryStore.getState().removeRecent(entry.reference)}>Remove</button>
        </li>)}</ul>
      </details>
    </> : null}
  </section>;
}
