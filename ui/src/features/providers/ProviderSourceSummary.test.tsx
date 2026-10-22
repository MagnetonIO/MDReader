import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, test } from 'vitest';
import { ProviderSourceSummary } from './ProviderSourceSummary';
import { sourceDetails } from './source-presentation';
import type { ProviderSource } from '../../platform/providers';
afterEach(cleanup);
const source: ProviderSource = { sourceId: 'selection', readerSourceId: null, name: 'Public account', kind: 'dropbox', enabled: true,
  selectedScope: { kind: 'scoped_root', path: '/', recursive: false }, health: 'ready', coverage: { kind: 'never_indexed' },
  capabilities: { read: true, list: true, assets: false, changes: true, guardedWrite: false } };
const details = () => document.querySelector('.provider-source__details')!.textContent;

test('saved root scope and retained completed count remain distinct from account-wide or searchable bodies', () => {
  const observed = new Date(2026, 9, 21, 18, 53);
  render(<ProviderSourceSummary source={{ ...source, metadataListing: { knownDocuments: 4,
    completeness: 'complete_selected', observedAtMs: observed.getTime(), refreshPending: true } }} allowEditing />);
  expect(screen.getByRole('heading', { name: 'Public account' })).toBeTruthy();
  expect(screen.getByText('Dropbox')).toBeTruthy();
  expect(screen.getByText('Connected')).toBeTruthy();
  expect(details()).toMatch(/^\/ · no subfolders · read-only · 4 files · more files pending · checked /);
  expect(document.querySelector('.provider-source__details')!.getAttribute('title')).toContain('Last checked');
  expect(screen.queryByText(/indexed|account-wide|all files/i)).toBeNull();
});

test('unknown zero is not a complete empty census, and a partial count says so far', () => {
  const view = render(<ProviderSourceSummary source={{ ...source, metadataListing: { knownDocuments: 0,
    completeness: 'never_observed', observedAtMs: null, refreshPending: false } }} allowEditing />);
  expect(details()).toBe('/ · no subfolders · read-only · file list not loaded');
  expect(details()).not.toMatch(/0 files|checked/);
  view.rerender(<ProviderSourceSummary source={{ ...source, selectedScope: { kind: 'scoped_root', path: '/Notes', recursive: true },
    metadataListing: { knownDocuments: 10, completeness: 'partial', observedAtMs: null, refreshPending: true } }} allowEditing />);
  expect(details()).toBe('/Notes · subfolders · read-only · 10 files so far · more files pending');
});

test('checked time shows a clock time today and a date otherwise', () => {
  const now = new Date(2026, 9, 21, 20, 0);
  const listing = (at: Date) => ({ ...source, metadataListing: { knownDocuments: 1, completeness: 'complete_selected' as const, observedAtMs: at.getTime(), refreshPending: false } });
  const today = sourceDetails(listing(new Date(2026, 9, 21, 18, 53)), false, now).at(-1)!;
  expect(today).toBe(`checked ${new Date(2026, 9, 21, 18, 53).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })}`);
  expect(sourceDetails(listing(new Date(2026, 9, 1, 9, 0)), false, now)).toContain('1 file');
  expect(sourceDetails(listing(new Date(2026, 9, 1, 9, 0)), false, now).at(-1)).toBe(`checked ${new Date(2026, 9, 1).toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}`);
});

test('opaque Drive folder IDs stay out of display and platform editing permission cannot expand the grant', () => {
  const editable = { ...source, kind: 'google_drive' as const, selectedScope: { kind: 'drive_folder' as const, folderId: 'opaque-private-id', recursive: false },
    capabilities: { ...source.capabilities, guardedWrite: true } };
  const view = render(<ProviderSourceSummary source={editable} allowEditing={false} />);
  expect(details()).toBe('Drive folder · no subfolders · read-only · file list not loaded');
  expect(screen.queryByText(/opaque-private-id/)).toBeNull();
  view.rerender(<ProviderSourceSummary source={editable} allowEditing />);
  expect(details()).toContain('can edit');
});

test('legacy broad Drive selection is distinct from a modern My Drive folder grant', () => {
  const drive: ProviderSource = { ...source, kind: 'google_drive',
    selectedScope: { kind: 'drive_readonly_folder', folderId: 'root', accountWide: true } };
  const view = render(<ProviderSourceSummary source={drive} allowEditing />);
  expect(details()).toMatch(/^Accessible Drive files · /);
  expect(details()).not.toMatch(/My Drive/);
  view.rerender(<ProviderSourceSummary source={{ ...drive,
    selectedScope: { kind: 'drive_folder', folderId: 'root', recursive: true } }} allowEditing />);
  expect(details()).toMatch(/^My Drive · subfolders · /);
});

test('status pill distinguishes paused, reconnect needed and imported connections', () => {
  const view = render(<ProviderSourceSummary source={{ ...source, enabled: false, health: 'paused' }} allowEditing />);
  expect(screen.getByText('Paused')).toBeTruthy();
  view.rerender(<ProviderSourceSummary source={{ ...source, health: 'reconnect_required' }} allowEditing />);
  expect(screen.getByText('Reconnect needed')).toBeTruthy();
  view.rerender(<ProviderSourceSummary source={{ ...source, selectedScope: { kind: 'imported_legacy_broad' } }} allowEditing />);
  expect(screen.getByText('Reconnect needed')).toBeTruthy();
  view.rerender(<ProviderSourceSummary source={{ ...source, health: 'unavailable' }} allowEditing />);
  expect(screen.getByText('Unavailable')).toBeTruthy();
});
