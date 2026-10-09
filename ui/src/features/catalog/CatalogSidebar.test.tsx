import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { CatalogSidebar } from './CatalogSidebar';
import { CatalogError, catalogReference, type CatalogBackend, type CatalogSource, type CatalogDocument } from './backend';
const localId='1'.repeat(64);const remoteId=`provider:${'a'.repeat(64)}`;
const source:CatalogSource={catalogId:`local:${localId}`,name:'Selected folder',kind:'local',enabled:true,coverage:'indexed_observation',observedAtMs:1,failure:null,librarySourceId:'library-id'};
const local:CatalogDocument={name:'Plan',displayPath:'plan.md',modifiedAtMs:1,sizeBytes:null,observedAtMs:1,observation:'indexed_local',open:{kind:'exact',sourceId:localId,path:'plan.md',revision:'b'.repeat(64)}};
function backend(){return {list:vi.fn(async()=>({operation:'list' as const,sources:[source],providerFailure:null,localFailure:null})),page:vi.fn<CatalogBackend['page']>(async()=>({operation:'page' as const,source,documents:[local],nextCursor:'cursor-a'})),close:vi.fn(async()=>{}),readDocument:vi.fn()} satisfies CatalogBackend;}
afterEach(cleanup);
test('catalog lists without fetch/admission and opens exact local evidence without filesystem path construction',async()=>{
    const api=backend();const fetchCurrent=vi.fn();const open=vi.fn();render(<CatalogSidebar backend={api} providers={{fetchCurrent}} onOpen={open} onManage={vi.fn()}/>);
    fireEvent.click(await screen.findByRole('button',{name:'Browse Selected folder'}));
    fireEvent.click(await screen.findByRole('button',{name:'Open Plan'}));
    expect(open).toHaveBeenCalledWith({sourceId:localId,path:'plan.md',revision:'b'.repeat(64),title:'Plan',line:1});expect(fetchCurrent).not.toHaveBeenCalled();expect(api.readDocument).not.toHaveBeenCalled();
    api.page.mockRejectedValueOnce(new CatalogError('stale'));fireEvent.click(screen.getByRole('button',{name:'Load more documents'}));
    expect((await screen.findByRole('alert')).textContent).toContain('catalog changed');expect(screen.queryByRole('button',{name:'Open Plan'})).toBeNull();expect(api.close).toHaveBeenCalledWith('cursor-a');
});
test('uncached provider fetch is explicit, wrong binding rejected and a late reply cannot open after refresh',async()=>{
    const api=backend();const cloud={...source,catalogId:'remote:selected',kind:'google_drive'};api.list.mockResolvedValue({operation:'list',sources:[cloud],providerFailure:null,localFailure:null});
    const uncached={...local,open:{kind:'selected_uncached' as const,sourceId:'selected',readerSourceId:remoteId,documentId:'opaque-id'}};
    api.page.mockResolvedValue({operation:'page',source:cloud,documents:[uncached],nextCursor:null});
    let finish!:(value:{jobId:string;readerSourceId:string;path:string;revision:string})=>void;
    const fetchCurrent=vi.fn(()=>new Promise<{jobId:string;readerSourceId:string;path:string;revision:string}>((resolve)=>{finish=resolve;}));const open=vi.fn();
    const props={backend:api,providers:{fetchCurrent},onOpen:open,onManage:vi.fn()};const view=render(<CatalogSidebar {...props}/>);
    fireEvent.click(await screen.findByRole('button',{name:'Browse Selected folder'}));expect(fetchCurrent).not.toHaveBeenCalled();
    fireEvent.click(await screen.findByRole('button',{name:'Fetch current Plan'}));
    await act(async()=>finish({jobId:'job',readerSourceId:`provider:${'c'.repeat(64)}`,path:`records/${'d'.repeat(64)}.md`,revision:'b'.repeat(64)}));
    expect((await screen.findByRole('alert')).textContent).toContain('binding could not be verified');expect(open).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button',{name:'Fetch current Plan'}));view.rerender(<CatalogSidebar {...props} refreshKey={1}/>);
    await act(async()=>finish({jobId:'job',readerSourceId:remoteId,path:`records/${'d'.repeat(64)}.md`,revision:'b'.repeat(64)}));expect(open).not.toHaveBeenCalled();await waitFor(()=>expect(api.list).toHaveBeenCalledTimes(2));
});
test('missing revisions, absolute paths and parent escapes cannot become canonical catalog opens',()=>{
    for(const path of ['/private/cache/plan.md','../plan.md','safe/../../plan.md','C:\\plan.md'])expect(()=>catalogReference({kind:'exact',sourceId:localId,path,revision:'b'.repeat(64)},'Plan')).toThrow();
    expect(()=>catalogReference({...local.open,revision:''} as typeof local.open,'Plan')).toThrow();
});
test('a single provider catalog row owns explicit search scope and drops disabled selections on refresh', async () => {
  const api=backend();const cloud={...source,catalogId:'remote:selected',kind:'google_drive'};api.list.mockResolvedValue({operation:'list',sources:[cloud],providerFailure:null,localFailure:null});
  const summary={sourceId:'selected',readerSourceId:remoteId,name:'Selected folder',kind:'google_drive' as const,enabled:true,selectedScope:{kind:'exact_documents' as const,count:1},capabilities:{read:true,list:true,changes:false,assets:false,guardedWrite:false},health:'ready' as const,coverage:{kind:'metadata_complete_selected' as const,observedAtMs:1}};
  const state=vi.fn(async()=>({sources:[summary],availability:[],pendingAdmissions:[],error:null}));const providers={state,fetchCurrent:vi.fn()};const select=vi.fn();const props={backend:api,providers,onOpen:vi.fn(),onManage:vi.fn(),onProviderSelectionChange:select};
  const view=render(<CatalogSidebar {...props} selectedProviderSources={[]}/>);
  const checkbox=await screen.findByRole('checkbox',{name:'Include Selected folder in search'});await waitFor(()=>expect((checkbox as HTMLInputElement).disabled).toBe(false));
  expect((checkbox as HTMLInputElement).checked).toBe(false);expect(api.page).not.toHaveBeenCalled();expect(providers.fetchCurrent).not.toHaveBeenCalled();fireEvent.click(checkbox);expect(select).toHaveBeenCalledWith([remoteId]);
  state.mockResolvedValue({sources:[{...summary,enabled:false}],availability:[],pendingAdmissions:[],error:null});view.rerender(<CatalogSidebar {...props} selectedProviderSources={[remoteId]} refreshKey={1}/>);
  await waitFor(()=>expect(select).toHaveBeenCalledWith([]));expect(screen.queryByRole('checkbox',{name:'Include Selected folder in search'})).toBeNull();
});


test('explicit cloud browsing owns its documents and a verified first saved copy enables search immediately', async () => {
  const api = backend();
  const cloud: CatalogSource = { ...source, catalogId: 'remote:selected', kind: 'dropbox', name: 'Dropbox notes', coverage: 'never_indexed' };
  const listed = { ...cloud, coverage: 'selected_metadata' };
  const uncached: CatalogDocument = { ...local, open: { kind: 'selected_uncached', sourceId: 'selected', readerSourceId: remoteId, documentId: 'opaque-id' } };
  api.list.mockResolvedValue({ operation: 'list', sources: [cloud, source], providerFailure: null, localFailure: null });
  api.page.mockResolvedValueOnce({ operation: 'page', source: cloud, documents: [], nextCursor: null })
    .mockResolvedValueOnce({ operation: 'page', source: listed, documents: [uncached], nextCursor: null });
  const state = vi.fn(async () => ({ sources: [{ sourceId: 'selected', readerSourceId: null, name: cloud.name,
    kind: 'dropbox' as const, enabled: true, selectedScope: { kind: 'scoped_root' as const, recursive: false },
    capabilities: { read: true, list: true, changes: true, assets: false, guardedWrite: false },
    health: 'ready' as const, coverage: { kind: 'never_indexed' as const } }], availability: [], pendingAdmissions: [], error: null }));
  const refresh = vi.fn(async () => ({ operation: 'refresh' as const, pages: 1, metadataComplete: true }));
  const fetchCurrent = vi.fn(async () => ({ jobId: 'job', readerSourceId: remoteId,
    path: `records/${'d'.repeat(64)}.md`, revision: 'b'.repeat(64) }));
  const selected = vi.fn();
  const open = vi.fn();
  render(<CatalogSidebar backend={api} providers={{ state, refresh, fetchCurrent }} onOpen={open}
    selectedProviderSources={[]} onProviderSelectionChange={selected} />);
  const group = await screen.findByRole('group', { name: 'Source Dropbox notes' });
  expect(within(group).queryByRole('checkbox')).toBeNull();
  expect(within(group).getByRole('button', { name: 'Open a file to enable search' })).toBeTruthy();
  expect(refresh).not.toHaveBeenCalled();
  fireEvent.click(within(group).getByRole('button', { name: 'Browse Dropbox notes' }));
  const file = await within(group).findByRole('button', { name: 'Fetch current Plan' });
  expect(refresh).toHaveBeenCalledExactlyOnceWith('selected');
  expect(within(screen.getByRole('group', { name: 'Source Selected folder' })).queryByRole('button', { name: 'Fetch current Plan' })).toBeNull();
  expect(fetchCurrent).not.toHaveBeenCalled();
  fireEvent.click(file);
  const checkbox = await within(group).findByRole('checkbox', { name: 'Include Dropbox notes in search' });
  expect((checkbox as HTMLInputElement).disabled).toBe(false);
  expect(within(group).getByRole('button', { name: 'Open Plan' })).toBeTruthy();
  expect(open).toHaveBeenCalledWith({ sourceId: remoteId, path: `records/${'d'.repeat(64)}.md`, revision: 'b'.repeat(64), title: 'Plan', line: 1 });
  fireEvent.click(checkbox);
  expect(selected).toHaveBeenCalledWith([remoteId]);
  expect(fetchCurrent).toHaveBeenCalledExactlyOnceWith('selected', 'opaque-id');
});
