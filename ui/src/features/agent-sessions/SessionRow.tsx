import { useCallback, useRef, useState, type KeyboardEvent, type MouseEvent } from 'react';
import { ChevronRight } from '../../shell/icons';
import type { CanonicalReaderReference } from '../reader/contracts';
import type { AgentActivitySession } from './contracts';
import { isLive, relativeTime, sessionTooltip } from './presentation';
import { SessionFiles } from './SessionFiles';
import { SessionMenu, type MenuAction } from './SessionMenu';
import { useTreeItem } from './tree';
import type { SessionFilesState } from './useAgentActivity';

const MAX_TITLE = 200;

export function SessionRow({ session, now, expanded, files, onToggle, onOpen, onRetryFiles, onRename, onHide, onReveal, onError }: {
  session: AgentActivitySession; now: number; expanded: boolean; files: SessionFilesState | undefined;
  onToggle(): void; onOpen(reference: CanonicalReaderReference): void; onRetryFiles(): void;
  onRename(title: string): Promise<boolean>; onHide(): void; onReveal?: () => void; onError(message: string): void;
}) {
  const item = useTreeItem(`session:${session.id}`);
  const ref = useRef<HTMLLIElement>(null);
  const [menu, setMenu] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const live = isLive(session, now);
  const title = session.title || 'Untitled session';
  const closeMenu = useCallback(() => { setMenu(false); ref.current?.focus(); }, []);
  const actions: MenuAction[] = [
    { label: 'Rename…', run: () => setRenaming(true) },
    { label: 'Hide session', run: onHide },
    ...(onReveal && session.cwd ? [{ label: 'Reveal folder', run: onReveal }] : []),
  ];
  function onKeyDown(event: KeyboardEvent<HTMLLIElement>) {
    if (event.target !== event.currentTarget) return;
    if (event.key === 'ContextMenu' || event.key === 'F10' && event.shiftKey) { event.preventDefault(); setMenu(true); }
    else if (event.key === 'F2') { event.preventDefault(); setRenaming(true); }
  }
  function onContextMenu(event: MouseEvent) { event.preventDefault(); setMenu(true); }
  async function commit(value: string) {
    const next = value.trim().slice(0, MAX_TITLE);
    setRenaming(false); ref.current?.focus();
    if (next && next !== session.title) await onRename(next);
  }
  return <li ref={ref} role="treeitem" aria-level={2} aria-expanded={expanded} aria-label={`${title}${live ? ', live' : ''}`} {...item}
    className="agent-sessions__item" onKeyDown={onKeyDown}>
    <div data-tree-row className="agent-sessions__row agent-sessions__session" title={sessionTooltip(session)}
      onClick={renaming ? undefined : onToggle} onContextMenu={onContextMenu}>
      <ChevronRight className="agent-sessions__chevron" />
      {renaming ? <RenameField initial={session.title} onCommit={value => void commit(value)} onCancel={() => { setRenaming(false); ref.current?.focus(); }} />
        : <span className="agent-sessions__title">{title}</span>}
      <span className="agent-sessions__meta">
        {live ? <span className="agent-sessions__live"><span aria-hidden="true">● </span>live · </span> : null}
        <time dateTime={new Date(session.lastActivityMs).toISOString()}>{relativeTime(session.lastActivityMs, now)}</time>
      </span>
      <button type="button" tabIndex={-1} className="agent-sessions__more" aria-label={`Actions for ${title}`} aria-haspopup="menu"
        aria-expanded={menu} onClick={event => { event.stopPropagation(); setMenu(value => !value); }}>⋯</button>
    </div>
    {menu ? <SessionMenu label={`Actions for ${title}`} actions={actions} onClose={closeMenu} /> : null}
    {expanded ? <SessionFiles sessionId={session.id} state={files} onOpen={onOpen} onRetry={onRetryFiles} onError={onError} /> : null}
  </li>;
}

function RenameField({ initial, onCommit, onCancel }: { initial: string; onCommit(value: string): void; onCancel(): void }) {
  const done = useRef(false);
  const finish = (action: () => void) => { if (done.current) return; done.current = true; action(); };
  return <input className="agent-sessions__rename" aria-label="Session name" defaultValue={initial} maxLength={MAX_TITLE} autoFocus
    onFocus={event => event.currentTarget.select()} onClick={event => event.stopPropagation()}
    onBlur={event => { const value = event.currentTarget.value; finish(() => onCommit(value)); }}
    onKeyDown={event => {
      event.stopPropagation();
      if (event.key === 'Enter') { const value = event.currentTarget.value; finish(() => onCommit(value)); }
      else if (event.key === 'Escape') finish(onCancel);
    }} />;
}
