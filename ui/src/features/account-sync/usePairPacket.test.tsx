import { act, renderHook } from '@testing-library/react';
import { expect, test } from 'vitest';
import type { AccountStatus } from './backend';
import { usePairPacket } from './usePairPacket';

const LIBRARY = '00000000-0000-4000-8000-000000000001';
const packet = JSON.stringify({ v: 1, kind: 'mdreader-pair-request', account: 'a'.repeat(64), libraryId: LIBRARY,
  pairId: '00000000-0000-4000-8000-000000000002',
  signingJwk: { kty: 'EC', crv: 'P-256', x: 'A'.repeat(43), y: 'B'.repeat(43) },
  wrappingJwk: { kty: 'EC', crv: 'P-256', x: 'C'.repeat(43), y: 'D'.repeat(43) },
  signingThumb: 'E'.repeat(43), wrappingThumb: 'F'.repeat(43), nonce: 'G'.repeat(43), expires: 2000000000 });
const state: AccountStatus = { configured: true, connected: false, retainedLibrary: true, custodyWritePending: false, activeLibraryId: LIBRARY,
  existingLibraryId: null, existingLibrarySameSelection: false, recoveryAvailable: true, selectedScope: null,
  custody: { state: 'pair_candidate', enrollmentUncertain: true, enrollmentExpired: false, materialStaged: false,
    recoveryExportAvailable: false, deviceApproval: 'none' },
  latestJob: { id: LIBRARY, selectedSync: null, selectedPurpose: null, state: {
    state: 'completed', outcome: { kind: 'public_packet', packetKind: 'request', packet },
  } },
};
test('a returned public request survives harmless latest-job changes and is retired by expiry or owner change', () => {
  const view = renderHook(({ status }) => usePairPacket(status, 'request'), { initialProps: { status: state } });
  expect(view.result.current?.text).toBe(packet);
  act(() => view.rerender({ status: { ...state, latestJob: null } }));
  expect(view.result.current?.text).toBe(packet);
  act(() => view.rerender({ status: { ...state, latestJob: null,
    custody: { ...state.custody!, enrollmentExpired: true } } }));
  expect(view.result.current).toBeNull();
  act(() => view.rerender({ status: { ...state, latestJob: null } }));
  expect(view.result.current).toBeNull();
  act(() => view.rerender({ status: { ...state,
    activeLibraryId: '00000000-0000-4000-8000-000000000003', latestJob: null } }));
  expect(view.result.current).toBeNull();
  view.unmount();
});
