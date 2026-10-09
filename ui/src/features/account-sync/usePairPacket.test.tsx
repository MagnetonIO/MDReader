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
  custody: { state: 'pair_candidate', pairRenewal: 'none', pairRequestId: null, enrollmentUncertain: true, enrollmentExpired: false, materialStaged: false,
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

test('a replacement request identity retires retained text even if a stale latest job still carries the old packet', () => {
  const oldPair = JSON.parse(packet).pairId as string;
  const nextPair = '00000000-0000-4000-8000-000000000004';
  const selected = { ...state, custody: { ...state.custody!, pairRequestId: oldPair } };
  const view = renderHook(({ status }) => usePairPacket(status, 'request'), { initialProps: { status: selected } });
  expect(view.result.current?.packet.pairId).toBe(oldPair);
  const replaced = { ...selected, custody: { ...selected.custody, pairRequestId: nextPair } };
  act(() => view.rerender({ status: replaced }));
  expect(view.result.current).toBeNull();
  const replacement = JSON.stringify({ ...JSON.parse(packet), pairId: nextPair });
  act(() => view.rerender({ status: { ...replaced, latestJob: { ...state.latestJob!, state: {
    state: 'completed', outcome: { kind: 'public_packet', packetKind: 'request', packet: replacement },
  } } } }));
  expect(view.result.current?.text).toBe(replacement);
  act(() => view.rerender({ status: { ...replaced, latestJob: null } }));
  expect(view.result.current?.text).toBe(replacement);
  view.unmount();
});

test.each(['prepared', 'uncertain', 'approval_required'] as const)('a %s renewal cannot offer an old request as ready to share', pairRenewal => {
  const view = renderHook(({ status }) => usePairPacket(status, 'request'), { initialProps: { status: state } });
  expect(view.result.current?.text).toBe(packet);
  act(() => view.rerender({ status: { ...state, custody: { ...state.custody!, pairRenewal } } }));
  expect(view.result.current).toBeNull();
  view.unmount();
});
