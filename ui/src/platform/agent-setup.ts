import { invoke } from '@tauri-apps/api/core';
import { pickAndAddLibrarySource } from './settings';

export interface AgentSetupSelection {
  agent: 'claude' | 'codex'; scope: 'project' | 'user'; project_root: string | null;
}
export interface AgentSetupState {
  supported: boolean; installed: boolean; enabled: boolean; command: string | null;
  client_version: string | null; diagnostic: string | null;
  source_ids: string[]; sources: { id: string; name: string; editable?: boolean }[];
  projects: { id?: string | null; path: string; name: string; enabled?: boolean; monitoring?: boolean }[];
  reporting_project_ids?: string[]; edit_source_ids?: string[];
  connected?: boolean; last_seen_ms?: number | null; connection_status: 'not_verified' | 'connected';
}
export interface SimpleAgentBackend {
  state(selection: AgentSetupSelection): Promise<AgentSetupState>;
  install(selection: AgentSetupSelection): Promise<AgentSetupState>;
  setMcp(selection: AgentSetupSelection, sources: string[], enabled: boolean): Promise<AgentSetupState>;
  setPermissions?(selection: AgentSetupSelection, grants: { enabled: boolean; project_ids: string[]; read_source_ids: string[]; edit_source_ids: string[] }): Promise<AgentSetupState>;
  uninstall(selection: AgentSetupSelection): Promise<AgentSetupState>;
  chooseProjectFolder?(): Promise<{ path: string; name: string } | null>;
}
/** Configuration only: native validates target, owned entries and explicit read grants. */
export const simpleAgentBackend: SimpleAgentBackend = {
  chooseProjectFolder: async () => {
    const picked = await pickAndAddLibrarySource();
    if (!picked.source) return null;
    if (picked.source.state.kind !== 'available') throw new Error('Selected folder unavailable');
    return { path: picked.source.path, name: picked.source.name };
  },
  state: selection => invoke('agent_setup_state', { selection }),
  install: selection => invoke('agent_setup_install', { selection }),
  setMcp: (selection, sources, enabled) => invoke('agent_setup_set_mcp', { selection, sources, enabled }),
  setPermissions: (selection, grants) => invoke('agent_setup_set_permissions', { selection, enabled: grants.enabled,
    projectIds: grants.project_ids, readSourceIds: grants.read_source_ids, editSourceIds: grants.edit_source_ids }),
  uninstall: selection => invoke('agent_setup_uninstall', { selection }),
};
