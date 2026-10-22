import type { ReactNode } from 'react';
import { ChevronRight } from '../../shell/icons';
import { agentName, type AgentKind } from './contracts';
import { useTreeItem } from './tree';
import type { SessionGroup } from './useAgentActivity';

function ShowMore({ agent, busy, more, onMore }: { agent: AgentKind; busy: boolean; more: number | null; onMore(): void }) {
  const item = useTreeItem(`more:${agent}`);
  return <li role="treeitem" aria-level={2} aria-disabled={busy || undefined} className="agent-sessions__item" {...item}>
    <div data-tree-row className="agent-sessions__row agent-sessions__more-row" onClick={busy ? undefined : onMore}>
      {busy ? 'Loading…' : more === null ? 'Show more…' : `Show ${more} more`}
    </div>
  </li>;
}

export function AgentGroup({ agent, group, total, more, hasMore, expanded, tracked, onToggle, onMore, onRetry, children }: {
  agent: AgentKind; group: SessionGroup; total: number | null; more: number | null; hasMore: boolean; expanded: boolean; tracked: boolean;
  onToggle(): void; onMore(): void; onRetry(): void; children: ReactNode;
}) {
  const item = useTreeItem(`agent:${agent}`);
  const name = agentName(agent);
  const count = total === null ? `${group.sessions.length}+` : `${total}`;
  return <li role="treeitem" aria-level={1} aria-expanded={expanded} aria-label={`${name}, ${count} sessions`} className="agent-sessions__item" {...item}>
    <div data-tree-row className="agent-sessions__row agent-sessions__agent" onClick={onToggle}>
      <ChevronRight className="agent-sessions__chevron" />
      <span className="agent-sessions__title">{name}</span>
      <span className="agent-sessions__count">{group.loading && !group.sessions.length ? '…' : count}</span>
    </div>
    {expanded ? <ul role="group" className="agent-sessions__group">
      {children}
      {hasMore ? <ShowMore agent={agent} busy={group.loading} more={more} onMore={onMore} /> : null}
      {!group.loading && !group.failed && !group.sessions.length
        ? <li className="agent-sessions__note">{tracked ? 'No sessions with Markdown yet.' : 'Tracking is off.'}</li> : null}
      {group.failed ? <li className="agent-sessions__note" role="alert">Sessions could not be loaded. <button type="button" onClick={onRetry}>Retry</button></li> : null}
    </ul> : null}
  </li>;
}
