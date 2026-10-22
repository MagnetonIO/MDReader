import type {
  AgentActivityFile as NativeFile, AgentActivitySession as NativeSession, AgentTrackingState as NativeTracking,
} from '../../bindings';
import { readerBackend, unwrapNative } from '../../platform/backend';
import type { AgentActivityBackend, AgentActivityFile, AgentActivitySession, AgentTrackingState } from './contracts';

// Native always records these timestamps; Specta types them nullable only because they are f64.
const session = (value: NativeSession): AgentActivitySession => ({
  ...value, startedAtMs: value.startedAtMs ?? 0, lastActivityMs: value.lastActivityMs ?? value.startedAtMs ?? 0,
});
const file = (value: NativeFile): AgentActivityFile => ({ ...value, firstAtMs: value.firstAtMs ?? 0, lastAtMs: value.lastAtMs ?? value.firstAtMs ?? 0 });
function tracking(states: NativeTracking[]): AgentTrackingState {
  const of = (agent: 'claude' | 'codex') => {
    const state = states.find(entry => entry.agent === agent);
    return { enabled: !!state?.enabled, available: !!state?.available };
  };
  return { claude: of('claude'), codex: of('codex') };
}

export const agentActivityBackend: AgentActivityBackend = {
  async list({ agent, offset, limit, filter }) {
    const page = await unwrapNative(readerBackend().agentActivity.agentActivityList(agent ?? null, offset, limit, filter ?? null,
      // Fallback titles carry the viewer's local date.
      -new Date().getTimezoneOffset()));
    return { sessions: page.sessions.map(session), nextOffset: page.nextOffset, total: agent ? page.totals[agent] : null };
  },
  files: async sessionId => (await unwrapNative(readerBackend().agentActivity.agentActivityFiles(sessionId))).map(file),
  rename: async (sessionId, title) => { await unwrapNative(readerBackend().agentActivity.agentActivityRename(sessionId, title)); },
  hide: async sessionId => { await unwrapNative(readerBackend().agentActivity.agentActivityHide(sessionId)); },
  revealCwd: async sessionId => { await unwrapNative(readerBackend().agentActivity.agentActivityRevealCwd(sessionId)); },
  trackingState: async () => tracking(await unwrapNative(readerBackend().agentActivity.agentActivityTrackingState())),
  setTracking: async (agent, enabled) => tracking(await unwrapNative(readerBackend().agentActivity.setAgentActivityTracking(agent, enabled))),
  loadOlder: async days => { await unwrapNative(readerBackend().agentActivity.agentActivityLoadOlder(days)); },
  forget: async agent => { await unwrapNative(readerBackend().agentActivity.agentActivityForget(agent)); },
  onChanged: handler => readerBackend().agentActivity.onChanged(payload => handler(payload.sessionId)),
};
