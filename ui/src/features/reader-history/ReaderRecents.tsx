import { useReaderHistoryStore } from './store';
import { presentation } from './references';
import { referenceKey } from './model';
import type { CanonicalReaderReference } from '../reader/contracts';
import { documentKey, type DocumentRef } from '../../stores/types';
import './reader-history.css';
const NO_WORKSPACE_RECENTS: readonly DocumentRef[] = [];
export function ReaderRecents({ onOpen, workspaceRecent = NO_WORKSPACE_RECENTS, onOpenWorkspace }: {
  onOpen(reference: CanonicalReaderReference): void;
  /** Already scoped by the host to its active folder grant; never canonicalized. */
  workspaceRecent?: readonly DocumentRef[];
  onOpenWorkspace?: (reference: DocumentRef) => void;
}) {
  const { data, diagnostic, error } = useReaderHistoryStore();
  const workspaceRows = onOpenWorkspace ? workspaceRecent : NO_WORKSPACE_RECENTS;
  const list = useRef<HTMLUListElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const nextFocus = useRef<number | null>(null);
  useLayoutEffect(() => {
    if (nextFocus.current === null) return;
    const buttons = list.current?.querySelectorAll<HTMLButtonElement>('.reader-recents__open');
    const index = nextFocus.current;
    nextFocus.current = null;
    (buttons?.[Math.min(index, buttons.length - 1)] ?? heading.current)?.focus();
  }, [data.recent, workspaceRows]);
  return <section className="recent reader-recents" aria-label="Recent reads">
    {diagnostic ? <p role="status">{diagnostic}</p> : null}
    {error ? <p role="alert">{error}</p> : null}
    <h3 ref={heading} className="recent__heading" tabIndex={-1}>Recent</h3>
    {workspaceRows.length || data.recent.length ? <ul ref={list} aria-label="Recent documents">
      {workspaceRows.map(reference => <li key={`workspace:${documentKey(reference)}`}>
        <button className="recent__item reader-recents__open" type="button" title={reference.name}
          onClick={() => onOpenWorkspace?.(reference)}>{reference.name}</button>
      </li>)}
      {data.recent.map((entry, index) => {
        const path = entry.reference.kind === 'device' ? '' : entry.reference.path;
        const title = entry.title || path.split(/[\\/]/).pop() || 'Untitled document';
        const fullTitle = path && path !== title ? `${title} — ${path}` : title;
        return <li key={`receipt:${referenceKey(entry.reference)}`}>
          <button className="recent__item reader-recents__open" type="button" title={fullTitle}
            onClick={() => onOpen(presentation(entry))}>{title}</button>
          <button className="reader-recents__remove" type="button"
            aria-label={`Remove ${fullTitle} from recent documents`} title={`Remove ${fullTitle} from recents`}
            onClick={() => {
              nextFocus.current = workspaceRows.length + index;
              useReaderHistoryStore.getState().removeRecent(entry.reference);
            }}><CloseIcon width="14" height="14" /></button>
        </li>;
      })}
    </ul> : <p className="reader-recents__empty">No recent documents.</p>}
  </section>;
}
import { useLayoutEffect, useRef } from 'react';
import { CloseIcon } from '../../shell/icons';
