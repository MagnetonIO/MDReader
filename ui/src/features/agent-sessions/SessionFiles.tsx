import type { CanonicalReaderReference } from '../reader/contracts';
import type { AgentActivityFile } from './contracts';
import { fileName, fileReference } from './presentation';
import { useTreeItem } from './tree';
import type { SessionFilesState } from './useAgentActivity';

function FileRow({ sessionId, file, onOpen, onError }: {
  sessionId: string; file: AgentActivityFile; onOpen(reference: CanonicalReaderReference): void; onError(message: string): void;
}) {
  const item = useTreeItem(`file:${sessionId}:${file.absPath}`);
  const name = fileName(file), folder = file.displayPath.slice(0, file.displayPath.length - name.length);
  const openable = file.exists && !!file.reference;
  const tag = !file.exists ? 'deleted' : file.isPlan ? 'plan' : file.op === 'create' ? 'created' : 'edited';
  const hint = !file.exists ? 'This file no longer exists.' : !file.reference ? file.reason || 'This file is not available to MDReader.' : null;
  function open() {
    if (!openable) return;
    try {
      const reference = fileReference(file);
      if (reference) onOpen(reference);
    } catch { onError('This file changed. Expand the session again to reopen it.'); }
  }
  return <li role="treeitem" aria-level={3} aria-disabled={!openable || undefined} aria-label={`${file.displayPath}, ${tag}`} {...item}
    className={`agent-sessions__item${openable ? '' : ' agent-sessions__item--unavailable'}`}>
    <div data-tree-row className="agent-sessions__row agent-sessions__file" title={hint ? `${file.absPath}\n${hint}` : file.absPath} onClick={open}>
      <span className="agent-sessions__path"><span className="agent-sessions__folder">{folder}</span><span className="agent-sessions__name">{name}</span></span>
      <span className={`agent-sessions__tag agent-sessions__tag--${tag}`}>{tag}</span>
    </div>
  </li>;
}

export function SessionFiles({ sessionId, state, onOpen, onRetry, onError }: {
  sessionId: string; state: SessionFilesState | undefined;
  onOpen(reference: CanonicalReaderReference): void; onRetry(): void; onError(message: string): void;
}) {
  const files = state?.files;
  return <ul role="group" className="agent-sessions__group agent-sessions__files">
    {files?.map(file => <FileRow key={file.absPath} sessionId={sessionId} file={file} onOpen={onOpen} onError={onError} />)}
    {!files && state?.loading !== false ? <li className="agent-sessions__note" role="status">Loading files…</li> : null}
    {files && !files.length ? <li className="agent-sessions__note">No Markdown files recorded.</li> : null}
    {state?.failed ? <li className="agent-sessions__note" role="alert">Files could not be loaded. <button type="button" onClick={onRetry}>Retry</button></li> : null}
  </ul>;
}
