import { act, cleanup, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { nativeProvidersBackend, type ProviderCoverage, type ProviderSource } from '../../platform/providers';
import { useProviderEditing } from './useProviderEditing';
import { providerEditBackend } from './backend';

const reference = { sourceId: `provider:${'a'.repeat(64)}`, path: 'records/public.md', revision: 'b'.repeat(64) };
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

test.each([
  [{ kind: 'never_indexed' }, false],
  [{ kind: 'partial', reason: 'metadata incomplete' }, false],
  [{ kind: 'metadata_complete_selected', observedAtMs: 1 }, true],
  [{ kind: 'complete_selected', observedAtMs: 1 }, true],
] satisfies [ProviderCoverage, boolean][])('requires completed native metadata before offering guarded provider editing (%j)', async (coverage, expected) => {
  const source: ProviderSource = { sourceId: 'public-provider', readerSourceId: reference.sourceId, name: 'Public notes', kind: 'google_drive', enabled: true,
    selectedScope: { kind: 'drive_folder', folderId: 'public-root', recursive: true },
    capabilities: { read: true, list: true, changes: false, assets: false, guardedWrite: true }, health: 'ready', coverage };
  const state = vi.spyOn(nativeProvidersBackend, 'state').mockResolvedValue({ sources: [source], availability: [], pendingAdmissions: [], error: null });
  const view = renderHook(() => useProviderEditing(reference, 'public-tab', vi.fn()));
  await act(async () => { await Promise.resolve(); });
  expect(state).toHaveBeenCalledTimes(1);
  await waitFor(() => expect(view.result.current.canEdit).toBe(expected));
  if (!expected) {
    const prepare = vi.spyOn(providerEditBackend, 'begin');
    await view.result.current.edit(reference);
    expect(prepare).not.toHaveBeenCalled();
  }
});
