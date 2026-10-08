import { beforeEach, expect, test, vi } from 'vitest';
import type { FlushRequest } from './persistence';

const boundary = vi.hoisted(() => ({
  listeners: new Map<string, (event: { payload: FlushRequest }) => void>(),
  off: vi.fn(), flush: vi.fn(), release: vi.fn(), listen: vi.fn(),
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: boundary.listen }));
vi.mock('./persistence.js', () => ({
  flushForPreservation: boundary.flush, releasePreservation: boundary.release,
}));
import { installProfileTransitionBridge } from './profile-transition-bridge';

const request = (requestId: string): FlushRequest => ({
  requestId, identity: { ownerEpoch: 'owner', rendererEpoch: 'renderer' },
});
const emit = (kind: 'flush' | 'released', payload: FlushRequest) => {
  boundary.listeners.get(`mdreader://profile-preservation-${kind}`)?.({ payload });
};
const settle = async () => { await Promise.resolve(); await Promise.resolve(); };
function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((yes) => { resolve = yes; });
  return { promise, resolve };
}
function bridge() {
  return { verifyEditorFrozen: vi.fn(async () => {}), latest: vi.fn(), failed: vi.fn(), resumed: vi.fn() };
}
beforeEach(() => {
  boundary.listeners.clear();
  boundary.off.mockReset(); boundary.flush.mockReset(); boundary.release.mockReset();
  boundary.release.mockReturnValue(false);
  boundary.listen.mockReset().mockImplementation(async (name: string, callback: (event: { payload: FlushRequest }) => void) => {
    boundary.listeners.set(name, callback);
    return () => { boundary.listeners.delete(name); boundary.off(name); };
  });
});

test('failed preflight retires on full matching release and admits a fresh request, without replaying duplicates', async () => {
  const owner = bridge();
  owner.verifyEditorFrozen.mockRejectedValueOnce(new Error('synthetic preflight failure'));
  const dispose = await installProfileTransitionBridge(owner);
  const first = request('first');
  emit('flush', first);
  // Event data cannot mutate the held request identity.
  first.identity.ownerEpoch = 'mutated';
  await settle();
  expect(owner.failed).toHaveBeenCalledTimes(1);
  for (const identity of [
    { ownerEpoch: 'other', rendererEpoch: 'renderer' },
    { ownerEpoch: 'owner', rendererEpoch: 'other' },
  ]) emit('released', { ...request('first'), identity });
  emit('flush', request('blocked'));
  expect(owner.verifyEditorFrozen).toHaveBeenCalledTimes(1);
  emit('released', request('first'));
  emit('released', request('first'));
  expect(boundary.release).toHaveBeenCalledExactlyOnceWith(request('first'));
  expect(owner.resumed).toHaveBeenCalledTimes(1);
  emit('flush', request('first'));
  emit('flush', request('fresh'));
  emit('flush', request('fresh'));
  await settle();
  expect(owner.verifyEditorFrozen).toHaveBeenCalledTimes(2);
  expect(boundary.flush).toHaveBeenCalledExactlyOnceWith(request('fresh'), owner.latest, expect.any(AbortSignal));
  emit('released', request('fresh'));
  dispose();
  expect(boundary.listeners.size).toBe(0);
});

test('released or disposed preflight cannot later flush or affect a replacement attempt', async () => {
  const owner = bridge();
  const old = deferred(); const current = deferred();
  owner.verifyEditorFrozen.mockReturnValueOnce(old.promise).mockReturnValueOnce(current.promise);
  const dispose = await installProfileTransitionBridge(owner);
  emit('flush', request('old'));
  emit('released', request('old'));
  emit('flush', request('current'));
  old.resolve(); await settle();
  expect(boundary.flush).not.toHaveBeenCalled();
  dispose(); dispose();
  current.resolve(); await settle();
  expect(boundary.flush).not.toHaveBeenCalled();
  expect(owner.failed).not.toHaveBeenCalled();
  expect(owner.resumed).toHaveBeenCalledTimes(1);
  // A preflight now owns a real editor hold even before JSON flush starts.
  expect(boundary.listeners.size).toBe(1);
  emit('released', request('current'));
  expect(boundary.listeners.size).toBe(0);
  expect(boundary.off).toHaveBeenCalledTimes(2);
});

test('disposal aborts pending flush but retains only exact release cleanup, without resuming the UI', async () => {
  const owner = bridge(); const gate = deferred();
  boundary.flush.mockImplementation(async () => { await gate.promise; throw new Error('retired'); });
  const dispose = await installProfileTransitionBridge(owner);
  emit('flush', request('held')); await settle();
  const signal = boundary.flush.mock.calls[0]![2] as AbortSignal;
  dispose(); dispose();
  expect(signal.aborted).toBe(true);
  expect(boundary.listeners.size).toBe(1);
  expect(boundary.release).not.toHaveBeenCalled();
  gate.resolve(); await settle();
  emit('released', { ...request('held'), identity: { ownerEpoch: 'owner', rendererEpoch: 'wrong' } });
  expect(boundary.listeners.size).toBe(1);
  emit('released', request('held'));
  expect(boundary.release).toHaveBeenCalledExactlyOnceWith(request('held'));
  expect(boundary.listeners.size).toBe(0);
  expect(owner.failed).not.toHaveBeenCalled();
  expect(owner.resumed).not.toHaveBeenCalled();
});

test('failed second listener installation removes the first listener', async () => {
  boundary.listen.mockImplementationOnce(async (name: string) => () => boundary.off(name))
    .mockRejectedValueOnce(new Error('synthetic registration failure'));
  await expect(installProfileTransitionBridge(bridge())).rejects.toThrow('synthetic registration failure');
  expect(boundary.off).toHaveBeenCalledExactlyOnceWith('mdreader://profile-preservation-released');
});
