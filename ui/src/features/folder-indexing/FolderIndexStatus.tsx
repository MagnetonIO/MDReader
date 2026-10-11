import type { FolderIndexStatus as Status } from '../../platform/folder-indexing';

export function FolderIndexStatus({ status }: { status: Status | undefined }) {
  if (!status) return <span>Indexing status unavailable</span>;
  const label = !status.monitorEnabled ? 'Paused' : {
    paused: 'Paused', discovering: 'Finding Markdown files…', indexing: 'Indexing…', ready: 'Search ready',
    error: 'Indexing needs attention', unavailable: 'Folder unavailable',
  }[status.phase];
  return <>
    <span>{label}</span>
    {status.observedAtMs !== null ? <span>{status.indexedDocuments.toLocaleString()} indexed · {status.knownDocuments.toLocaleString()} {status.complete ? 'Markdown files' : 'found so far'}</span> : null}
    {status.monitorEnabled && status.phase !== 'unavailable' ? <span>Automatic updates on</span> : null}
  </>;
}
