import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AccountSyncPanel } from '../AccountSyncPanel';
import type { AccountBackend, AccountJob, AccountReply, AccountStatus } from '../backend';
import type { Outcome } from '../../../bindings';

afterEach(cleanup);
const SCOPE = { accountDigest: 'a'.repeat(64), libraryId: '00000000-0000-4000-8000-000000000001', deviceId: 'A'.repeat(43) };
const ENTRY = { sequence: '9007199254740993', revisionId: 'b'.repeat(64), kind: 'content', title: 'Shared notes',
  reason: 'head_conflict', resolutionToken: 'native-issued-token', choices: ['keep_both' as const] };
const completed = (id: string, outcome: Outcome): AccountJob => ({ id, selectedSync: null, selectedPurpose: null, state: { state: 'completed', outcome } });
const state = (latestJob: AccountJob | null, selectedScope = SCOPE): AccountStatus => ({ configured: true, connected: true,
  retainedLibrary: true, activeLibraryId: null, existingLibraryId: null, existingLibrarySameSelection: false, recoveryAvailable: false, custody: null, selectedScope, latestJob });
const resolveRequests = (request: ReturnType<typeof vi.fn<AccountBackend['request']>>) => request.mock.calls
  .map(([operation]) => operation).filter((operation) => operation.operation === 'resolve_pending');

it('preserves the public approval packet when Account settings opens and reopens without starting another job', async () => {
  const text = JSON.stringify({ v: 1, kind: 'mdreader-pair-approval', account: SCOPE.accountDigest, libraryId: SCOPE.libraryId,
    pairId: '00000000-0000-4000-8000-000000000003', senderJwk: { kty: 'EC', crv: 'P-256', x: 'A'.repeat(43), y: 'B'.repeat(43) },
    senderThumb: 'C'.repeat(43), grantHash: 'd'.repeat(64), jweHash: 'e'.repeat(64) });
  const packet = completed('approval-job', { kind: 'public_packet', packetKind: 'approval', packet: text });
  const request = vi.fn<AccountBackend['request']>(async () => ({ operation: 'status', status: state(packet) }));
  const backend = { request };
  const view = render(<AccountSyncPanel backend={backend} />);
  fireEvent.click(await screen.findByText('Advanced · public JSON', { selector: 'summary' }));
  expect((await screen.findByRole('textbox', { name: 'Public device packet' }) as HTMLTextAreaElement).value).toBe(text);
  view.rerender(<AccountSyncPanel backend={backend} active={false} />);
  view.rerender(<AccountSyncPanel backend={backend} active />);
  await waitFor(() => expect(request).toHaveBeenCalledTimes(2));
  expect((screen.getByRole('textbox', { name: 'Public device packet' }) as HTMLTextAreaElement).value).toBe(text);
  expect(request.mock.calls.map(([operation]) => operation.operation)).toEqual(['status', 'status']);
});

it('requires confirmation of the exact native choice and never retries an uncertain resolution automatically', async () => {
  let job: AccountJob | null = null;
  let lists = 0;
  const request = vi.fn<AccountBackend['request']>(async (operation) => {
    if (operation.operation === 'list_pending') {
      lists++;
      job = completed(`list-${lists}`, { kind: 'pending_list', scope: SCOPE,
        entries: [{ ...ENTRY, resolutionToken: lists === 1 ? ENTRY.resolutionToken : 'fresh-native-token' }] });
      return { operation: 'begun', jobId: job.id };
    }
    if (operation.operation === 'poll') return { operation: 'job', job };
    if (operation.operation === 'resolve_pending') throw { kind: 'unavailable' };
    return { operation: 'status', status: state(job) };
  });
  render(<AccountSyncPanel backend={{ request }} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Refresh changes' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Keep both copies…' }));
  expect(resolveRequests(request)).toEqual([]);
  fireEvent.click(screen.getByRole('button', { name: 'Keep both copies' }));
  await screen.findByText(/Your choice could not be confirmed/);
  expect(resolveRequests(request)).toEqual([{ operation: 'resolve_pending', scope: SCOPE,
    sequence: ENTRY.sequence, revisionId: ENTRY.revisionId, resolutionToken: ENTRY.resolutionToken, choice: 'keep_both' }]);
  fireEvent.click(screen.getByRole('button', { name: 'Refresh status' }));
  await waitFor(() => expect(request.mock.calls.filter(([operation]) => operation.operation === 'status')).toHaveLength(3));
  expect(resolveRequests(request)).toHaveLength(1);
  expect((screen.getByRole('button', { name: 'Keep both copies…' }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Refresh changes' }));
  await waitFor(() => expect((screen.getByRole('button', { name: 'Keep both copies…' }) as HTMLButtonElement).disabled).toBe(false));
  expect(resolveRequests(request)).toHaveLength(1);
  fireEvent.click(screen.getByRole('button', { name: 'Keep both copies…' }));
  fireEvent.click(screen.getByRole('button', { name: 'Keep both copies' }));
  await waitFor(() => expect(resolveRequests(request)).toHaveLength(2));
  expect(resolveRequests(request)[1]).toMatchObject({ resolutionToken: 'fresh-native-token' });
});

it('retires a late list from the previous scope and invalidates a ready item after ordinary sync', async () => {
  let release!: (reply: AccountReply) => void;
  const late = new Promise<AccountReply>((done) => { release = done; });
  let scope = SCOPE;
  let job: AccountJob | null = null;
  let lists = 0;
  const request = vi.fn<AccountBackend['request']>(async (operation) => {
    if (operation.operation === 'list_pending') {
      lists++;
      if (lists > 1) job = completed('fresh-list', { kind: 'pending_list', scope,
        entries: [{ ...ENTRY, reason: 'ready_to_apply', choices: [] }] });
      return { operation: 'begun', jobId: lists === 1 ? 'old-list' : 'fresh-list' };
    }
    if (operation.operation === 'poll') return operation.jobId === 'old-list' ? late : { operation: 'job', job };
    if (operation.operation === 'begin') {
      job = completed('sync', { kind: 'sync', committed: '0', conflicts: '0', received: '1', applied: '1', acknowledged: '1', remotePending: false, localPending: false, pending: null });
      return { operation: 'begun', jobId: job.id };
    }
    return { operation: 'status', status: state(job, scope) };
  });
  render(<AccountSyncPanel backend={{ request }} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Refresh changes' }));
  await waitFor(() => expect(request).toHaveBeenCalledWith({ operation: 'poll', jobId: 'old-list' }));
  scope = { ...SCOPE, accountDigest: 'c'.repeat(64) };
  fireEvent.click(screen.getByRole('button', { name: 'Refresh status' }));
  await waitFor(() => expect(request.mock.calls.filter(([operation]) => operation.operation === 'status')).toHaveLength(2));
  await act(async () => release({ operation: 'job', job: completed('old-list', { kind: 'pending_list', scope: SCOPE, entries: [ENTRY] }) }));
  expect(screen.queryByText('Shared notes')).toBeNull();
  expect(resolveRequests(request)).toEqual([]);
  fireEvent.click(screen.getByRole('button', { name: 'Refresh changes' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Continue sync' }));
  await screen.findByText('Queued sync work finished.');
  await waitFor(() => expect(screen.queryByText('Shared notes')).toBeNull());
  expect(screen.getByText('Pending changes have not been checked.')).toBeTruthy();
  expect(request.mock.calls.filter(([operation]) => operation.operation === 'list_pending')).toHaveLength(2);
  expect(resolveRequests(request)).toEqual([]);
});

it('offers only the native reading-state choice and preserves it without uploading automatically', async () => {
  let job: AccountJob | null = null;
  const request = vi.fn<AccountBackend['request']>(async (operation) => {
    if (operation.operation === 'list_pending') {
      job = completed('state-list', { kind: 'pending_list', scope: SCOPE,
        entries: [{ ...ENTRY, kind: 'user_state', reason: 'local_state_changed', choices: ['keep_local_state'] }] });
      return { operation: 'begun', jobId: job.id };
    }
    if (operation.operation === 'resolve_pending') {
      job = completed('state-choice', { kind: 'pending_resolved', scope: SCOPE, sequence: ENTRY.sequence,
        revisionId: ENTRY.revisionId, choice: 'keep_local_state', document: {
          documentId: '00000000-0000-4000-8000-000000000003', sourceId: 'd'.repeat(64), revision: 'e'.repeat(64),
        } });
      return { operation: 'begun', jobId: job.id };
    }
    if (operation.operation === 'poll') return { operation: 'job', job };
    return { operation: 'status', status: state(job) };
  });
  render(<AccountSyncPanel backend={{ request }} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Refresh changes' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Keep my reading state…' }));
  expect(screen.queryByRole('button', { name: 'Keep both copies…' })).toBeNull();
  expect(screen.getByText(/Nothing is uploaded automatically/)).toBeTruthy();
  expect(resolveRequests(request)).toEqual([]);
  fireEvent.click(screen.getByRole('button', { name: 'Keep my reading state' }));
  await screen.findByText('Your pin and position are kept. The other device’s state is preserved in history; nothing was uploaded.');
  expect(resolveRequests(request)).toEqual([{ operation: 'resolve_pending', scope: SCOPE,
    sequence: ENTRY.sequence, revisionId: ENTRY.revisionId, resolutionToken: ENTRY.resolutionToken, choice: 'keep_local_state' }]);
  expect(request.mock.calls.some(([operation]) => operation.operation === 'begin'
    || operation.operation === 'sync_selected' || operation.operation === 'sync_selected_state')).toBe(false);
});
