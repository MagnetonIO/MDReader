import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, test } from 'vitest';
import { ProviderSourceSummary } from './ProviderSourceSummary';
import type { ProviderSource } from '../../platform/providers';
afterEach(cleanup);
const source: ProviderSource = { sourceId: 'selection', readerSourceId: null, name: 'Public account', kind: 'dropbox', enabled: true,
  selectedScope: { kind: 'scoped_root', path: '/', recursive: false }, health: 'ready', coverage: { kind: 'never_indexed' },
  capabilities: { read: true, list: true, assets: false, changes: true, guardedWrite: false } };

test('saved root scope and retained completed count remain distinct from account-wide or searchable bodies', () => {
  render(<ProviderSourceSummary source={{ ...source, metadataListing: { knownDocuments: 4,
    completeness: 'complete_selected', observedAtMs: 1_700_000_000_000, refreshPending: true } }} allowEditing />);
  expect(screen.getByText('/ · Direct files only')).toBeTruthy();
  expect(screen.getByText('4 Markdown files in this selection · More files remain to be checked')).toBeTruthy();
  expect(screen.getByText('Dropbox · Connected')).toBeTruthy();
  expect(screen.getByText('Read only for this connection')).toBeTruthy();
  expect(screen.getByText('Last checked')).toBeTruthy();
  expect(screen.queryByText(/indexed|account-wide|all files/i)).toBeNull();
});

test('unknown zero is not a complete empty census, and a partial count says found so far', () => {
  const view = render(<ProviderSourceSummary source={{ ...source, metadataListing: { knownDocuments: 0,
    completeness: 'never_observed', observedAtMs: null, refreshPending: false } }} allowEditing />);
  expect(screen.getByText('File list not loaded yet')).toBeTruthy();
  expect(screen.queryByText(/0 Markdown files/)).toBeNull();
  expect(screen.queryByText('Last checked')).toBeNull();
  view.rerender(<ProviderSourceSummary source={{ ...source, selectedScope: { kind: 'scoped_root', path: '/Notes', recursive: true },
    metadataListing: { knownDocuments: 10, completeness: 'partial', observedAtMs: 10, refreshPending: true } }} allowEditing />);
  expect(screen.getByText('/Notes · Includes subfolders')).toBeTruthy();
  expect(screen.getByText('10 Markdown files found so far · More files remain to be checked')).toBeTruthy();
});

test('opaque Drive folder IDs stay out of display and platform editing permission cannot expand the grant', () => {
  render(<ProviderSourceSummary source={{ ...source, kind: 'google_drive', selectedScope: { kind: 'drive_folder', folderId: 'opaque-private-id', recursive: false },
    capabilities: { ...source.capabilities, guardedWrite: true } }} allowEditing={false} />);
  expect(screen.getByText('Selected Drive folder · Direct files only')).toBeTruthy();
  expect(screen.getByText('Read only for this connection')).toBeTruthy();
  expect(screen.queryByText(/opaque-private-id/)).toBeNull();
});

test('legacy broad Drive selection is distinct from a modern My Drive folder grant', () => {
  const drive: ProviderSource = { ...source, kind: 'google_drive',
    selectedScope: { kind: 'drive_readonly_folder', folderId: 'root', accountWide: true } };
  const view = render(<ProviderSourceSummary source={drive} allowEditing />);
  expect(screen.getByText('Accessible Drive files · Broad saved selection')).toBeTruthy();
  expect(screen.queryByText(/My Drive/)).toBeNull();
  view.rerender(<ProviderSourceSummary source={{ ...drive,
    selectedScope: { kind: 'drive_folder', folderId: 'root', recursive: true } }} allowEditing />);
  expect(screen.getByText('My Drive · Includes subfolders')).toBeTruthy();
  expect(screen.queryByText(/Broad saved selection/)).toBeNull();
});
