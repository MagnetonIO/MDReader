import { useEffect, useRef, useState } from 'react';
import { agentProjectsBackend, projectDocumentReference, type AgentProject, type AgentSession, type AgentProjectDocument, type AgentProjectsBackend } from './backend';
import type { CanonicalReaderReference } from '../reader/contracts';
import './agent-projects.css';

/** Files stay available in All documents even when no agent reported a session. */
export function AgentProjects({ backend = agentProjectsBackend, refreshKey = 0, onOpen, onManage }: {
  backend?: AgentProjectsBackend; refreshKey?: number | string; onOpen(reference: CanonicalReaderReference): void; onManage(): void;
}) {
  const [projects, setProjects] = useState<AgentProject[]>([]);
  const [projectId, setProjectId] = useState<string | null>(null);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [result, setResult] = useState<{ projectId: string; sessionId: string | null; sessions: AgentSession[]; documents: AgentProjectDocument[] } | null>(null);
  const selected = result?.projectId === projectId && result.sessionId === sessionId ? result : null;
  const sessions = result?.projectId === projectId ? result.sessions : [];
  const documents = selected?.documents ?? [];
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  const [projectBusy, setProjectBusy] = useState(false);
  const request = useRef(0);
  useEffect(() => {
    if (!open) return;
    let current = true;
    setProjectBusy(true);
    void backend.projects().then(value => {
      if (!current) return;
      setProjects(value);
      if (projectId && !value.some(project => project.id === projectId && project.enabled)) { setProjectId(null); setSessionId(null); setResult(null); }
    }).catch(() => { if (current) setError('Projects could not be loaded.'); }).finally(() => { if (current) setProjectBusy(false); });
    return () => { current = false; };
  }, [backend, open, refreshKey, retry, projectId]);
  useEffect(() => {
    if (!open) return;
    const timer = window.setInterval(() => setRetry(value => value + 1), 10_000);
    return () => window.clearInterval(timer);
  }, [open]);
  useEffect(() => {
    const id = ++request.current;
    setError(null);
    if (!open || !projectId) { setBusy(false); return; }
    setBusy(true);
    void Promise.all([backend.sessions(projectId), backend.documents(projectId, sessionId)]).then(([nextSessions, nextDocuments]) => {
      if (request.current !== id) return;
      setResult({ projectId, sessionId, sessions: nextSessions, documents: nextDocuments });
    }).catch(() => { if (request.current === id) setError('Project documents could not be loaded.'); })
      .finally(() => { if (request.current === id) setBusy(false); });
    return () => { request.current++; };
  }, [backend, open, projectId, sessionId, refreshKey, retry]);
  function chooseProject(id: string) { setProjectId(id); setSessionId(null); }
  return <nav className="agent-projects" aria-label="Agent projects">
    <div className="agent-projects__heading"><button type="button" aria-expanded={open} onClick={() => setOpen(value => !value)}><span aria-hidden="true">{open ? '▾' : '▸'}</span> Projects</button><button type="button" className="agent-projects__add" onClick={onManage} aria-label="Manage agent projects">+</button></div>
    {open ? <div className="agent-projects__body">
      {projects.map(project => <button type="button" key={project.id} className="agent-projects__project" disabled={!project.enabled} aria-pressed={projectId === project.id} onClick={() => chooseProject(project.id)}>{project.name}{!project.monitoring ? <span className="agent-projects__muted"> · Monitoring paused</span> : null}</button>)}
      {!projects.length && !error ? <p>{projectBusy ? 'Loading projects…' : 'Choose a project folder in Agents & integrations.'}</p> : null}
      {projectId ? <>
        <button type="button" className="agent-projects__all" aria-pressed={!sessionId} onClick={() => setSessionId(null)}>All documents</button>
        {sessions.length ? <div className="agent-projects__sessions" aria-label="Project sessions">{sessions.map(session => <button type="button" key={session.id} aria-pressed={session.id === sessionId} onClick={() => setSessionId(session.id)} title={`${session.agent === 'codex' ? 'Codex' : 'Claude Code'} · ${new Date(session.started_at_ms).toLocaleString()}`}><span>{session.title || 'Untitled session'}</span><span className="agent-projects__muted">{session.agent === 'codex' ? 'Codex' : 'Claude'}</span></button>)}</div> : null}
        {busy && !selected ? <p role="status">Loading documents…</p> : error ? null : documents.length ? <ul className="agent-projects__documents">{documents.map(document => <li key={`${document.source_id}:${document.path}`}><button type="button" title={document.path} onClick={() => {
          try { onOpen(projectDocumentReference(document)); } catch { setError('This document changed. Refresh the project to reopen it.'); }
        }}>{document.title || document.path.split('/').pop()}{!sessionId && document.session_ids.length ? <span className="agent-projects__muted"> · {document.session_ids.length} {document.session_ids.length === 1 ? 'session' : 'sessions'}</span> : null}</button></li>)}</ul> : <p>{sessionId ? 'No Markdown reported in this session yet.' : 'No Markdown indexed in this project yet.'}</p>}
      </> : null}
      {error ? <div role="alert"><p>{error}</p><button type="button" onClick={() => setRetry(value => value + 1)}>Retry</button></div> : null}
    </div> : null}
  </nav>;
}
