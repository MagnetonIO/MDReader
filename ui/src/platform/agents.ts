import type { AgentCaptureState, AgentCaptureUpdate } from '../bindings';
import type { AgentIntegrationsState, AgentIntegrationSelection, AgentIntegrationReview } from '../bindings';
import { readerBackend } from './backend';
export type { AgentCaptureState, AgentCaptureUpdate, AgentIntegrationsState, AgentIntegrationSelection, AgentIntegrationReview };
export interface AgentIntegrationBackend {
  state(root: string): Promise<AgentIntegrationsState>;
  prepare(root: string, selection: AgentIntegrationSelection): Promise<AgentIntegrationReview>;
  apply(root: string, id: string): Promise<void>;
  uninstall(root: string, id: string): Promise<void>;
}
export const agentIntegrationBackend: AgentIntegrationBackend = {
  async state(root) { const r = await readerBackend().agents.agentIntegrationsState(root); if (r.status === 'error') throw new Error(r.error); return r.data; },
  async prepare(root, selection) { const r = await readerBackend().agents.prepareAgentIntegration(root, selection); if (r.status === 'error') throw new Error(r.error); return r.data; },
  async apply(root, id) { const r = await readerBackend().agents.applyAgentIntegration(root, id); if (r.status === 'error') throw new Error(r.error); },
  async uninstall(root, id) { const r = await readerBackend().agents.uninstallAgentIntegration(root, id); if (r.status === 'error') throw new Error(r.error); },
};
export interface AgentCaptureBackend {
  state(root: string): Promise<AgentCaptureState>;
  update(root: string, update: AgentCaptureUpdate): Promise<AgentCaptureState>;
}
export const agentCaptureBackend: AgentCaptureBackend = {
  async state(root) {
    const result = await readerBackend().agents.agentCaptureState(root);
    if (result.status === 'error') throw new Error(result.error);
    return result.data;
  },
  async update(root, update) {
    const result = await readerBackend().agents.setAgentCapture(root, update);
    if (result.status === 'error') throw new Error(result.error);
    return result.data;
  },
};
