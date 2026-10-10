import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import type { HitTarget } from '@mdreader/ui';
import type { ProviderReaderReference } from '../platform/providers';
import App from '../App';
import { nativeProvidersBackend } from '../platform/providers';
import { catalogBackend } from '../features/catalog/backend';
import { useReaderHistoryStore } from '../features/reader-history/store';
import { useTabsStore } from '../stores/tabs';
import { useSourcesStore } from '../stores/sources';
import { useSettingsStore } from '../stores/settings';
import { detectMac } from '../keymap/registry';
const fixture = vi.hoisted(() => ({ sourceId: `provider:${'a'.repeat(64)}`, path: `records/${'b'.repeat(64)}.md`, revision: 'c'.repeat(64), title: 'Exact public note', line: 41, heading: 'fn:1' }));
vi.mock('./persistence', async original => ({ ...await original<typeof import('./persistence')>(),
  isTauri: () => true, loadPersisted: async () => ({ session: null, settings: {} }), saveSettings: async () => {} }));
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ onCloseRequested: async () => () => {}, destroy: async () => {} }) }));
vi.mock('@mdreader/ui', async original => ({ ...await original<typeof import('@mdreader/ui')>(),
  SearchPalette: ({ onOpenHit, onAddSource }: { onAddSource(): void; onOpenHit(target: HitTarget, options: { background: boolean }): void }) =>
    <><button onClick={() => onOpenHit({ ...fixture, name: fixture.title }, { background: false })}>Choose exact search hit</button><button onClick={onAddSource}>Add source</button></> }));
vi.mock('../features/reader/CanonicalReader', () => ({ CanonicalReader: ({ reference }: { reference: ProviderReaderReference }) => <p>Exact reader: {reference.revision}</p> }));
vi.mock('./SettingsSheet', () => ({ SettingsSheet: ({ onOpenReadingLink, initialSection }: { initialSection: string; onOpenReadingLink(reference: ProviderReaderReference): void }) =>
  <><p>Settings section: {initialSection}</p><button onClick={() => onOpenReadingLink(fixture)}>Open exact reading link</button></> }));
beforeEach(() => {
  vi.spyOn(nativeProvidersBackend, 'state').mockResolvedValue({ availability: [], sources: [], pendingAdmissions: [], error: null });
  vi.spyOn(catalogBackend, 'list').mockResolvedValue({ operation: 'list', sources: [], providerFailure: null, localFailure: null });
  vi.spyOn(nativeProvidersBackend, 'readDocument').mockResolvedValue({ ...fixture, operation: 'read_document', markdown: '# Exact public body', lineStart: 1, lineEnd: 1 });
  vi.spyOn(nativeProvidersBackend, 'readCurrentDocument').mockResolvedValue({ ...fixture, revision: 'd'.repeat(64), operation: 'read_document', markdown: '# Newer body', lineStart: 1, lineEnd: 1 });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); useReaderHistoryStore.getState().reset(); useTabsStore.getState().reset(); useSourcesStore.getState().reset(); useSettingsStore.getState().reset(); });
test.each(['search', 'reading link'] as const)('shell %s preserves its exact revision and location instead of current-head reopen', async kind => {
  render(<App />);
  await waitFor(() => expect(catalogBackend.list).toHaveBeenCalled());
  if (kind === 'search') {
    const mac = detectMac(); fireEvent.keyDown(window, { key: 'k', metaKey: mac, ctrlKey: !mac });
    fireEvent.click(await screen.findByRole('button', { name: 'Choose exact search hit' }));
  } else {
    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Open exact reading link' }));
  }
  await screen.findByText(`Exact reader: ${fixture.revision}`);
  const state = useReaderHistoryStore.getState(), tab = state.data.tabs.find(value => value.id === state.data.activeTabId)!;
  expect(state.selectionIntent).toBe('exact');
  expect(tab.entries[tab.index]?.reference).toMatchObject({ sourceId: fixture.sourceId, path: fixture.path, revision: fixture.revision });
  expect(tab.entries[tab.index]?.position).toMatchObject({ revision: fixture.revision, line: 41 });
  if (kind === 'reading link') expect(tab.entries[tab.index]?.heading).toBe(fixture.heading);
  expect(nativeProvidersBackend.readCurrentDocument).not.toHaveBeenCalled();
});


test('search Add source closes the palette and opens host Library & sources settings', async () => {
  render(<App />);
  await waitFor(() => expect(catalogBackend.list).toHaveBeenCalled());
  const mac = detectMac(); fireEvent.keyDown(window, { key: 'k', metaKey: mac, ctrlKey: !mac });
  fireEvent.click(await screen.findByRole('button', { name: 'Add source' }));
  expect(await screen.findByText('Settings section: library')).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Choose exact search hit' })).toBeNull();
  expect(nativeProvidersBackend.readCurrentDocument).not.toHaveBeenCalled();
});
