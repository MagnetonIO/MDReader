import { describe, expect, test } from 'vitest';

import { createSerialWriter } from './persistence.js';

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
});
