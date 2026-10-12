import { invoke } from '@tauri-apps/api/core';
import { pickAndAddLibrarySource } from './settings';

export interface AgentSetupSelection {
  agent: 'claude' | 'codex'; scope: 'project' | 'user'; project_root: string | null;
}
export interface AgentSetupState {
  supported: boolean; installed: boolean; enabled: boolean; command: string | null;
  client_version: string | null; diagnostic: string | null;
  source_ids: string[]; sources: { id: string; name: string }[];
  projects: { path: string; name: string }[]; connection_status: 'not_verified';
}
export interface SimpleAgentBackend {
  state(selection: AgentSetupSelection): Promise<AgentSetupState>;
  install(selection: AgentSetupSelection): Promise<AgentSetupState>;
  setMcp(selection: AgentSetupSelection, sources: string[], enabled: boolean): Promise<AgentSetupState>;
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
  uninstall: selection => invoke('agent_setup_uninstall', { selection }),
};
