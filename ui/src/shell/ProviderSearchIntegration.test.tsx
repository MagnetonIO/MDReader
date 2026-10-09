import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import type { SearchHit } from '@mdreader/ui';
const fixture=vi.hoisted(()=>({sourceId:'provider:'+'a'.repeat(64),path:'records/'+'b'.repeat(64)+'.md',revision:'c'.repeat(64),selection:()=>[] as readonly string[]}));
vi.mock('./persistence',async(importOriginal)=>({ ...await importOriginal<typeof import('./persistence')>(),isTauri:()=>true,loadPersisted:async()=>({session:null,settings:{}}),saveSettings:async()=>{} }));
vi.mock('@tauri-apps/api/window',()=>({getCurrentWindow:()=>({onCloseRequested:async()=>()=>{},destroy:async()=>{}})}));
vi.mock('../platform/fs.js',async(importOriginal)=>{
  const original=await importOriginal<typeof import('../platform/fs.js')>();
  return {...original,onFsChanged:async()=>()=>{},onWatchError:async()=>()=>{},makeSearchAdapter:(_generation:()=>number,selection:()=>readonly string[])=>{
    fixture.selection=selection;
    const hit:SearchHit={chunkId:'chunk',documentId:'doc',sourceId:fixture.sourceId,revision:fixture.revision,path:fixture.path,filePath:fixture.path,name:'Provider plan',headingPath:['Plan'],startLine:3,endLine:4,score:1,rrfScore:1,boosts:{title:0,path:0,recency:0,graph:0},lexRank:1,semRank:null,snippet:'quartz evidence',mtime:1,explain:null};
    return {search:async(_query:string,_opts:unknown,onPhase:(phase:'lexical'|'final',hits:SearchHit[])=>void)=>{onPhase('final',[hit]);return {hits:[hit],semanticUnavailable:null};},related:async()=>[]};
  }};
});
import { AppShell } from './AppShell';
import { nativeProvidersBackend } from '../platform/providers';
import { catalogBackend } from '../features/catalog/backend';
import { useReaderHistoryStore } from '../features/reader-history/store';
import * as settings from '../platform/settings';
import { useTabsStore } from '../stores/tabs';
import { useSourcesStore } from '../stores/sources';
import { useSettingsStore } from '../stores/settings';
import { detectMac } from '../keymap/registry';
afterEach(()=>{cleanup();vi.restoreAllMocks();vi.unstubAllGlobals();useTabsStore.getState().reset();useSourcesStore.getState().reset();useSettingsStore.getState().reset();useReaderHistoryStore.getState().reset();});
test('explicit provider search opens canonical revision in the shared reader without constructing a local tab or folder grant',async()=>{
  vi.spyOn(nativeProvidersBackend,'state').mockResolvedValue({availability:[],pendingAdmissions:[],error:null,sources:[{sourceId:'service-source',readerSourceId:fixture.sourceId,name:'Selected plans',kind:'google_drive',enabled:true,selectedScope:{kind:'exact_documents',count:1},capabilities:{read:true,list:true,changes:true,guardedWrite:false,assets:false},health:'ready',coverage:{kind:'metadata_complete_selected',observedAtMs:1}}]});
  vi.spyOn(catalogBackend,'list').mockResolvedValue({ operation:'list', localFailure:null, providerFailure:null, sources:[{
    catalogId:'remote:service-source', name:'Selected plans', kind:'google_drive', enabled:true,
    coverage:'metadata_complete_selected', observedAtMs:1, failure:null, librarySourceId:null,
  }] });
  const read=vi.spyOn(nativeProvidersBackend,'readDocument').mockResolvedValue({operation:'read_document',sourceId:fixture.sourceId,path:fixture.path,revision:fixture.revision,markdown:'# Canonical cloud reading\n\nquartz evidence',lineStart:1,lineEnd:3});
  const watch=vi.spyOn(settings,'watchLibrarySource'); const grant=vi.spyOn(settings,'grantLibrarySource');
  Element.prototype.scrollIntoView = vi.fn();
  vi.stubGlobal('ResizeObserver', class { observe() {} unobserve() {} disconnect() {} });
  vi.spyOn(settings,'listLibrarySources').mockResolvedValue({version:1,sources:[],cloud_sources:[],connected_catdb:null,connected_catdb_rediscovery_required:false,cleanup:{kind:'complete'}});
  render(<AppShell />);
  fireEvent.click(await screen.findByRole('checkbox',{name:'Include Selected plans in search'}));
  expect(fixture.selection()).toEqual([fixture.sourceId]);
  act(()=>fireEvent.keyDown(window,{key:'k',metaKey:detectMac(),ctrlKey:!detectMac()}));
  fireEvent.change(await screen.findByRole('combobox'),{target:{value:'quartz'}});
  fireEvent.click(await screen.findByRole('option'));
  expect(await screen.findByRole('heading',{name:/Canonical cloud reading/})).toBeTruthy();
  expect(read).toHaveBeenCalledWith(expect.objectContaining({sourceId:fixture.sourceId,path:fixture.path,revision:fixture.revision}));
  const history = useReaderHistoryStore.getState().data;
  const active = history.tabs.find((tab) => tab.id === history.activeTabId)!;
  expect(active.entries[active.index]).toMatchObject({
    reference: { kind:'provider', sourceId:fixture.sourceId, path:fixture.path, revision:fixture.revision },
    position: { line:3, revision:fixture.revision },
  });
  expect(useTabsStore.getState().tabs).toHaveLength(0); expect(useSourcesStore.getState().root).toBeNull();
  expect(watch).not.toHaveBeenCalled(); expect(grant).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button',{name:'Back to sources'}));
  await waitFor(()=>expect(screen.queryByRole('region',{name:'Cloud document'})).toBeNull());
  expect(screen.queryByRole('dialog',{name:'Settings'})).toBeNull();
});
