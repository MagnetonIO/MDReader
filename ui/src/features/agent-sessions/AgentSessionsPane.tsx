import { useEffect, useState } from 'react';
import { SettingsIcon } from '../../shell/icons';
import type { CanonicalReaderReference } from '../reader/contracts';
import { AgentGroup } from './AgentGroup';
import { agentActivityBackend } from './backend';
import { AGENTS, type AgentActivityBackend, type AgentKind } from './contracts';
import { SessionRow } from './SessionRow';
import { TreeFocusProvider, useTreeNavigation } from './tree';
import { useAgentActivity } from './useAgentActivity';
import './agent-sessions.css';

const OLDER_DAYS = 365;

function useNow(intervalMs: number) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(timer);
  }, [intervalMs]);
  return now;
}

/** Agent → session → Markdown file, for every session that created or edited a .md file. */
export function AgentSessionsPane({ backend = agentActivityBackend, refreshKey, onOpen, onManage }: {
  backend?: AgentActivityBackend; refreshKey?: unknown; onOpen(reference: CanonicalReaderReference): void; onManage(): void;
}) {
  const activity = useAgentActivity(backend, refreshKey);
  const { groups, tracking, query } = activity;
  const now = useNow(30_000);
  const tree = useTreeNavigation();
  const [collapsed, setCollapsed] = useState<ReadonlySet<AgentKind>>(() => new Set());
  const [olderLoaded, setOlderLoaded] = useState(false);
  const anyTracked = AGENTS.some(agent => tracking?.[agent].enabled);
  const visible = AGENTS.filter(agent => !query && tracking?.[agent].enabled || groups[agent].sessions.length || groups[agent].failed);
  const loading = AGENTS.some(agent => groups[agent].loading && !groups[agent].sessions.length);
  const empty = !visible.length && !loading && !query;
  const toggleAgent = (agent: AgentKind) => setCollapsed(previous => {
    const next = new Set(previous);
    if (!next.delete(agent)) next.add(agent);
    return next;
  });
  return <section className="agent-sessions" aria-labelledby="agent-sessions-title">
    <header className="agent-sessions__header">
      <h2 id="agent-sessions-title">Agents</h2>
      <button type="button" className="agent-sessions__icon" aria-label="Agent settings" title="Agent settings" onClick={onManage}><SettingsIcon /></button>
    </header>
    {empty && tracking && !anyTracked ? <div className="agent-sessions__empty">
      <p>See every Markdown file Claude Code and Codex create or edit, grouped by session.</p>
      <button type="button" className="agent-sessions__primary" onClick={onManage}>Turn on session tracking</button>
    </div> : <>
      {!empty || query ? <input type="search" className="agent-sessions__filter" aria-label="Filter sessions and files"
        placeholder="Filter sessions and files" value={activity.filter} onChange={event => activity.setFilter(event.target.value)} /> : null}
      {loading && !visible.length ? <p className="agent-sessions__note" role="status">Loading sessions…</p> : null}
      {empty && anyTracked ? <p className="agent-sessions__note">No sessions have created or edited Markdown in the last 30 days.</p> : null}
      {query && !loading && !visible.length ? <p className="agent-sessions__note" role="status">No sessions or files match “{query}”.</p> : null}
      {visible.length ? <TreeFocusProvider value={tree.active}>
        <ul role="tree" aria-label="Agent sessions" className="agent-sessions__tree" {...tree.treeProps}>
          {visible.map(agent => <AgentGroup key={agent} agent={agent} group={groups[agent]} expanded={!collapsed.has(agent)}
            tracked={!!tracking?.[agent].enabled} onToggle={() => toggleAgent(agent)} onMore={() => activity.showMore(agent)} onRetry={() => activity.retry(agent)}>
            {groups[agent].sessions.map(session => <SessionRow key={session.id} session={session} now={now}
              expanded={activity.expanded.has(session.id)} files={activity.files[session.id]}
              onToggle={() => activity.toggleSession(session.id)} onOpen={onOpen} onRetryFiles={() => void activity.loadFiles(session.id)}
              onRename={title => activity.rename(session.id, title)} onHide={() => void activity.hide(session.id)}
              onReveal={activity.revealCwd ? () => void activity.revealCwd!(session.id) : undefined} onError={activity.reportError} />)}
          </AgentGroup>)}
        </ul>
      </TreeFocusProvider> : null}
      {anyTracked && !olderLoaded && !query ? <button type="button" className="agent-sessions__link"
        onClick={() => void activity.loadOlder(OLDER_DAYS).then(done => setOlderLoaded(done))}>Load sessions older than 30 days</button> : null}
    </>}
    {activity.actionError ? <p className="agent-sessions__note" role="alert">{activity.actionError}</p> : null}
  </section>;
}
