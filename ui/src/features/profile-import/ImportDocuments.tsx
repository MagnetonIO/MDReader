import { useId, useMemo, useState } from 'react';
import type { ImportDocument } from './backend';

/** Selection is explicit and remains tied to the native preview's exact revisions. */
export function ImportDocuments({ documents, selected, onSelection, disabled }: {
  documents: readonly ImportDocument[];
  selected: ReadonlySet<string>;
  onSelection: (selection: Set<string>) => void;
  disabled: boolean;
}) {
  const [query, setQuery] = useState('');
  const filterId = useId();
  const visible = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    return documents.filter((item) => item.name.toLocaleLowerCase().includes(needle));
  }, [documents, query]);
  const selectedBytes = documents.reduce((sum, item) => sum + (selected.has(item.documentId) ? item.bytes : 0), 0);
  function toggle(documentId: string, checked: boolean) {
    const next = new Set(selected);
    if (checked) next.add(documentId); else next.delete(documentId);
    onSelection(next);
  }
  function selectDocuments(items: readonly ImportDocument[]) {
    const next = new Set(selected);
    for (const item of items) next.add(item.documentId);
    onSelection(next);
  }
  return <div className="profile-import__selection">
    {documents.length > 8 ? <label className="profile-import__filter" htmlFor={filterId}>
      Find a document
      <input id={filterId} type="search" value={query} disabled={disabled}
        onChange={(event) => setQuery(event.currentTarget.value)} placeholder="Search document names" />
    </label> : null}
    {documents.length ? <div className="profile-import__selection-actions">
      <button type="button" className="settings-link-button" disabled={disabled || documents.every((item) => selected.has(item.documentId))}
        onClick={() => selectDocuments(documents)}>Select all documents</button>
      {query.trim() ? <button type="button" className="settings-link-button" disabled={disabled || !visible.length || visible.every((item) => selected.has(item.documentId))}
        onClick={() => selectDocuments(visible)}>Select matching documents</button> : null}
      <button type="button" className="settings-link-button" disabled={disabled || !selected.size}
        onClick={() => onSelection(new Set())}>Clear selection</button>
    </div> : null}
    <div className="profile-import__documents" role="group" aria-label="Documents to import">
      {visible.map((item) => <label key={item.documentId}>
        <input type="checkbox" checked={selected.has(item.documentId)} disabled={disabled}
          onChange={(event) => toggle(item.documentId, event.currentTarget.checked)} />
        <span><strong>{item.name}</strong><small>{formatBytes(item.bytes)}{item.pinned ? ' · Pinned' : ''}{item.line > 1 ? ` · Line ${item.line}` : ''}</small></span>
      </label>)}
      {!documents.length ? <p>No readable documents were found. You can still preserve the original records with Export originals.</p>
        : !visible.length ? <p>No document names match your search. Your selection is unchanged.</p> : null}
    </div>
    <p className="profile-import__selection-count" role="status">{selected.size.toLocaleString()} of {documents.length.toLocaleString()} documents selected{selected.size ? ` · ${formatBytes(selectedBytes)}` : ''}</p>
  </div>;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes.toLocaleString()} bytes`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toLocaleString(undefined, { maximumFractionDigits: 1 })} KB`;
  return `${(bytes / (1024 * 1024)).toLocaleString(undefined, { maximumFractionDigits: 1 })} MB`;
}
