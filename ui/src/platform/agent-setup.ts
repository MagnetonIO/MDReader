import type { SetupState } from '../bindings';
import { readerBackend, unwrapNative, type NativeResult } from './backend';
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
const setupState = (pending: Promise<NativeResult<SetupState, string>>) =>
  // `connection_status` is a closed vocabulary that native serializes as `String`.
  unwrapNative(pending) as Promise<AgentSetupState>;
/** Configuration only: native validates target, owned entries and explicit read grants. */
export const simpleAgentBackend: SimpleAgentBackend = {
  chooseProjectFolder: async () => {
    const picked = await pickAndAddLibrarySource();
    if (!picked.source) return null;
    if (picked.source.state.kind !== 'available') throw new Error('Selected folder unavailable');
    return { path: picked.source.path, name: picked.source.name };
  },
  state: selection => setupState(readerBackend().agents.agentSetupState(selection)),
  install: selection => setupState(readerBackend().agents.agentSetupInstall(selection)),
  setMcp: (selection, sources, enabled) => setupState(readerBackend().agents.agentSetupSetMcp(selection, sources, enabled)),
  setPermissions: (selection, grants) => setupState(readerBackend().agents.agentSetupSetPermissions(selection, grants.enabled,
    grants.project_ids, grants.read_source_ids, grants.edit_source_ids)),
  uninstall: selection => setupState(readerBackend().agents.agentSetupUninstall(selection)),
};
