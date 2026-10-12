import { invoke } from '@tauri-apps/api/core';
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
  projects: () => invoke('agent_projects_list'),
  sessions: projectId => invoke('agent_project_sessions', { projectId }),
  documents: (projectId, sessionId) => invoke('agent_project_documents', { projectId, sessionId }),
};
export function projectDocumentReference(document: AgentProjectDocument): CanonicalReaderReference {
  return catalogReference({ kind: 'exact', sourceId: document.source_id, path: document.path, revision: document.revision }, document.title);
}
