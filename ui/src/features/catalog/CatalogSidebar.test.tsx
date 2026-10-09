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
test('sidebar omits search preferences but still drops native-disabled selected sources on refresh', async () => {
  const api=backend();const cloud={...source,catalogId:'remote:selected',kind:'google_drive'};api.list.mockResolvedValue({operation:'list',sources:[cloud],providerFailure:null,localFailure:null});
  const summary={sourceId:'selected',readerSourceId:remoteId,name:'Selected folder',kind:'google_drive' as const,enabled:true,selectedScope:{kind:'exact_documents' as const,count:1},capabilities:{read:true,list:true,changes:false,assets:false,guardedWrite:false},health:'ready' as const,coverage:{kind:'metadata_complete_selected' as const,observedAtMs:1}};
  const state=vi.fn(async()=>({sources:[summary],availability:[],pendingAdmissions:[],error:null}));const providers={state,fetchCurrent:vi.fn()};const select=vi.fn();const props={backend:api,providers,onOpen:vi.fn(),onManage:vi.fn(),onProviderSelectionChange:select};
  const view=render(<CatalogSidebar {...props} selectedProviderSources={[]}/>);
  await screen.findByRole('button', { name: 'Browse Selected folder' });
  expect(screen.queryByRole('checkbox')).toBeNull();expect(api.page).not.toHaveBeenCalled();expect(providers.fetchCurrent).not.toHaveBeenCalled();expect(select).not.toHaveBeenCalled();
  state.mockResolvedValue({sources:[{...summary,enabled:false}],availability:[],pendingAdmissions:[],error:null});view.rerender(<CatalogSidebar {...props} selectedProviderSources={[remoteId]} refreshKey={1}/>);
  await waitFor(()=>expect(select).toHaveBeenCalledWith([]));expect(screen.queryByRole('checkbox',{name:'Include Selected folder in search'})).toBeNull();
});


test('compact cloud browsing retains exact first saved receipts without search controls or evidence clutter', async () => {
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
  expect(within(group).getByRole('button', { name: 'Refresh Dropbox notes documents' }).getAttribute('title')).toBe('Refresh documents');
  expect(refresh).not.toHaveBeenCalled();
  fireEvent.click(within(group).getByRole('button', { name: 'Browse Dropbox notes' }));
  const file = await within(group).findByRole('button', { name: 'Fetch current Plan' });
  expect(refresh).toHaveBeenCalledExactlyOnceWith('selected');
  expect(within(screen.getByRole('group', { name: 'Source Selected folder' })).queryByRole('button', { name: 'Fetch current Plan' })).toBeNull();
  expect(fetchCurrent).not.toHaveBeenCalled();
  fireEvent.click(file);
  await within(group).findByRole('button', { name: 'Open Plan' });
  expect(within(group).queryByRole('checkbox')).toBeNull();
  expect(within(group).queryByText('About this source')).toBeNull();
  expect(within(group).queryByText(/documents loaded/)).toBeNull();
  expect(open).toHaveBeenCalledWith({ sourceId: remoteId, path: `records/${'d'.repeat(64)}.md`, revision: 'b'.repeat(64), title: 'Plan', line: 1 });
  expect(selected).not.toHaveBeenCalled();
  expect(fetchCurrent).toHaveBeenCalledExactlyOnceWith('selected', 'opaque-id');
});


test('duplicate file names show paths and expose distinct accessible actions', async () => {
  const api = backend();
  const first: CatalogDocument = { ...local, name: 'README.md', displayPath: 'project-a/README.md',
    open: { kind: 'exact', sourceId: localId, path: 'project-a/README.md', revision: 'b'.repeat(64) } };
  const second: CatalogDocument = { ...first, displayPath: 'project-b/README.md',
    open: { ...first.open, kind: 'exact', sourceId: localId, path: 'project-b/README.md', revision: 'c'.repeat(64) } };
  api.page.mockResolvedValue({ operation: 'page', source, documents: [first, second], nextCursor: null });
  const open = vi.fn();
  render(<CatalogSidebar backend={api} onOpen={open} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Browse Selected folder' }));
  const firstButton = await screen.findByRole('button', { name: 'Open README.md — project-a/README.md' });
  expect(screen.getByText('project-a/README.md').className).toBe('reader-catalog__document-path');
  expect(screen.getByText('project-b/README.md').className).toBe('reader-catalog__document-path');
  fireEvent.click(firstButton);
  expect(open).toHaveBeenLastCalledWith({ sourceId: localId, path: 'project-a/README.md', revision: 'b'.repeat(64), title: 'README.md', line: 1 });
  fireEvent.click(screen.getByRole('button', { name: 'Open README.md — project-b/README.md' }));
  expect(open).toHaveBeenLastCalledWith({ sourceId: localId, path: 'project-b/README.md', revision: 'c'.repeat(64), title: 'README.md', line: 1 });
});

test('partial cold browsing and explicit continuation retain published files without automatically fetching further pages or bodies', async () => {
  const api = backend();
  const cloud: CatalogSource = { ...source, catalogId: 'remote:selected', kind: 'google_drive', coverage: 'never_indexed' };
  const partial = { ...cloud, coverage: 'partial_selected_metadata' };
  const first: CatalogDocument = { ...local, open: { kind: 'selected_uncached', sourceId: 'selected', readerSourceId: remoteId, documentId: 'first-id' } };
  const next: CatalogDocument = { ...first, name: 'Next.md', displayPath: 'Next.md', open: { ...first.open, kind: 'selected_uncached', sourceId: 'selected', readerSourceId: remoteId, documentId: 'next-id' } };
  api.list.mockResolvedValue({ operation: 'list', sources: [cloud], providerFailure: null, localFailure: null });
  api.page.mockResolvedValueOnce({ operation: 'page', source: cloud, documents: [], nextCursor: null })
    .mockResolvedValueOnce({ operation: 'page', source: partial, documents: [first], nextCursor: null })
    .mockResolvedValueOnce({ operation: 'page', source: partial, documents: [next], nextCursor: null })
    .mockResolvedValueOnce({ operation: 'page', source: { ...cloud, coverage: 'selected_metadata' }, documents: [next], nextCursor: null });
  let finish!: (value: { operation: 'refresh'; pages: number; metadataComplete: boolean }) => void;
  const refresh = vi.fn().mockResolvedValueOnce({ operation: 'refresh', pages: 8, metadataComplete: false })
    .mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }))
    .mockResolvedValueOnce({ operation: 'refresh', pages: 1, metadataComplete: true });
  const fetchCurrent = vi.fn();
  render(<CatalogSidebar backend={api} providers={{ refresh, fetchCurrent }} onOpen={vi.fn()} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Browse Selected folder' }));
  const continueLoading = await screen.findByRole('button', { name: 'Continue loading Selected folder documents' });
  expect(screen.getByRole('button', { name: 'Fetch current Plan' })).toBeTruthy();
  expect(refresh).toHaveBeenCalledExactlyOnceWith('selected');
  fireEvent.click(continueLoading);
  await waitFor(() => expect(refresh).toHaveBeenCalledTimes(2));
  expect(screen.getByRole('button', { name: 'Fetch current Plan' })).toBeTruthy();
  await act(async () => finish({ operation: 'refresh', pages: 8, metadataComplete: false }));
  await screen.findByRole('button', { name: 'Fetch current Next.md' });
  expect(screen.getByRole('button', { name: 'Fetch current Plan' })).toBeTruthy();
  expect(refresh).toHaveBeenCalledTimes(2);
  fireEvent.click(screen.getByRole('button', { name: 'Continue loading Selected folder documents' }));
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Continue loading Selected folder documents' })).toBeNull());
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Fetch current Plan' })).toBeNull());
  expect(refresh).toHaveBeenCalledTimes(3); expect(fetchCurrent).not.toHaveBeenCalled();
});

test('a late partial refresh cannot publish its continuation into another source generation', async () => {
  const api = backend();
  const cloud: CatalogSource = { ...source, catalogId: 'remote:selected', kind: 'google_drive', coverage: 'selected_metadata' };
  api.list.mockResolvedValue({ operation: 'list', sources: [cloud], providerFailure: null, localFailure: null });
  api.page.mockResolvedValue({ operation: 'page', source: cloud, documents: [local], nextCursor: null });
  let finish!: (value: { operation: 'refresh'; pages: number; metadataComplete: boolean }) => void;
  const refresh = vi.fn(() => new Promise<{ operation: 'refresh'; pages: number; metadataComplete: boolean }>(resolve => { finish = resolve; }));
  const providers = { refresh, fetchCurrent: vi.fn() };
  const view = render(<CatalogSidebar backend={api} providers={providers} onOpen={vi.fn()} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Browse Selected folder' }));
  await screen.findByRole('button', { name: 'Open Plan' });
  fireEvent.click(screen.getByRole('button', { name: 'Refresh Selected folder documents' }));
  await waitFor(() => expect(refresh).toHaveBeenCalledExactlyOnceWith('selected'));
  view.rerender(<CatalogSidebar backend={api} providers={providers} onOpen={vi.fn()} refreshKey="another-library" />);
  await act(async () => finish({ operation: 'refresh', pages: 8, metadataComplete: false }));
  await waitFor(() => expect(api.list).toHaveBeenCalledTimes(2));
  expect(screen.queryByRole('button', { name: 'Continue loading Selected folder documents' })).toBeNull();
  expect(api.page).toHaveBeenCalledTimes(1); expect(providers.fetchCurrent).not.toHaveBeenCalled();
});
