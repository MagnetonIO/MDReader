import { act, renderHook, waitFor } from '@testing-library/react';
import { expect, test, vi } from 'vitest';
import { useAccountSync } from './useAccountSync';
import type { AccountBackend, AccountReply, AccountStatus } from './backend';

const JOB = '00000000-0000-4000-8000-000000000001';
const state: AccountStatus = { configured: true, connected: false, retainedLibrary: true, custodyWritePending: false, activeLibraryId: JOB,
  existingLibraryId: null, existingLibrarySameSelection: false, recoveryAvailable: false, custody: null, latestJob: null, selectedScope: null };
test('same-render actions dispatch once and a concurrent refresh observes the retained job after begin settles', async () => {
  let finish!: (reply: AccountReply) => void;
  const begun = new Promise<AccountReply>((resolve) => { finish = resolve; });
  let active = false;
  const request = vi.fn<AccountBackend['request']>(async (operation) => {
    if (operation.operation === 'begin') return begun;
    if (operation.operation === 'poll') { active = false; return { operation: 'job', job: {
      id: JOB, selectedSync: null, selectedPurpose: null, state: { state: 'completed', outcome: { kind: 'connected' } },
    } }; }
    return { operation: 'status', status: { ...state, latestJob: active ? {
      id: JOB, selectedSync: null, selectedPurpose: null, state: { state: 'running', phase: 'signing_in' },
    } : null } };
  });
  const backend = { request };
  const second = renderHook(() => useAccountSync(backend));
  act(() => {
    void second.result.current.act({ action: 'sign_in' });
    void second.result.current.act({ action: 'sign_in' });
    void second.result.current.refresh();
  });
  expect(request.mock.calls.map(([operation]) => operation.operation)).toEqual(['begin']);
  active = true;
  await act(async () => finish({ operation: 'begun', jobId: JOB }));
  await waitFor(() => expect(request.mock.calls.some(([operation]) => operation.operation === 'status')).toBe(true));
  expect(request.mock.calls.filter(([operation]) => operation.operation === 'begin')).toHaveLength(1);
  second.unmount();
});
