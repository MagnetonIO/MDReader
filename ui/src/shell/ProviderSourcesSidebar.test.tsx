import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { ProviderSourcesSidebar } from './ProviderSourcesSidebar';
import { providerTarget } from './provider-reading';
import type { NativeProvidersBackend, ProviderSource } from '../platform/providers';
const id = `provider:${'a'.repeat(64)}`;
const projection = { jobId: 'job', readerSourceId: id, path: `records/${'b'.repeat(64)}.md`, revision: 'c'.repeat(64) };
const source: ProviderSource = { sourceId: 'service-source', readerSourceId: id, name: 'Selected plans', kind: 'google_drive', enabled: true, selectedScope: { kind: 'exact_documents', count: 1 }, capabilities: { read: true, list: true, changes: true, assets: false, guardedWrite: false }, health: 'ready', coverage: { kind: 'metadata_complete_selected', observedAtMs: 1 } };
function backend() {
  return { state: vi.fn(async () => ({ availability: [], sources: [source], pendingAdmissions: [], error: null })), documents: vi.fn(async () => ({ operation: 'documents' as const, offset: 0, nextOffset: null, documents: [{ documentId: 'opaque-handle', name: 'Plan.md', displayPath: 'Plan.md', modifiedAtMs: null, sizeBytes: 10, cached: true, cachedObservedAtMs: 1, admitted: true, projection }] })), readCached: vi.fn(async () => projection), fetchCurrent: vi.fn(async () => projection) } satisfies Pick<NativeProvidersBackend, 'state'|'documents'|'readCached'|'fetchCurrent'>;
}
afterEach(cleanup);
test('sidebar browses locally, requires explicit search opt-in and current fetch, and opens exact cached evidence', async () => {
  const api=backend(); const select=vi.fn(); const open=vi.fn();
  render(<ProviderSourcesSidebar backend={api} selectedSources={[]} onSelectionChange={select} onOpen={open} onManage={vi.fn()} />);
  const checkbox=await screen.findByRole('checkbox',{name:'Include Selected plans in search'});
  expect((checkbox as HTMLInputElement).checked).toBe(false);
  expect(api.documents).not.toHaveBeenCalled(); expect(api.fetchCurrent).not.toHaveBeenCalled();
  fireEvent.click(checkbox); expect(select).toHaveBeenCalledWith([id]);
  fireEvent.click(screen.getByRole('button',{name:'Browse Selected plans'}));
  fireEvent.click(await screen.findByRole('button',{name:'Read cached Plan.md'}));
  await waitFor(()=>expect(open).toHaveBeenCalledWith({sourceId:id,path:projection.path,revision:projection.revision,title:'Plan.md',line:1}));
  expect(api.readCached).toHaveBeenCalledWith('service-source','opaque-handle'); expect(api.fetchCurrent).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button',{name:'Fetch current Plan.md'}));
  await waitFor(()=>expect(api.fetchCurrent).toHaveBeenCalledTimes(1));
});
test('source refresh cancels late document opens and clears disabled search selection', async () => {
  const api=backend(); let finish!:(value:typeof projection)=>void;
  api.readCached.mockImplementation(()=>new Promise((resolve)=>{finish=resolve;}));
  const select=vi.fn(); const open=vi.fn(); const props={backend:api,selectedSources:[id],onSelectionChange:select,onOpen:open,onManage:vi.fn()};
  const {rerender}=render(<ProviderSourcesSidebar {...props} refreshKey={0} />);
  fireEvent.click(await screen.findByRole('button',{name:'Browse Selected plans'}));
  fireEvent.click(await screen.findByRole('button',{name:'Read cached Plan.md'}));
  api.state.mockResolvedValue({availability:[],pendingAdmissions:[],error:null,sources:[{...source,enabled:false,health:'paused'}]});
  rerender(<ProviderSourcesSidebar {...props} refreshKey={1} />);
  await waitFor(()=>expect(select).toHaveBeenCalledWith([]));
  await act(async()=>finish(projection));
  expect(open).not.toHaveBeenCalled(); expect(api.fetchCurrent).not.toHaveBeenCalled();
  expect((screen.getByRole('checkbox',{name:'Include Selected plans in search'}) as HTMLInputElement).disabled).toBe(true);
});
test('canonical provider hit routes by exact revision and refuses local-path or missing-revision fallback', () => {
  expect(providerTarget({sourceId:id,path:projection.path,revision:projection.revision,name:'Plan',line:3})).toEqual({sourceId:id,path:projection.path,revision:projection.revision,title:'Plan',line:3});
  expect(()=>providerTarget({sourceId:id,path:projection.path,name:'Plan'})).toThrow(/exact revision/);
  expect(()=>providerTarget({sourceId:id,path:'/private/cache/plan.md',revision:projection.revision,name:'Plan'})).toThrow(/exact revision/);
  expect(providerTarget({sourceId:'local-source',path:'/granted/local.md',name:'Local'})).toBeNull();
});
