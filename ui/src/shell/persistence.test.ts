import { afterEach, describe, expect, test, vi } from 'vitest';

const native = vi.hoisted(() => ({ invoke: vi.fn(), pluginWrite: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: native.invoke }));
vi.mock('@tauri-apps/plugin-store', () => ({
  load: async () => ({ get: async () => { throw new Error('legacy read failed'); }, set: native.pluginWrite, save: async () => {} }),
}));

import { createSerialWriter } from './persistence.js';

afterEach(() => {
  delete (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  native.invoke.mockReset();
  native.pluginWrite.mockReset();
});

/** A resolvable promise handle for driving write completion by hand. */
function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

/** Flush the microtask queue (the finally→pump hop is a microtask). */
function flush() {
  return new Promise((res) => setTimeout(res, 0));
}

describe('createSerialWriter — single-flight, newest-wins', () => {
  test('runs one write at a time and coalesces a burst to the newest value', async () => {
    const started: number[] = [];
    const gates: Array<() => void> = [];
    const writer = createSerialWriter<number>((n) => {
      started.push(n);
      const d = deferred();
      gates.push(d.resolve);
      return d.promise;
    });

    writer.write(1); // starts immediately
    writer.write(2); // queued behind the in-flight write
    writer.write(3); // coalesces over 2 — only the newest is kept
    expect(started).toEqual([1]);

    gates[0]!(); // finish write(1)
    await flush();
    // 2 was dropped; the coalesced newest (3) runs next.
    expect(started).toEqual([1, 3]);

    gates[1]!(); // finish write(3)
    await flush();
    expect(started).toEqual([1, 3]);
  });

  test('drain awaits the in-flight write and its coalesced follow-up', async () => {
    const finished: number[] = [];
    const writer = createSerialWriter<number>(async (n) => {
      await Promise.resolve();
      finished.push(n);
    });

    writer.write(1); // in flight
    writer.write(2); // queued
    await writer.drain();
    expect(finished).toEqual([1, 2]);
  });

  test('drain resolves immediately when nothing is queued', async () => {
    const writer = createSerialWriter<number>(async () => {});
    await expect(writer.drain()).resolves.toBeUndefined();
  });

  test('a rejected write does not wedge the queue', async () => {
    const finished: number[] = [];
    const writer = createSerialWriter<number>(async (n) => {
      if (n === 1) throw new Error('boom');
      finished.push(n);
    });

    writer.write(1); // rejects internally
    writer.write(2); // must still run after the failure
    await writer.drain();
    expect(finished).toEqual([2]);
  });

  test('latest failure rejects strict drain until a newer value succeeds', async () => {
    const writer = createSerialWriter<number>(async (n) => { if (n < 3) throw new Error('storage failed'); });
    writer.write(1);
    await expect(writer.drain()).rejects.toThrow('storage failed');
    writer.write(2);
    await expect(writer.drain()).rejects.toThrow('storage failed');
    writer.write(3);
    await expect(writer.drain()).resolves.toBeUndefined();
  });
});

test('native hydration failure keeps default rendering read-only until explicit matching reload', async () => {
  vi.resetModules();
  (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {};
  const identity = { ownerEpoch: 'owner', rendererEpoch: 'renderer' };
  let registrationFailure = true;
  let readFailure = true;
  let mismatchedIdentity = false;
  let writes = 0;
  native.invoke.mockImplementation(async (command: string, args?: { kind: string; generation: number }) => {
    if (command === 'native_profile_persistence_host') return 'native_atomic';
    if (command === 'native_profile_persistence_load') {
      if (registrationFailure) throw new Error('synthetic interrupted registration reply');
      return { identity, sessionJson: null, settingsJson: null };
    }
    if (command === 'native_profile_persistence_read') {
      if (readFailure) throw new Error('private native read diagnostic');
      return { identity: mismatchedIdentity ? { ...identity, rendererEpoch: 'other' } : identity,
        sessionJson: null, settingsJson: '{"settings":{"theme":"dark"}}' };
    }
    if (command === 'native_profile_persistence_write' && args) {
      writes += 1;
      return { ...identity, kind: args.kind, generation: args.generation, sha256: 'a'.repeat(64) };
    }
    throw new Error('unexpected synthetic command');
  });
  const persistence = await import('./persistence.js');
  const { DEFAULT_SETTINGS } = await import('../stores/session.js');
  const session = { version: 1, root: null, activeTabId: null, tabs: [], recent: [] };
  expect(await persistence.loadPersisted()).toEqual({ session: null, settings: {}, restoreFailed: true });
  await expect(persistence.saveSession(session)).rejects.toThrow('not hydrated');
  await expect(persistence.saveSettings(DEFAULT_SETTINGS)).rejects.toThrow('not hydrated');
  expect(writes).toBe(0);
  registrationFailure = false;
  expect(await persistence.loadPersisted()).toEqual({ session: null, settings: {}, restoreFailed: true });
  await expect(persistence.saveSession(session)).rejects.toThrow('not hydrated');
  expect(writes).toBe(0);
  readFailure = false;
  mismatchedIdentity = true;
  expect(await persistence.loadPersisted()).toEqual({ session: null, settings: {}, restoreFailed: true });
  await expect(persistence.saveSession(session)).rejects.toThrow('not hydrated');
  expect(writes).toBe(0);
  mismatchedIdentity = false;
  expect(await persistence.loadPersisted()).toEqual({
    session: null, settings: { theme: 'dark' },
    readerHistory: { mode: 'native_atomic', diagnostic: null,
      data: { version: 1, activeTabId: null, tabs: [], recent: [], mru: [] } },
  });
  await persistence.saveSession(session);
  await persistence.saveSettings(DEFAULT_SETTINGS);
  expect(writes).toBe(2);
});

test('explicit legacy-host ordinary persistence retains its existing write behavior', async () => {
  vi.resetModules();
  (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {};
  native.invoke.mockImplementation(async (command: string) => {
    if (command === 'native_profile_persistence_host') return 'legacy_host';
    throw new Error('native atomic writer must not be used');
  });
  const persistence = await import('./persistence.js');
  expect(await persistence.loadPersisted()).toEqual({ session: null, settings: {}, restoreFailed: true });
  await persistence.saveSession({ version: 1, root: null, activeTabId: null, tabs: [], recent: [] });
  expect(native.pluginWrite).toHaveBeenCalledTimes(1);
  expect(native.invoke).toHaveBeenCalledTimes(1);
});

test('cancelled preservation never admits after its first await', async () => {
  vi.resetModules();
  (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {};
  const identity = { ownerEpoch: 'owner', rendererEpoch: 'renderer' };
  native.invoke.mockImplementation(async (command: string, args?: { kind: string; generation: number }) => {
    if (command === 'native_profile_persistence_host') return 'native_atomic';
    if (command === 'native_profile_persistence_load' || command === 'native_profile_persistence_read') {
      return { identity, sessionJson: null, settingsJson: null };
    }
    if (command === 'native_profile_persistence_write' && args) {
      return { ...identity, kind: args.kind, generation: args.generation, sha256: 'a'.repeat(64) };
    }
    throw new Error('unexpected synthetic command');
  });
  const persistence = await import('./persistence.js');
  await persistence.loadPersisted();
  const signal = new AbortController();
  const latest = vi.fn();
  const pending = persistence.flushForPreservation({ requestId: 'cancelled', identity }, latest, signal.signal);
  signal.abort();
  await expect(pending).rejects.toThrow('retired');
  expect(latest).not.toHaveBeenCalled();
  await persistence.saveSession({ version: 1, root: '/retained' });
  expect(native.invoke.mock.calls.some(([command]) => command === 'native_profile_persistence_flush')).toBe(false);
});

test('release or cancellation while draining cannot submit a retired flush or reopen held admission', async () => {
  vi.resetModules();
  (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {};
  const identity = { ownerEpoch: 'owner', rendererEpoch: 'renderer' };
  let gate = deferred();
  native.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    if (command === 'native_profile_persistence_host') return 'native_atomic';
    if (command === 'native_profile_persistence_load' || command === 'native_profile_persistence_read') {
      return { identity, sessionJson: null, settingsJson: null };
    }
    if (command === 'native_profile_persistence_write' && args) {
      await gate.promise;
      return { ...identity, kind: args.kind, generation: args.generation, sha256: 'a'.repeat(64) };
    }
    if (command === 'native_profile_persistence_flush' && args) {
      const submission = args.submission as { sessionGeneration: number; settingsGeneration: number };
      return [
        { ...identity, kind: 'session', generation: submission.sessionGeneration, sha256: 'b'.repeat(64) },
        { ...identity, kind: 'settings', generation: submission.settingsGeneration, sha256: 'c'.repeat(64) },
      ];
    }
    throw new Error('unexpected synthetic command');
  });
  const persistence = await import('./persistence.js');
  const { DEFAULT_SETTINGS } = await import('../stores/session.js');
  await persistence.loadPersisted();
  persistence.sessionWriter.write({ version: 1, root: '/pending' });
  await flush();
  const first = { requestId: 'first', identity };
  const current = { requestId: 'current', identity };
  const oldLatest = vi.fn();
  const old = persistence.flushForPreservation(first, oldLatest);
  const retired = expect(old).rejects.toThrow('retired');
  await flush();
  expect(persistence.releasePreservation({ ...first, identity: { ...identity, rendererEpoch: 'wrong' } })).toBe(false);
  expect(persistence.releasePreservation(first)).toBe(true);
  const currentLatest = vi.fn(() => ({ session: persistence.sessionWriter.snapshot(null, false), settings: DEFAULT_SETTINGS }));
  const next = persistence.flushForPreservation(current, currentLatest);
  await flush();
  gate.resolve();
  await retired; await next;
  expect(oldLatest).not.toHaveBeenCalled();
  expect(currentLatest).toHaveBeenCalledTimes(1);
  const submissions = native.invoke.mock.calls.filter(([command]) => command === 'native_profile_persistence_flush');
  expect(submissions).toHaveLength(1);
  expect(submissions[0]![1]).toMatchObject({ submission: { request: current } });
  await expect(persistence.saveSession({ version: 1 })).rejects.toThrow('frozen');
  expect(persistence.releasePreservation(first)).toBe(false);
  expect(persistence.releasePreservation(current)).toBe(true);
  await persistence.saveSession({ version: 1 });

  gate = deferred();
  persistence.sessionWriter.write({ version: 1, root: '/still-pending' });
  await flush();
  const disposed = { requestId: 'disposed', identity };
  const controller = new AbortController();
  const disposedLatest = vi.fn();
  const pending = persistence.flushForPreservation(disposed, disposedLatest, controller.signal);
  const cancelled = expect(pending).rejects.toThrow('retired');
  await flush();
  controller.abort(); gate.resolve(); await cancelled;
  expect(disposedLatest).not.toHaveBeenCalled();
  await expect(persistence.saveSession({ version: 1 })).rejects.toThrow('frozen');
  expect(persistence.releasePreservation(disposed)).toBe(true);
  await persistence.saveSession({ version: 1 });
});
