import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { AccountSyncPanel } from './AccountSyncPanel.js';
import type { AccountBackend, AccountReply, AccountStatus } from './backend.js';

afterEach(cleanup);
const JOB = '00000000-0000-4000-8000-000000000001';
const status = (patch: Partial<AccountStatus> = {}): AccountStatus => ({
  configured: true, connected: false, retainedLibrary: true, custody: null, latestJob: null, ...patch,
});
function open() { fireEvent.click(screen.getByRole('button', { name: 'Manage encrypted sync' })); }

describe('native Account & sync controls', () => {
  it('inspects state only on explicit open and disables unavailable account actions', async () => {
    const request = vi.fn<AccountBackend['request']>(async () => ({ operation: 'status', status: status({ configured: false }) }));
    render(<AccountSyncPanel backend={{ request }} />);
    expect(request).not.toHaveBeenCalled();
    open();
    await screen.findByText('Encrypted sync is not configured in this build.');
    expect((screen.getByRole('button', { name: 'Sign in / reconnect' }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole('button', { name: 'Sync queued work' }) as HTMLButtonElement).disabled).toBe(true);
    expect(request).toHaveBeenCalledExactlyOnceWith({ operation: 'status' });
  });

  it('keeps a lost begin reply uncertain and refreshes the same owner without automatic reauthorization', async () => {
    const request = vi.fn<AccountBackend['request']>(async (operation) => {
      if (operation.operation === 'begin') throw { kind: 'unavailable' };
      return { operation: 'status', status: status() };
    });
    render(<AccountSyncPanel backend={{ request }} />);
    open();
    await screen.findByText('Library retained; sign in to resume.');
    fireEvent.click(screen.getByRole('button', { name: 'Sign in / reconnect' }));
    await screen.findByRole('alert');
    expect(screen.getByRole('alert').textContent).toContain('outcome may be unknown');
    fireEvent.click(screen.getByRole('button', { name: 'Refresh account state' }));
    await waitFor(() => expect(request.mock.calls.filter(([r]) => r.operation === 'status')).toHaveLength(2));
    expect(request.mock.calls.filter(([r]) => r.operation === 'begin')).toEqual([[{ operation: 'begin', intent: { action: 'sign_in' } }]]);
  });

  it('reports actual pending sync counters without turning durable application into reader acceptance', async () => {
    let begun = false;
    const completed: NonNullable<AccountStatus['latestJob']> = { id: JOB, state: { state: 'completed', outcome: {
      kind: 'sync', committed: '2', conflicts: '1', received: '9007199254740993', applied: '0', acknowledged: '0', remotePending: true, localPending: false,
    } } };
    const request = vi.fn<AccountBackend['request']>(async (operation) => {
      if (operation.operation === 'begin') { begun = true; return { operation: 'begun', jobId: JOB }; }
      if (operation.operation === 'poll') return { operation: 'job', job: completed };
      return { operation: 'status', status: status({ connected: true, latestJob: begun ? completed : null }) };
    });
    render(<AccountSyncPanel backend={{ request }} />);
    open();
    await screen.findByText('Connected to the retained library.');
    fireEvent.click(screen.getByRole('button', { name: 'Sync queued work' }));
    await screen.findByText(/Received: 9007199254740993/);
    expect(screen.getByText(/These counts do not prove/).textContent).toContain('Remote work remains pending');
    expect(request).toHaveBeenCalledWith({ operation: 'begin', intent: { action: 'sync_now' } });
  });

  it('retires late observation on unmount without cancelling the service-owned operation', async () => {
    let resolve!: (reply: AccountReply) => void;
    const pending = new Promise<AccountReply>((done) => { resolve = done; });
    const request = vi.fn<AccountBackend['request']>(async (operation) => {
      if (operation.operation === 'begin') return { operation: 'begun', jobId: JOB };
      if (operation.operation === 'poll') return pending;
      return { operation: 'status', status: status() };
    });
    const { unmount } = render(<AccountSyncPanel backend={{ request }} />);
    open();
    await screen.findByText('Library retained; sign in to resume.');
    fireEvent.click(screen.getByRole('button', { name: 'Sign in / reconnect' }));
    await waitFor(() => expect(request).toHaveBeenCalledWith({ operation: 'poll', jobId: JOB }));
    unmount();
    await act(async () => resolve({ operation: 'job', job: { id: JOB, state: { state: 'completed', outcome: { kind: 'connected' } } } }));
    expect(request.mock.calls.map(([operation]) => operation.operation)).toEqual(['status', 'begin', 'poll']);
  });
});
