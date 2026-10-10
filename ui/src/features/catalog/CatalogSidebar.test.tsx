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
    api.page.mockRejectedValueOnce(new CatalogError('stale'));fireEvent.click(screen.getByRole('button',{name:'Show more'}));
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

test('explicit cold browsing loads sequential batches automatically while preserving published files and never fetching bodies', async () => {
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
  let finishSecond!: (value: { operation: 'refresh'; pages: number; metadataComplete: boolean }) => void;
  let finishThird!: (value: { operation: 'refresh'; pages: number; metadataComplete: boolean }) => void;
  const refresh = vi.fn().mockResolvedValueOnce({ operation: 'refresh', pages: 8, metadataComplete: false })
    .mockImplementationOnce(() => new Promise(resolve => { finishSecond = resolve; }))
    .mockImplementationOnce(() => new Promise(resolve => { finishThird = resolve; }));
  const fetchCurrent = vi.fn();
  render(<CatalogSidebar backend={api} providers={{ refresh, fetchCurrent }} onOpen={vi.fn()} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Browse Selected folder' }));
  await screen.findByRole('button', { name: 'Fetch current Plan' });
  await waitFor(() => expect(refresh).toHaveBeenCalledTimes(2));
  expect(screen.queryByRole('button', { name: 'Continue loading Selected folder documents' })).toBeNull();
  expect(screen.getByText(/Finding Markdown files/)).toBeTruthy();
  await act(async () => finishSecond({ operation: 'refresh', pages: 8, metadataComplete: false }));
  await screen.findByRole('button', { name: 'Fetch current Next.md' });
  expect(screen.queryByRole('button', { name: 'Fetch current Plan' })).toBeNull();
  await waitFor(() => expect(refresh).toHaveBeenCalledTimes(3));
  await act(async () => finishThird({ operation: 'refresh', pages: 1, metadataComplete: true }));
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Stop refreshing Selected folder documents' })).toBeNull());
  expect(screen.queryByRole('button', { name: 'Fetch current Plan' })).toBeNull();
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
  await waitFor(() => expect(refresh).toHaveBeenCalledExactlyOnceWith('selected'));
  view.rerender(<CatalogSidebar backend={api} providers={providers} onOpen={vi.fn()} refreshKey="another-library" />);
  await act(async () => finish({ operation: 'refresh', pages: 8, metadataComplete: false }));
  await waitFor(() => expect(api.list).toHaveBeenCalledTimes(2));
  expect(screen.queryByRole('button', { name: 'Continue loading Selected folder documents' })).toBeNull();
  expect(api.page).toHaveBeenCalledTimes(1); expect(providers.fetchCurrent).not.toHaveBeenCalled();
});

test('cached cloud files appear and open before a delayed automatic delta finishes, with no body fetch', async () => {
  const api = backend();
  const cloud: CatalogSource = { ...source, catalogId: 'remote:selected', kind: 'dropbox', coverage: 'selected_metadata' };
  const cached: CatalogDocument = { ...local, open: { kind: 'exact', sourceId: remoteId, path: `records/${'d'.repeat(64)}.md`, revision: 'b'.repeat(64) } };
  api.list.mockResolvedValue({ operation: 'list', sources: [cloud], providerFailure: null, localFailure: null });
  api.page.mockResolvedValue({ operation: 'page', source: cloud, documents: [cached], nextCursor: null });
  let finish!: (value: { operation: 'refresh'; pages: number; metadataComplete: boolean }) => void;
  const refresh = vi.fn(() => new Promise<{ operation: 'refresh'; pages: number; metadataComplete: boolean }>(resolve => { finish = resolve; }));
  const fetchCurrent = vi.fn(); const open = vi.fn();
  render(<CatalogSidebar backend={api} providers={{ refresh, fetchCurrent }} onOpen={open} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Browse Selected folder' }));
  const button = await screen.findByRole('button', { name: 'Open Plan' });
  await waitFor(() => expect(refresh).toHaveBeenCalledTimes(1));
  expect((button as HTMLButtonElement).disabled).toBe(false);
  fireEvent.click(button);
  expect(open).toHaveBeenCalledExactlyOnceWith(catalogReference(cached.open, cached.name));
  expect(fetchCurrent).not.toHaveBeenCalled();
  expect(api.readDocument).not.toHaveBeenCalled();
  expect(api.page).toHaveBeenCalledTimes(1);
  await act(async () => finish({ operation: 'refresh', pages: 1, metadataComplete: true }));
  await waitFor(() => expect(api.page).toHaveBeenCalledTimes(2));
  expect(screen.getByRole('button', { name: 'Open Plan' })).toBe(button);
});

test('stopping a cold metadata load finishes its one in-flight batch and keeps a resumable partial list', async () => {
  const api = backend();
  const cloud: CatalogSource = { ...source, catalogId: 'remote:selected', kind: 'google_drive', coverage: 'never_indexed' };
  api.list.mockResolvedValue({ operation: 'list', sources: [cloud], providerFailure: null, localFailure: null });
  api.page.mockResolvedValue({ operation: 'page', source: cloud, documents: [], nextCursor: null });
  let finish!: (value: { operation: 'refresh'; pages: number; metadataComplete: boolean }) => void;
  const refresh = vi.fn(() => new Promise<{ operation: 'refresh'; pages: number; metadataComplete: boolean }>(resolve => { finish = resolve; }));
  const fetchCurrent = vi.fn();
  render(<CatalogSidebar backend={api} providers={{ refresh, fetchCurrent }} onOpen={vi.fn()} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Browse Selected folder' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Stop refreshing Selected folder documents' }));
  expect(screen.getByText('Stopping after the current batch…')).toBeTruthy();
  expect(screen.queryByText('Refresh to find Markdown files.')).toBeNull();
  await act(async () => finish({ operation: 'refresh', pages: 8, metadataComplete: false }));
  await screen.findByRole('button', { name: 'Continue loading Selected folder documents' });
  expect(screen.getByText('Loading paused. Resume to keep looking for Markdown files.')).toBeTruthy();
  expect(refresh).toHaveBeenCalledExactlyOnceWith('selected'); expect(fetchCurrent).not.toHaveBeenCalled();
});

test('cloud first page and Show more preserve the server recency order and request ten at a time', async () => {
  const api = backend();
  const cloud = { ...source, catalogId: 'remote:selected', kind: 'dropbox', coverage: 'selected_metadata' };
  const documents = Array.from({ length: 20 }, (_, index) => ({ ...local, name: `Note-${index}.md`, modifiedAtMs: 100 - index,
    open: { kind: 'exact' as const, sourceId: remoteId, path: `records/${index}.md`, revision: 'b'.repeat(64) } }));
  api.list.mockResolvedValue({ operation: 'list', sources: [cloud], providerFailure: null, localFailure: null });
  api.page.mockResolvedValueOnce({ operation: 'page', source: cloud, documents: documents.slice(0, 10), nextCursor: 'next-ten' })
    .mockResolvedValueOnce({ operation: 'page', source: cloud, documents: documents.slice(10), nextCursor: null });
  render(<CatalogSidebar backend={api} providers={{ fetchCurrent: vi.fn() }} onOpen={vi.fn()} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Browse Selected folder' }));
  await screen.findByRole('button', { name: 'Open Note-9.md' });
  expect(screen.queryByRole('button', { name: 'Open Note-10.md' })).toBeNull();
  expect(api.page).toHaveBeenLastCalledWith('remote:selected', null, 10);
  fireEvent.click(screen.getByRole('button', { name: 'Show more' }));
  await screen.findByRole('button', { name: 'Open Note-19.md' });
  expect(api.page).toHaveBeenLastCalledWith('remote:selected', 'next-ten', 10);
  expect(screen.getAllByRole('button', { name: /^Open Note-/ }).map(button => button.textContent)).toEqual(documents.map(document => document.name));
});

test.each([false, true])('opening a discovered cloud file resumes only an incomplete scan (finished batch=%s)', async finished => {
  const api = backend();
  const cloud = { ...source, catalogId: 'remote:selected', kind: 'google_drive', coverage: 'never_indexed' };
  const partial = { ...cloud, coverage: 'partial_selected_metadata' };
  const file: CatalogDocument = { ...local, open: { kind: 'selected_uncached', sourceId: 'selected', readerSourceId: remoteId, documentId: 'first-file' } };
  const exact: CatalogDocument = { ...file, open: { kind: 'exact', sourceId: remoteId, path: `records/${'d'.repeat(64)}.md`, revision: 'b'.repeat(64) }, observation: 'cached_provider_revision' };
  api.list.mockResolvedValue({ operation: 'list', sources: [cloud], providerFailure: null, localFailure: null });
  api.page.mockResolvedValueOnce({ operation: 'page', source: cloud, documents: [], nextCursor: null })
    .mockResolvedValueOnce({ operation: 'page', source: partial, documents: [file], nextCursor: null })
    .mockResolvedValueOnce({ operation: 'page', source: { ...cloud, coverage: 'selected_metadata' }, documents: [exact], nextCursor: null });
  let finishBatch!: (value: { operation: 'refresh'; pages: number; metadataComplete: boolean }) => void;
  const refresh = vi.fn().mockResolvedValueOnce({ operation: 'refresh', pages: 1, metadataComplete: false })
    .mockImplementationOnce(() => new Promise(resolve => { finishBatch = resolve; }))
    .mockResolvedValueOnce({ operation: 'refresh', pages: 1, metadataComplete: true });
  const fetchCurrent = vi.fn(async () => ({ jobId: 'public-job', readerSourceId: remoteId, path: exact.open.kind === 'exact' ? exact.open.path : '', revision: 'b'.repeat(64) }));
  const open = vi.fn();
  render(<CatalogSidebar backend={api} providers={{ refresh, fetchCurrent }} onOpen={open} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Browse Selected folder' }));
  const found = await screen.findByRole('button', { name: 'Fetch current Plan' });
  await waitFor(() => expect(refresh).toHaveBeenCalledTimes(2));
  expect((found as HTMLButtonElement).disabled).toBe(false);
  fireEvent.click(found);
  expect(fetchCurrent).not.toHaveBeenCalled();
  await act(async () => finishBatch({ operation: 'refresh', pages: 1, metadataComplete: finished }));
  await screen.findByRole('button', { name: 'Open Plan' });
  await waitFor(() => expect(api.page).toHaveBeenCalledTimes(3));
  expect(refresh).toHaveBeenCalledTimes(finished ? 2 : 3);
  expect(fetchCurrent).toHaveBeenCalledExactlyOnceWith('selected', 'first-file');
  expect(open).toHaveBeenCalledExactlyOnceWith(catalogReference(exact.open, 'Plan'));
  expect(api.page.mock.calls.every(([, , limit]) => limit === 10)).toBe(true);
});


test('local index completion is reobserved without resetting a cloud catalog or fetching provider bodies', async () => {
  const api = backend();
  const initial = { ...source, coverage: 'not_indexed' };
  api.list.mockResolvedValueOnce({ operation: 'list', sources: [initial], providerFailure: null, localFailure: null });
  api.page.mockResolvedValueOnce({ operation: 'page', source: initial, documents: [], nextCursor: null });
  const refresh = vi.fn(); const fetchCurrent = vi.fn();
  const props = { backend: api, providers: { refresh, fetchCurrent }, onOpen: vi.fn() };
  const view = render(<CatalogSidebar {...props} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Browse Selected folder' }));
  await waitFor(() => expect(api.page).toHaveBeenCalledTimes(1));
  expect(screen.queryByText(/Not indexed yet|No text index observed/)).toBeNull();
  view.rerender(<CatalogSidebar {...props} observationKey={1} />);
  await screen.findByRole('button', { name: 'Open Plan' });
  expect(api.list).toHaveBeenCalledTimes(2);
  expect(refresh).not.toHaveBeenCalled(); expect(fetchCurrent).not.toHaveBeenCalled();
  expect(api.readDocument).not.toHaveBeenCalled();
  api.list.mockResolvedValue({ operation: 'list', sources: [source], providerFailure: null, localFailure: null });
  fireEvent(window, new Event('focus'));
  await waitFor(() => expect(api.list).toHaveBeenCalledTimes(3));
  expect(api.page).toHaveBeenCalledTimes(2);
  expect(screen.getByRole('button', { name: 'Open Plan' })).toBeTruthy();
});


test('a late local observation cannot republish sources after the catalog scope changes', async () => {
  const api = backend();
  const props = { backend: api, providers: { fetchCurrent: vi.fn() }, onOpen: vi.fn() };
  const view = render(<CatalogSidebar {...props} />);
  await screen.findByRole('button', { name: 'Browse Selected folder' });
  let finish!: (value: Awaited<ReturnType<typeof api.list>>) => void;
  api.list.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  fireEvent(window, new Event('focus'));
  await waitFor(() => expect(api.list).toHaveBeenCalledTimes(2));
  view.rerender(<CatalogSidebar {...props} refreshKey={1} />);
  await screen.findByRole('button', { name: 'Browse Selected folder' });
  await act(async () => finish({ operation: 'list', sources: [{ ...source, name: 'Old grant observation' }], providerFailure: null, localFailure: null }));
  expect(screen.queryByRole('button', { name: 'Browse Old grant observation' })).toBeNull();
  expect(api.page).not.toHaveBeenCalled();
  expect(props.providers.fetchCurrent).not.toHaveBeenCalled();
});
