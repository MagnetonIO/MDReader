import { readerBackend, unwrapNative } from '../../platform/backend';
import { catalogReference } from '../catalog/backend';
import type { CanonicalReaderReference } from '../reader/contracts';

export interface AgentProject { id: string; name: string; enabled: boolean; monitoring: boolean }
export interface AgentSession { id: string; project_id: string; agent: 'codex' | 'claude'; title: string; started_at_ms: number; last_seen_ms: number; ended_at_ms: number | null }
export interface AgentProjectDocument { source_id: string; path: string; title: string; revision: string; session_ids: string[] }
export interface AgentProjectsBackend {
  projects(): Promise<AgentProject[]>;
  sessions(projectId: string): Promise<AgentSession[]>;
  documents(projectId: string, sessionId: string | null): Promise<AgentProjectDocument[]>;
}
export const agentProjectsBackend: AgentProjectsBackend = {
  projects: () => unwrapNative(readerBackend().agents.agentProjectsList()),
  // Native serializes `agent` as `String`; keep asserting the renderer's closed
  // projection, exactly as the untyped `invoke<T>` did.
  sessions: projectId => unwrapNative(readerBackend().agents.agentProjectSessions(projectId)) as Promise<AgentSession[]>,
  documents: (projectId, sessionId) => unwrapNative(readerBackend().agents.agentProjectDocuments(projectId, sessionId)),
};
export function projectDocumentReference(document: AgentProjectDocument): CanonicalReaderReference {
  return catalogReference({ kind: 'exact', sourceId: document.source_id, path: document.path, revision: document.revision }, document.title);
}
