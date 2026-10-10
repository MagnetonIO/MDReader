import { act, cleanup, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { nativeProvidersBackend, type ProviderCoverage, type ProviderSource } from '../../platform/providers';
import { useProviderEditing } from './useProviderEditing';
import { providerEditBackend } from './backend';

const reference = { sourceId: `provider:${'a'.repeat(64)}`, path: 'records/public.md', revision: 'b'.repeat(64) };
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

test.each([
  [{ kind: 'never_indexed' }, true],
  [{ kind: 'partial', reason: 'metadata incomplete' }, true],
  [{ kind: 'metadata_complete_selected', observedAtMs: 1 }, true],
  [{ kind: 'complete_selected', observedAtMs: 1 }, true],
] satisfies [ProviderCoverage, boolean][])('offers an explicit exact edit for an authorized grant independently of whole-source discovery (%j)', async (coverage, expected) => {
  const source: ProviderSource = { sourceId: 'public-provider', readerSourceId: reference.sourceId, name: 'Public notes', kind: 'google_drive', enabled: true,
    selectedScope: { kind: 'drive_folder', folderId: 'public-root', recursive: true },
    capabilities: { read: true, list: true, changes: false, assets: false, guardedWrite: true }, health: 'ready', coverage };
  const state = vi.spyOn(nativeProvidersBackend, 'state').mockResolvedValue({ sources: [source], availability: [], pendingAdmissions: [], error: null });
  const view = renderHook(() => useProviderEditing(reference, 'public-tab', vi.fn()));
  await act(async () => { await Promise.resolve(); });
  expect(state).toHaveBeenCalledTimes(1);
  await waitFor(() => expect(view.result.current.canEdit).toBe(expected));
  const begin = vi.spyOn(providerEditBackend, 'begin').mockRejectedValue(new Error('Native exact revision check refused'));
  expect(begin).not.toHaveBeenCalled();
  await act(async () => view.result.current.edit(reference));
  expect(begin).toHaveBeenCalledExactlyOnceWith(reference);
});

test('a new exact receipt rechecks editing availability and ignores a late reply for the prior document', async () => {
  const writable: ProviderSource = { sourceId: 'public-provider', readerSourceId: reference.sourceId, name: 'Public notes', kind: 'google_drive', enabled: true,
    selectedScope: { kind: 'drive_folder', folderId: 'public-root', recursive: true },
    capabilities: { read: true, list: true, changes: false, assets: false, guardedWrite: true }, health: 'ready',
    coverage: { kind: 'metadata_complete_selected', observedAtMs: 1 } };
  const snapshot = (allowed: boolean) => ({ sources: [{ ...writable, capabilities: { ...writable.capabilities, guardedWrite: allowed } }], availability: [], pendingAdmissions: [], error: null });
  let previous!: (value: ReturnType<typeof snapshot>) => void;
  let next!: (value: ReturnType<typeof snapshot>) => void;
  const state = vi.spyOn(nativeProvidersBackend, 'state')
    .mockImplementationOnce(() => new Promise(resolve => { previous = resolve; }))
    .mockImplementationOnce(() => new Promise(resolve => { next = resolve; }));
  const view = renderHook(value => useProviderEditing(value, 'public-tab', vi.fn()), { initialProps: reference });
  const newReference = { ...reference, path: 'records/second.md', revision: 'c'.repeat(64) };
  view.rerender(newReference);
  expect(view.result.current.canEdit).toBe(false);
  await act(async () => next(snapshot(true)));
  expect(view.result.current.canEdit).toBe(true);
  await act(async () => previous(snapshot(false)));
  expect(view.result.current.canEdit).toBe(true);
  expect(state).toHaveBeenCalledTimes(2);
});

test('a changed grant observation is fenced immediately until its new capability answer arrives', async () => {
  const source: ProviderSource = { sourceId: 'public-provider', readerSourceId: reference.sourceId, name: 'Public notes', kind: 'google_drive', enabled: true,
    selectedScope: { kind: 'drive_folder', folderId: 'root', recursive: true },
    capabilities: { read: true, list: true, changes: false, assets: false, guardedWrite: true }, health: 'ready',
    coverage: { kind: 'complete_selected', observedAtMs: 1 } };
  let finish!: (value: Awaited<ReturnType<typeof nativeProvidersBackend.state>>) => void;
  vi.spyOn(nativeProvidersBackend, 'state').mockResolvedValueOnce({ sources: [source], availability: [], pendingAdmissions: [], error: null })
    .mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  const view = renderHook(key => useProviderEditing(reference, 'public-tab', vi.fn(), key), { initialProps: 0 });
  await waitFor(() => expect(view.result.current.canEdit).toBe(true));
  view.rerender(1);
  expect(view.result.current.canEdit).toBe(false);
  await act(async () => finish({ sources: [{ ...source, enabled: false, health: 'paused' }], availability: [], pendingAdmissions: [], error: null }));
  expect(view.result.current.canEdit).toBe(false);
});

test.each(['readonly', 'paused', 'reconnect'])('never offers editing for a %s saved grant', async kind => {
  const source: ProviderSource = { sourceId: 'public-provider', readerSourceId: reference.sourceId, name: 'Public notes', kind: 'google_drive',
    enabled: kind !== 'paused', health: kind === 'reconnect' ? 'reconnect_required' : 'ready', selectedScope: { kind: 'drive_folder', folderId: 'root', recursive: true },
    capabilities: { read: true, list: true, changes: false, assets: false, guardedWrite: kind !== 'readonly' }, coverage: { kind: 'complete_selected', observedAtMs: 1 } };
  const state = vi.spyOn(nativeProvidersBackend, 'state').mockResolvedValue({ sources: [source], availability: [], pendingAdmissions: [], error: null });
  const view = renderHook(() => useProviderEditing(reference, 'public-tab', vi.fn()));
  await waitFor(() => expect(state).toHaveBeenCalledOnce());
  expect(view.result.current.canEdit).toBe(false);
});
