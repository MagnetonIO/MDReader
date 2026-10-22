import type { AgentActivityBackend } from './contracts';

// Placeholder until the native agent-activity bindings land; replaced by the ReaderBackend adapter.
const unavailable = () => Promise.reject(new Error('Agent activity is unavailable'));
export const agentActivityBackend: AgentActivityBackend = {
  list: async () => ({ sessions: [], nextOffset: null }),
  files: async () => [],
  rename: unavailable, hide: unavailable, loadOlder: unavailable, forget: unavailable, setTracking: unavailable,
  trackingState: async () => ({ claude: { enabled: false, available: false }, codex: { enabled: false, available: false } }),
  onChanged: async () => () => {},
};
