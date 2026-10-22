import type { CanonicalReaderReference } from '../reader/contracts';
import type { AgentActivityFile } from './contracts';
import { fileName, fileReference } from './presentation';
import { useTreeItem } from './tree';
import type { SessionFilesState } from './useAgentActivity';

const UNAVAILABLE: Record<string, string> = {
  missing: 'This file no longer exists.',
  private: 'This file is in a private folder, so MDReader lists it without opening it.',
  not_markdown: 'This file is not Markdown.',
  too_large: 'This file is too large to open in MDReader.',
  unreadable: 'MDReader could not read this file.',
};

/** The parent folder name, shown only to tell apart files that share a name. */
const parentFolder = (file: AgentActivityFile) => file.absPath.split('/').slice(-2, -1)[0] || null;

function FileRow({ sessionId, file, ambiguous, onOpen, onError }: {
  sessionId: string; file: AgentActivityFile; ambiguous: boolean; onOpen(reference: CanonicalReaderReference): void; onError(message: string): void;
}) {
  const item = useTreeItem(`file:${sessionId}:${file.absPath}`);
  const name = fileName(file), folder = ambiguous ? parentFolder(file) : null;
  const openable = file.exists && !!file.reference;
  const tag = !file.exists ? 'deleted' : file.isPlan ? 'plan' : null;
  const hint = !file.exists ? UNAVAILABLE.missing! : !file.reference ? UNAVAILABLE[file.reason ?? ''] ?? 'This file is not available to MDReader.' : null;
  function open() {
    if (!openable) return;
    try {
      const reference = fileReference(file);
      if (reference) onOpen(reference);
    } catch { onError('This file changed. Expand the session again to reopen it.'); }
  }
  return <li role="treeitem" aria-level={3} aria-disabled={!openable || undefined} aria-label={[name, folder && `in ${folder}`, tag].filter(Boolean).join(', ')} {...item}
    className={`agent-sessions__item${openable ? '' : ' agent-sessions__item--unavailable'}`}>
    <div data-tree-row className="agent-sessions__row agent-sessions__file" title={hint ? `${file.absPath}\n${hint}` : file.absPath} onClick={open}>
      <span className="agent-sessions__path"><span className="agent-sessions__name">{name}</span>{folder ? <span className="agent-sessions__folder">{folder}/</span> : null}</span>
      {tag ? <span className={`agent-sessions__tag agent-sessions__tag--${tag}`}>{tag}</span> : null}
    </div>
  </li>;
}

export function SessionFiles({ sessionId, state, onOpen, onRetry, onError }: {
  sessionId: string; state: SessionFilesState | undefined;
  onOpen(reference: CanonicalReaderReference): void; onRetry(): void; onError(message: string): void;
}) {
  const files = state?.files;
  const names = new Map<string, number>();
  for (const file of files ?? []) names.set(fileName(file), (names.get(fileName(file)) ?? 0) + 1);
  return <ul role="group" className="agent-sessions__group agent-sessions__files">
    {files?.map(file => <FileRow key={file.absPath} sessionId={sessionId} file={file} ambiguous={(names.get(fileName(file)) ?? 0) > 1} onOpen={onOpen} onError={onError} />)}
    {!files && state?.loading !== false ? <li className="agent-sessions__note" role="status">Loading files…</li> : null}
    {files && !files.length ? <li className="agent-sessions__note">No Markdown files recorded.</li> : null}
    {state?.failed ? <li className="agent-sessions__note" role="status">Files could not be loaded. <button type="button" onClick={onRetry}>Retry</button></li> : null}
  </ul>;
}
