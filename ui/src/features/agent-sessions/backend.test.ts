import { afterEach, expect, test, vi } from 'vitest';
import type { AgentActivityChanged, AgentTrackingState } from '../../bindings';
import { setReaderBackend, type ReaderBackend } from '../../platform/backend';
import { tauriBackend } from '../../platform/tauri-backend';
import { agentActivityBackend } from './backend';

let restore: (() => void) | undefined;
afterEach(() => { restore?.(); restore = undefined; });
const ok = <T,>(data: T) => ({ status: 'ok' as const, data });

function install(activity: Partial<ReaderBackend['agentActivity']>) {
  restore = setReaderBackend({ ...tauriBackend, agentActivity: { ...tauriBackend.agentActivity, ...activity } });
}

test('pages carry the queried agent total and native nulls become numbers', async () => {
  const agentActivityList = vi.fn(async () => ok({ nextOffset: 20, totals: { claude: 42, codex: 3 }, sessions: [{
    id: 's', agent: 'claude' as const, title: 'repo · 2026-10-21', titleSource: 'fallback' as const, cwd: '/public/repo', gitBranch: null,
    startedAtMs: 5, lastActivityMs: null, endedAtMs: null, fileCount: 1 }] }));
  install({ agentActivityList });
  const page = await agentActivityBackend.list({ agent: 'claude', offset: 0, limit: 20 });
  expect(agentActivityList).toHaveBeenCalledWith('claude', 0, 20, null, -new Date().getTimezoneOffset());
  expect(page).toEqual({ nextOffset: 20, total: 42, sessions: [expect.objectContaining({ startedAtMs: 5, lastActivityMs: 5, endedAtMs: null })] });
});

test('tracking arrays become per-agent switches and native errors reject', async () => {
  const states: AgentTrackingState[] = [{ agent: 'codex', enabled: true, available: true, sinceMs: 1, sessions: 2 }];
  install({
    agentActivityTrackingState: async () => ok(states),
    setAgentActivityTracking: async () => ({ status: 'error' as const, error: 'Agent log folder unavailable' }),
  });
  expect(await agentActivityBackend.trackingState()).toEqual({ claude: { enabled: false, available: false }, codex: { enabled: true, available: true } });
  await expect(agentActivityBackend.setTracking('claude', true)).rejects.toBe('Agent log folder unavailable');
});

test('change events forward only the session id', async () => {
  let emit!: (payload: AgentActivityChanged) => void;
  install({ onChanged: async handler => { emit = handler; return () => {}; } });
  const handler = vi.fn();
  await agentActivityBackend.onChanged(handler);
  emit({ sessionId: null }); emit({ sessionId: 'one' });
  expect(handler.mock.calls).toEqual([[null], ['one']]);
});
