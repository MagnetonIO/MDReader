import { act, within, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { AccountSyncPanel } from './AccountSyncPanel.js';
import type { AccountBackend, AccountReply, AccountStatus } from './backend.js';

beforeEach(() => Object.defineProperty(navigator, 'locks', { configurable:true, value:{ request:vi.fn(async (_name: string, _options: unknown, action: (lock: unknown) => unknown) => action({name:'synthetic-pending',mode:'exclusive'})) } }));
afterEach(() => { cleanup(); localStorage.clear(); });
const JOB = '00000000-0000-4000-8000-000000000001';
const SCOPE = { accountDigest:'a'.repeat(64), libraryId:JOB, deviceId:'A'.repeat(43) };
const DOC = { documentId:'00000000-0000-4000-8000-000000000003',sourceId:'b'.repeat(64),revision:'c'.repeat(64),title:'Selected public document',path:'document.md',line:1 };
const status = (patch: Partial<AccountStatus> = {}): AccountStatus => ({
  configured: true, connected: false, retainedLibrary: true, custodyWritePending: false, activeLibraryId: null, existingLibraryId: null, existingLibrarySameSelection: false, recoveryAvailable: false, custody: null, latestJob: null,
  selectedScope: patch.connected ? SCOPE : null, ...patch,
});

describe('native Account & sync controls', () => {
  it.each(['cancelled', 'interrupted'] as const)('offers the exact vault retry after %s without new enrollment actions', async (failure) => {
    const retained = status({ custodyWritePending: true, activeLibraryId: JOB,
      custody: { state: 'bootstrap_pending', enrollmentUncertain: true, enrollmentExpired: true,
        recoveryExportAvailable: false, materialStaged: false, deviceApproval: 'none' },
      latestJob: { id: JOB, selectedSync: null, selectedPurpose: null,
        state: failure === 'cancelled' ? { state: 'cancelled' } : { state: 'failed', error: { kind: 'interrupted' } } },
    });
    const request = vi.fn<AccountBackend['request']>(async operation => {
      if (operation.operation === 'begin') throw { kind: 'unavailable' };
      return { operation: 'status', status: retained };
    });
    render(<AccountSyncPanel backend={{ request }} />);
    const retry = await screen.findByRole('button', { name: 'Retry secure storage' });
    expect(screen.getAllByRole('button', { name: 'Retry secure storage' })).toHaveLength(1);
    expect(screen.queryByRole('button', { name: 'Retry setup' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Sign in / reconnect' })).toBeNull();
    expect(screen.queryByText('Recovery', { selector: 'summary' })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Refresh status' }));
    await waitFor(() => expect(request.mock.calls.filter(([op]) => op.operation === 'status')).toHaveLength(2));
    expect(screen.getByRole('button', { name: 'Retry secure storage' })).toBeTruthy();
    fireEvent.click(retry);
    await waitFor(() => expect(request).toHaveBeenCalledWith({ operation: 'begin', intent: { action: 'retry_custody_write' } }));
  });

  it('connects the verified existing library without replacing or copying its ID', async () => {
    const request = vi.fn<AccountBackend['request']>(async (operation) => {
      if (operation.operation === 'begin') throw { kind: 'unavailable' };
      return { operation: 'status', status: status({ existingLibraryId: JOB }) };
    });
    render(<AccountSyncPanel backend={{ request }} />);
    await screen.findByText(/Your existing sync library was found/);
    expect(screen.queryByRole('textbox', { name: /Library ID/ })).toBeNull();
    expect(screen.queryByText(/Find the library ID/)).toBeNull();
    expect(screen.queryByRole('button', { name: 'Sign in / reconnect' })).toBeNull();
    expect(screen.getAllByRole('button', { name: 'Connect this device' })).toHaveLength(1);
    fireEvent.click(within(screen.getByRole('group', { name: 'Sync connection' })).getByRole('button', { name: 'Connect this device' }));
    await waitFor(() => expect(request).toHaveBeenCalledWith({ operation: 'begin', intent: { action: 'join_device', remoteLibrary: JOB } }));
  });
  it.each([false, true])('uses a single explicit setup action for pending custody (same-library=%s)', async (sameLibrary) => {
    const request = vi.fn<AccountBackend['request']>(async (operation) => {
      if (operation.operation === 'begin') throw { kind: 'unavailable' };
      return { operation: 'status', status: status({
        existingLibraryId: sameLibrary ? JOB : null, existingLibrarySameSelection: sameLibrary,
        custody: { state: 'bootstrap_pending', enrollmentUncertain: true, enrollmentExpired: false,
          recoveryExportAvailable: false, materialStaged: false, deviceApproval: 'none' },
      }) };
    });
    render(<AccountSyncPanel backend={{ request }} />);
    const button = await screen.findByRole('button', { name: sameLibrary ? 'Check setup' : 'Retry setup' });
    expect(screen.queryByRole('button', { name: 'Sign in / reconnect' })).toBeNull();
    expect(request.mock.calls.every(([operation]) => operation.operation === 'status')).toBe(true);
    fireEvent.click(button);
    await waitFor(() => expect(request).toHaveBeenCalledWith({ operation: 'begin', intent: {
      action: sameLibrary ? 'reconcile_enrollment' : 'renew_enrollment',
    } }));
    expect(screen.queryByRole('button', { name: 'Connect this device' })).toBeNull();
  });
  it.each([false, true])('continues the retained pair candidate without a new library (uncertain=%s)', async (uncertain) => {
    const request = vi.fn<AccountBackend['request']>(async (operation) => {
      if (operation.operation === 'begin') throw { kind: 'unavailable' };
      return { operation: 'status', status: status({ activeLibraryId: JOB,
        custody: { state: 'pair_candidate', enrollmentUncertain: uncertain, enrollmentExpired: false,
          recoveryExportAvailable: false, materialStaged: false, deviceApproval: 'none' },
      }) };
    });
    render(<AccountSyncPanel backend={{ request }} />);
    const button = await screen.findByRole('button', { name: uncertain ? 'Show device request' : 'Continue connection' });
    expect(screen.queryByRole('button', { name: 'Sign in / reconnect' })).toBeNull();
    expect(request.mock.calls.every(([operation]) => operation.operation === 'status')).toBe(true);
    fireEvent.click(button);
    await waitFor(() => expect(request).toHaveBeenCalledWith({ operation: 'begin', intent: uncertain
      ? { action: 'show_pair_request' } : { action: 'join_device', remoteLibrary: JOB },
    }));
  });
  it('creates a first library only after an explicit setup action, with no manual UUID step', async () => {
    const request = vi.fn<AccountBackend['request']>(async (operation) => {
      if (operation.operation === 'begin') throw { kind: 'unavailable' };
      return { operation: 'status', status: status({ retainedLibrary: false }) };
    });
    render(<AccountSyncPanel backend={{ request }} />);
    await screen.findByText('Set up sync to connect your devices. Signing in does not upload documents.');
    expect(screen.getAllByRole('heading', { name: 'Set up sync' })).toHaveLength(1);
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
    expect(screen.queryByRole('button', { name: 'Sign in / reconnect' })).toBeNull();
    expect(screen.queryByRole('heading', { name: 'Sync a document' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Sync queued work' })).toBeNull();
    expect(screen.queryByText('Set up sync')).toBeNull();
    expect(request).toHaveBeenCalledExactlyOnceWith({ operation: 'status' });
    configured = true;
    fireEvent.click(screen.getByRole('button', { name: 'Refresh status' }));
    await screen.findByText('Library retained; sign in to resume.');
    fireEvent.click(screen.getByText('Recovery', { selector: 'summary' }));
    fireEvent.click(screen.getByText('Advanced · recovery library ID', { selector: 'summary' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Library ID for recovery' }), { target: { value: JOB } });
    expect(screen.getByText(/Recovery dialogs are unavailable/)).toBeTruthy();
    expect((screen.getByRole('button', { name: 'Recover this device' }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.queryByRole('button', { name: 'Save recovery file' })).toBeNull();
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
    const completed: NonNullable<AccountStatus['latestJob']> = { id: JOB, selectedSync:null,selectedPurpose:null, state: { state: 'completed', outcome: {
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
    await act(async () => resolve({ operation: 'job', job: { id: JOB, selectedSync:null,selectedPurpose:null, state: { state: 'completed', outcome: { kind: 'connected' } } } }));
    expect(request.mock.calls.map(([operation]) => operation.operation)).toEqual(['status', 'begin', 'poll']);
  });
  it('uploads only an explicit exact selection and retains a lost reply across navigation and reload', async () => {
    let connectedScope = SCOPE;
    const request = vi.fn<AccountBackend['request']>(async (operation) => {
      if (operation.operation === 'sync_selected') throw { kind:'unavailable' };
      return { operation:'status', status:status({ connected:true, selectedScope:connectedScope }) };
    });
    const backend = { request };
    const first = render(<AccountSyncPanel backend={backend} selectedDocument={DOC} />);
    await screen.findByText('Connected to the retained library.');
    expect(request.mock.calls.map(([op]) => op.operation)).toEqual(['status']);
    fireEvent.click(screen.getByRole('button', { name:'Sync this document' }));
    await screen.findByText(/outcome may be unknown/);
    const dispatched = request.mock.calls.find(([op]) => op.operation === 'sync_selected')![0];
    expect(dispatched).toMatchObject({ operation:'sync_selected', selection:{ scope:SCOPE, receipt:{ documentId:DOC.documentId, sourceId:DOC.sourceId, revision:DOC.revision } } });
    const other = { ...DOC, documentId:'00000000-0000-4000-8000-000000000004',title:'Different public document' };
    first.rerender(<AccountSyncPanel backend={backend} selectedDocument={other} />);
    expect(screen.getByText(/Changing the selected document does not replace/)).toBeTruthy();
    first.unmount();
    render(<AccountSyncPanel backend={backend} selectedDocument={other} />);
    await screen.findByRole('button', { name:'Retry retained sync' });
    fireEvent.click(screen.getByRole('button', { name:'Retry retained sync' }));
    await waitFor(() => expect(request.mock.calls.filter(([op]) => op.operation === 'sync_selected')).toHaveLength(2));
    expect(request.mock.calls.filter(([op]) => op.operation === 'sync_selected').map(([op]) => op)).toEqual([dispatched,dispatched]);
    connectedScope = { ...SCOPE, accountDigest:'d'.repeat(64) };
    fireEvent.click(screen.getByRole('button', { name:'Refresh status' }));
    await screen.findByText(/another account or library is retained/);
    expect(localStorage.getItem('mdreader.selected-sync.v1')).toContain(DOC.documentId);
    expect(request.mock.calls.filter(([op]) => op.operation === 'sync_selected')).toHaveLength(2);
  });

  it('clears a retained operation only after an exact readonly native disposition, not generic job completion', async () => {
    const selection = { scope:SCOPE, receipt:{ documentId:DOC.documentId, sourceId:DOC.sourceId, revision:DOC.revision },operationId:'00000000-0000-4000-8000-000000000005' };
    localStorage.setItem('mdreader.selected-sync.v1',JSON.stringify({version:1,operations:[selection]}));
    let begun = false;
    const job: NonNullable<AccountStatus['latestJob']> = { id:JOB,selectedSync:selection,selectedPurpose:"content",state:{state:'completed',outcome:{kind:'selected_status',selection,purpose:'content',disposition:{state:'applied',sequence:'3'}}} };
    const request = vi.fn<AccountBackend['request']>(async (operation) => {
      if (operation.operation === 'inspect_selected') { begun=true; return {operation:'begun',jobId:JOB}; }
      if (operation.operation === 'poll') return {operation:'job',job};
      return {operation:'status',status:status({connected:true,latestJob:begun?job:null})};
    });
    render(<AccountSyncPanel backend={{request}} selectedDocument={DOC} />);
    await screen.findByRole('button',{name:'Check retained sync'});
    expect(request).toHaveBeenCalledExactlyOnceWith({operation:'status'});
    fireEvent.click(screen.getByRole('button',{name:'Check retained sync'}));
    await screen.findByText('This exact document upload was applied. Other documents are unchanged.');
    expect(JSON.parse(localStorage.getItem('mdreader.selected-sync.v1')!).operations).toEqual([]);
    expect(request).toHaveBeenCalledWith({operation:'inspect_selected',selection});
    expect(request.mock.calls.some(([op]) => op.operation === 'sync_selected')).toBe(false);
  });

  it('rechecks current verified scope after an asynchronous storage lock before dispatch', async () => {
    let grant!: () => void;
    const held = new Promise<void>((done) => { grant=done; });
    Object.defineProperty(navigator,'locks',{configurable:true,value:{request:async (_name: string,_options:unknown,action:(lock:unknown)=>void) => { await held; action({name:'synthetic-pending',mode:'exclusive'}); }}});
    let exactScope = SCOPE;
    const request = vi.fn<AccountBackend['request']>(async () => ({operation:'status',status:status({connected:true,selectedScope:exactScope})}));
    render(<AccountSyncPanel backend={{request}} selectedDocument={DOC} />);
    await screen.findByRole('button',{name:'Sync this document'});
    fireEvent.click(screen.getByRole('button',{name:'Sync this document'}));
    exactScope = {...SCOPE,accountDigest:'e'.repeat(64)};
    fireEvent.click(screen.getByRole('button',{name:'Refresh status'}));
    await waitFor(() => expect(request).toHaveBeenCalledTimes(2));
    await act(async () => grant());
    await screen.findByText(/account changed before dispatch/);
    expect(request.mock.calls.some(([op]) => op.operation === 'sync_selected')).toBe(false);
    expect(localStorage.getItem('mdreader.selected-sync.v1')).toContain(SCOPE.accountDigest);
  });

});

it('imports only public request files and requires full identity confirmation before native approval', async () => {
  const request = vi.fn<AccountBackend['request']>(async operation => {
    if (operation.operation === 'begin') throw { kind: 'unavailable' };
    return { operation: 'status', status: status({ connected: true, activeLibraryId: JOB }) };
  });
  render(<AccountSyncPanel backend={{ request }} />);
  fireEvent.click(await screen.findByText('Add another device', { selector: 'summary' }));
  const text = JSON.stringify({ v: 1, kind: 'mdreader-pair-request', account: SCOPE.accountDigest, libraryId: JOB, pairId: DOC.documentId,
    signingJwk: { kty: 'EC', crv: 'P-256', x: 'A'.repeat(43), y: 'B'.repeat(43) }, wrappingJwk: { kty: 'EC', crv: 'P-256', x: 'C'.repeat(43), y: 'D'.repeat(43) },
    signingThumb: 'E'.repeat(43), wrappingThumb: 'F'.repeat(43), nonce: 'G'.repeat(43), expires: 2000000000 });
  const file = new File([text], 'device-request.json', { type: 'application/json' });
  Object.defineProperty(file, 'text', { value: async () => text });
  fireEvent.change(screen.getByLabelText('Choose device request file'), { target: { files: [file] } });
  const approve = await screen.findByRole('button', { name: 'Approve requested device' });
  expect((approve as HTMLButtonElement).disabled).toBe(true);
  expect(request.mock.calls.every(([operation]) => operation.operation === 'status')).toBe(true);
  fireEvent.click(screen.getByText('Compare full device identities', { selector: 'summary' }));
  expect(screen.getByText(SCOPE.accountDigest)).toBeTruthy();
  expect(screen.getByText('E'.repeat(43))).toBeTruthy(); expect(screen.getByText('F'.repeat(43))).toBeTruthy();
  fireEvent.click(screen.getByRole('checkbox', { name: /I compared the complete account/ }));
  fireEvent.click(approve);
  await waitFor(() => expect(request).toHaveBeenCalledWith({ operation: 'begin', intent: { action: 'approve_device', packet: text } }));
});

it('rejects recovery/private material in the public file chooser without exposing it or dispatching approval', async () => {
  const request = vi.fn<AccountBackend['request']>(async () => ({ operation: 'status', status: status({ connected: true, activeLibraryId: JOB }) }));
  render(<AccountSyncPanel backend={{ request }} />);
  fireEvent.click(await screen.findByText('Add another device', { selector: 'summary' }));
  const text = JSON.stringify({ recoverySecret: 'synthetic-private-material-never-shown' });
  const file = new File([text], 'recovery.json', { type: 'application/json' }); Object.defineProperty(file, 'text', { value: async () => text });
  fireEvent.change(screen.getByLabelText('Choose device request file'), { target: { files: [file] } });
  await screen.findByRole('alert');
  expect(screen.queryByDisplayValue(/synthetic-private-material/)).toBeNull();
  expect(screen.queryByRole('button', { name: 'Approve requested device' })).toBeNull();
  expect(request.mock.calls.map(([operation]) => operation.operation)).toEqual(['status']);
});

it('guides an uncertain pair request once without implying connection or offering unsupported enrollment checks', async () => {
  const packet = JSON.stringify({ v: 1, kind: 'mdreader-pair-request', account: SCOPE.accountDigest, libraryId: JOB, pairId: DOC.documentId,
    signingJwk: { kty: 'EC', crv: 'P-256', x: 'A'.repeat(43), y: 'B'.repeat(43) }, wrappingJwk: { kty: 'EC', crv: 'P-256', x: 'C'.repeat(43), y: 'D'.repeat(43) },
    signingThumb: 'E'.repeat(43), wrappingThumb: 'F'.repeat(43), nonce: 'G'.repeat(43), expires: 2000000000 });
  const request = vi.fn<AccountBackend['request']>(async () => ({ operation: 'status', status: status({ activeLibraryId: JOB,
    existingLibraryId: JOB, existingLibrarySameSelection: true, recoveryAvailable: true,
    custody: { state: 'pair_candidate', enrollmentUncertain: true, enrollmentExpired: false, materialStaged: false, recoveryExportAvailable: false, deviceApproval: 'none' },
    latestJob: { id: JOB, selectedSync: null, selectedPurpose: null, state: { state: 'completed', outcome: { kind: 'public_packet', packetKind: 'request', packet } } },
  }) }));
  render(<AccountSyncPanel backend={{ request }} />);
  const connection = await screen.findByRole('group', { name: 'Sync connection' });
  expect(within(connection).getByText('Awaiting device approval')).toBeTruthy();
  expect(within(connection).getByText(/This device is not connected yet/)).toBeTruthy();
  expect(within(connection).getAllByRole('button', { name: 'Save device request file' })).toHaveLength(1);
  expect(within(connection).getAllByLabelText('Choose device approval file')).toHaveLength(1);
  expect(screen.queryByRole('button', { name: 'Check setup' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Show device request' })).toBeNull();
  expect(screen.getByRole('textbox', { name: 'Public device packet', hidden: true }).closest('details')?.hasAttribute('open')).toBe(false);
  const recovery = screen.getByText('Recovery', { selector: 'summary' }).closest('details');
  const advanced = screen.getByText('Advanced', { selector: 'summary' }).closest('details');
  expect(recovery?.open).toBe(false); expect(advanced?.open).toBe(false);
  fireEvent.click(screen.getByText('Advanced', { selector: 'summary' }));
  expect(screen.queryByRole('button', { name: 'Check uncertain enrollment' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Retry enrollment sign-in' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Check uncertain device approval' })).toBeNull();
  expect(request.mock.calls.map(([operation]) => operation.operation)).toEqual(['status']);
});

it('opens useful recovery guidance for an expired pair without dispatching an unsafe restart or same-slot recovery', async () => {
  const request = vi.fn<AccountBackend['request']>(async () => ({ operation: 'status', status: status({ activeLibraryId: JOB, recoveryAvailable: true,
    custody: { state: 'pair_candidate', enrollmentUncertain: true, enrollmentExpired: true, materialStaged: false, recoveryExportAvailable: false, deviceApproval: 'none' },
  }) }));
  render(<AccountSyncPanel backend={{ request }} />);
  expect(await screen.findByText('Connection request expired')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'View recovery options' }));
  const recovery = screen.getByText('Recovery', { selector: 'summary' }).closest('details');
  expect(recovery?.open).toBe(true);
  expect(screen.getByText('Recovery on another device')).toBeTruthy();
  expect(screen.getByText(/This pending connection cannot switch to recovery/)).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Recover this device' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Continue connection' })).toBeNull();
  expect(request.mock.calls.map(([operation]) => operation.operation)).toEqual(['status']);
});
