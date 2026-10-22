import { useCallback, useEffect, useRef, useState } from 'react';
import { AGENTS, type AgentActivityBackend, type AgentActivityFile, type AgentActivitySession, type AgentKind, type AgentTrackingState } from './contracts';

export const PAGE_SIZE = 20;
const FILTER_DELAY_MS = 200, CHANGE_COALESCE_MS = 250;

export interface SessionGroup { sessions: AgentActivitySession[]; nextOffset: number | null; loading: boolean; failed: boolean }
export interface SessionFilesState { files: AgentActivityFile[] | null; loading: boolean; failed: boolean }
const emptyGroup: SessionGroup = { sessions: [], nextOffset: null, loading: true, failed: false };
const emptyGroups = (): Record<AgentKind, SessionGroup> => ({ claude: emptyGroup, codex: emptyGroup });

function merge(previous: AgentActivitySession[], next: AgentActivitySession[]) {
  const ids = new Set(next.map(session => session.id));
  return [...previous.filter(session => !ids.has(session.id)), ...next];
}

/** Paged sessions per agent plus files of expanded sessions, refreshed only by native change events. */
export function useAgentActivity(backend: AgentActivityBackend, refreshKey: unknown = 0) {
  const [tracking, setTracking] = useState<AgentTrackingState | null>(null);
  const [filter, setFilter] = useState('');
  const [query, setQuery] = useState('');
  const [groups, setGroups] = useState(emptyGroups);
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set());
  const [files, setFiles] = useState<Record<string, SessionFilesState>>({});
  const [actionError, setActionError] = useState<string | null>(null);
  const turn = useRef<Record<AgentKind, number>>({ claude: 0, codex: 0 });
  const fileTurn = useRef(new Map<string, number>());
  const latest = useRef({ groups, expanded }); latest.current = { groups, expanded };

  useEffect(() => {
    const timer = window.setTimeout(() => setQuery(filter.trim()), FILTER_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [filter]);

  const load = useCallback(async (agent: AgentKind, offset: number, limit: number, append: boolean) => {
    const id = ++turn.current[agent];
    setGroups(all => ({ ...all, [agent]: { ...all[agent], loading: true, failed: false } }));
    try {
      const page = await backend.list({ agent, offset, limit, ...(query ? { filter: query } : {}) });
      if (turn.current[agent] !== id) return;
      // Sessions that wrote no Markdown are never listed, even if native reports them.
      const sessions = page.sessions.filter(session => session.agent === agent && session.fileCount > 0);
      setGroups(all => ({ ...all, [agent]: { sessions: append ? merge(all[agent].sessions, sessions) : sessions,
        nextOffset: page.nextOffset, loading: false, failed: false } }));
    } catch {
      if (turn.current[agent] === id) setGroups(all => ({ ...all, [agent]: { ...all[agent], loading: false, failed: true } }));
    }
  }, [backend, query]);

  const loadFiles = useCallback(async (sessionId: string) => {
    const id = (fileTurn.current.get(sessionId) ?? 0) + 1;
    fileTurn.current.set(sessionId, id);
    setFiles(all => ({ ...all, [sessionId]: { files: all[sessionId]?.files ?? null, loading: true, failed: false } }));
    try {
      const next = await backend.files(sessionId);
      if (fileTurn.current.get(sessionId) === id) setFiles(all => ({ ...all, [sessionId]: { files: next, loading: false, failed: false } }));
    } catch {
      if (fileTurn.current.get(sessionId) === id) setFiles(all => ({ ...all, [sessionId]: { files: all[sessionId]?.files ?? null, loading: false, failed: true } }));
    }
  }, [backend]);

  /** Re-reads every loaded row so a refresh never shrinks what "Show more" revealed. */
  const refreshLists = useCallback(() => {
    for (const agent of AGENTS) void load(agent, 0, Math.max(PAGE_SIZE, latest.current.groups[agent].sessions.length), false);
  }, [load]);
  const refreshTracking = useCallback(() => {
    void backend.trackingState().then(setTracking, () => setTracking(null));
  }, [backend]);

  useEffect(() => {
    setGroups(emptyGroups());
    for (const agent of AGENTS) void load(agent, 0, PAGE_SIZE, false);
  }, [load]);
  // Settings may have changed tracking; keep the visible rows while re-reading.
  const seenKey = useRef(refreshKey);
  useEffect(() => {
    refreshTracking();
    if (seenKey.current !== refreshKey) { seenKey.current = refreshKey; refreshLists(); }
  }, [refreshTracking, refreshKey]);
  useEffect(() => {
    let changed = new Set<string | null>(), timer = 0, live = true, unlisten: (() => void) | null = null;
    const flush = () => {
      const ids = changed; changed = new Set(); timer = 0;
      refreshLists();
      for (const id of latest.current.expanded) if (ids.has(null) || ids.has(id)) void loadFiles(id);
    };
    void backend.onChanged(sessionId => {
      changed.add(sessionId);
      if (!timer) timer = window.setTimeout(flush, CHANGE_COALESCE_MS);
    }).then(stop => { if (live) unlisten = stop; else stop(); }, () => {});
    return () => { live = false; unlisten?.(); window.clearTimeout(timer); };
  }, [backend, refreshLists, loadFiles]);

  function toggleSession(sessionId: string) {
    const next = new Set(expanded);
    if (next.delete(sessionId)) { setExpanded(next); return; }
    next.add(sessionId); setExpanded(next);
    void loadFiles(sessionId);
  }
  async function act(task: () => Promise<void>, failure: string) {
    setActionError(null);
    try { await task(); return true; } catch { setActionError(failure); return false; }
  }
  const update = (sessionId: string, change: (session: AgentActivitySession) => AgentActivitySession | null) =>
    setGroups(all => Object.fromEntries(AGENTS.map(agent => [agent, { ...all[agent],
      sessions: all[agent].sessions.flatMap(session => session.id === sessionId ? change(session) ?? [] : [session]) }])) as Record<AgentKind, SessionGroup>);

  return {
    tracking, filter, setFilter, query, groups, expanded, files, actionError,
    toggleSession, loadFiles,
    showMore: (agent: AgentKind) => { const next = groups[agent].nextOffset; if (next !== null) void load(agent, next, PAGE_SIZE, true); },
    retry: (agent: AgentKind) => void load(agent, 0, Math.max(PAGE_SIZE, groups[agent].sessions.length), false),
    rename: (sessionId: string, title: string) => act(async () => {
      await backend.rename(sessionId, title);
      update(sessionId, session => ({ ...session, title, titleSource: 'custom' }));
    }, 'The session could not be renamed.'),
    hide: (sessionId: string) => act(async () => { await backend.hide(sessionId); update(sessionId, () => null); }, 'The session could not be hidden.'),
    revealCwd: backend.revealCwd ? (sessionId: string) => act(() => backend.revealCwd!(sessionId), 'The folder could not be shown.') : undefined,
    loadOlder: (days: number) => act(async () => { await backend.loadOlder(days); refreshLists(); }, 'Older sessions could not be loaded.'),
    reportError: setActionError,
  };
}
export type AgentActivityController = ReturnType<typeof useAgentActivity>;
