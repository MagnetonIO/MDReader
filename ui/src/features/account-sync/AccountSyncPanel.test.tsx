import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { AccountSyncPanel } from './AccountSyncPanel.js';
import type { AccountBackend, AccountReply, AccountStatus } from './backend.js';

afterEach(cleanup);
const JOB = '00000000-0000-4000-8000-000000000001';
const status = (patch: Partial<AccountStatus> = {}): AccountStatus => ({
  configured: true, connected: false, retainedLibrary: true, recoveryAvailable: false, custody: null, latestJob: null, ...patch,
});

describe('native Account & sync controls', () => {
  it('creates a first library only after an explicit setup action, with no manual UUID step', async () => {
    const request = vi.fn<AccountBackend['request']>(async (operation) => {
      if (operation.operation === 'begin') throw { kind: 'unavailable' };
      return { operation: 'status', status: status({ retainedLibrary: false }) };
    });
    render(<AccountSyncPanel backend={{ request }} />);
    await screen.findByText('Not connected. Select a library explicitly to enroll.');
    fireEvent.click(screen.getByText('Set up sync', { selector: 'summary' }));
    expect(screen.queryByRole('textbox', { name: /^Library ID$/ })).toBeNull();
    expect(request).toHaveBeenCalledExactlyOnceWith({ operation: 'status' });
    fireEvent.click(screen.getByRole('button', { name: 'Create sync library' }));
    await screen.findByRole('alert');
    expect(request.mock.calls.filter(([operation]) => operation.operation === 'begin')).toEqual([
      [{ operation: 'begin', intent: { action: 'bootstrap', remoteLibrary: expect.stringMatching(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/) } }],
    ]);
  });

  it('inspects metadata on category activation and disables unavailable account actions', async () => {
    let configured = false;
    const request = vi.fn<AccountBackend['request']>(async () => ({ operation: 'status', status: status({ configured }) }));
    const backend = { request };
    const { rerender } = render(<AccountSyncPanel backend={backend} active={false} />);
    expect(request).not.toHaveBeenCalled();
    rerender(<AccountSyncPanel backend={backend} active />);
    await screen.findByText('Encrypted sync is not configured in this build.');
    expect((screen.getByRole('button', { name: 'Sign in / reconnect' }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.queryByRole('button', { name: 'Sync queued work' })).toBeNull();
    expect(screen.queryByText('Set up sync')).toBeNull();
    expect(request).toHaveBeenCalledExactlyOnceWith({ operation: 'status' });
    configured = true;
    fireEvent.click(screen.getByRole('button', { name: 'Refresh status' }));
    await screen.findByText('Library retained; sign in to resume.');
    fireEvent.click(screen.getByText('Set up sync', { selector: 'summary' }));
    fireEvent.click(screen.getByText('Recovery', { selector: 'summary' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Library ID for recovery' }), { target: { value: JOB } });
    expect(screen.getByText(/Recovery dialogs are unavailable/)).toBeTruthy();
    expect((screen.getByRole('button', { name: 'Recover this device' }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole('button', { name: 'Save recovery file' }) as HTMLButtonElement).disabled).toBe(true);
    expect(request.mock.calls.every(([operation]) => operation.operation === 'status')).toBe(true);
  });

  it('keeps a lost begin reply uncertain and refreshes the same owner without automatic reauthorization', async () => {
    const request = vi.fn<AccountBackend['request']>(async (operation) => {
      if (operation.operation === 'begin') throw { kind: 'unavailable' };
      return { operation: 'status', status: status() };
    });
    render(<AccountSyncPanel backend={{ request }} />);
    await screen.findByText('Library retained; sign in to resume.');
    fireEvent.click(screen.getByRole('button', { name: 'Sign in / reconnect' }));
    await screen.findByRole('alert');
    expect(screen.getByRole('alert').textContent).toContain('outcome may be unknown');
    fireEvent.click(screen.getByRole('button', { name: 'Refresh status' }));
    await waitFor(() => expect(request.mock.calls.filter(([r]) => r.operation === 'status')).toHaveLength(2));
    expect(request.mock.calls.filter(([r]) => r.operation === 'begin')).toEqual([[{ operation: 'begin', intent: { action: 'sign_in' } }]]);
  });

  it('reports actual pending sync counters without turning durable application into reader acceptance', async () => {
    let begun = false;
    const completed: NonNullable<AccountStatus['latestJob']> = { id: JOB, state: { state: 'completed', outcome: {
      kind: 'sync', committed: '2', conflicts: '1', received: '9007199254740993', applied: '0', acknowledged: '0', remotePending: true, localPending: false, pending: { sequence: '9007199254740993', reason: 'head_conflict' },
    } } };
    const request = vi.fn<AccountBackend['request']>(async (operation) => {
      if (operation.operation === 'begin') { begun = true; return { operation: 'begun', jobId: JOB }; }
      if (operation.operation === 'poll') return { operation: 'job', job: completed };
      return { operation: 'status', status: status({ connected: true, latestJob: begun ? completed : null }) };
    });
    render(<AccountSyncPanel backend={{ request }} />);
    await screen.findByText('Connected to the retained library.');
    fireEvent.click(screen.getByRole('button', { name: 'Sync queued work' }));
    await screen.findByText(/Received: 9007199254740993/);
    expect(screen.getByText(/These counts do not prove/).textContent).toContain('Remote work remains pending');
    expect(screen.getByText(/These counts do not prove/).textContent).toContain('Incoming item 9007199254740993: head conflict');
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
    await screen.findByText('Library retained; sign in to resume.');
    fireEvent.click(screen.getByRole('button', { name: 'Sign in / reconnect' }));
    await waitFor(() => expect(request).toHaveBeenCalledWith({ operation: 'poll', jobId: JOB }));
    unmount();
    await act(async () => resolve({ operation: 'job', job: { id: JOB, state: { state: 'completed', outcome: { kind: 'connected' } } } }));
    expect(request.mock.calls.map(([operation]) => operation.operation)).toEqual(['status', 'begin', 'poll']);
  });
});
