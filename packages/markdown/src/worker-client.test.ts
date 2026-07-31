import { afterEach, describe, expect, test } from 'vitest';

import {
  createMarkdownWorker,
  parseMarkdownAuto,
  parseMarkdownInWorker,
  resetMarkdownWorker,
} from './worker-client.js';
import { parseMarkdown } from './pipeline.js';
import { WORKER_BYTE_THRESHOLD } from './thresholds.js';
import type { WorkerRequest, WorkerResponse } from './worker.js';

type Listener = (event: unknown) => void;

/** Minimal Worker double that records posts and lets tests emit events. */
class MockWorker {
  static instances: MockWorker[] = [];
  posted: WorkerRequest[] = [];
  terminated = false;
  private listeners: Record<string, Listener[]> = { message: [], error: [], messageerror: [] };

  constructor(
    public url: URL,
    public options?: unknown,
  ) {
    MockWorker.instances.push(this);
  }

  postMessage(message: WorkerRequest): void {
    this.posted.push(message);
  }

  addEventListener(type: string, listener: Listener): void {
    (this.listeners[type] ??= []).push(listener);
  }

  removeEventListener(type: string, listener: Listener): void {
    this.listeners[type] = (this.listeners[type] ?? []).filter((l) => l !== listener);
  }

  terminate(): void {
    this.terminated = true;
  }

  emit(type: string, event: unknown): void {
    for (const listener of this.listeners[type] ?? []) listener(event);
  }

  reply(response: WorkerResponse): void {
    this.emit('message', { data: response });
  }
}

function installMockWorker(): void {
  (globalThis as { Worker?: unknown }).Worker = MockWorker as unknown;
  MockWorker.instances = [];
  resetMarkdownWorker();
}

afterEach(() => {
  delete (globalThis as { Worker?: unknown }).Worker;
  resetMarkdownWorker();
  MockWorker.instances = [];
});

const belowThreshold = '# Small\n\nTiny doc.\n';
const aboveThreshold = `# Big\n\n${'lorem ipsum dolor sit amet '.repeat(4000)}\n`;

describe('worker client — no Worker global', () => {
  test('parseMarkdownInWorker falls back to the sync path', async () => {
    const source = '# Hello\n\nWorld [[node-storage]].\n';
    const viaWorker = await parseMarkdownInWorker(source);
    const direct = parseMarkdown(source);
    expect(viaWorker.stats).toEqual(direct.stats);
    expect(viaWorker.outline).toEqual(direct.outline);
  });

  test('createMarkdownWorker throws where Web Workers are unavailable', () => {
    expect(() => createMarkdownWorker()).toThrow(/Web Workers/);
  });

  test('parseMarkdownAuto parses synchronously even for large input', async () => {
    expect(aboveThreshold.length).toBeGreaterThan(WORKER_BYTE_THRESHOLD);
    const doc = await parseMarkdownAuto(aboveThreshold);
    expect(doc.outline[0]?.text).toBe('Big');
    expect(MockWorker.instances.length).toBe(0);
  });
});

describe('worker routing — Worker available (mocked)', () => {
  test('below the 64 KiB threshold parses sync (no worker spawned)', async () => {
    installMockWorker();
    expect(new TextEncoder().encode(belowThreshold).length).toBeLessThan(WORKER_BYTE_THRESHOLD);

    const doc = await parseMarkdownAuto(belowThreshold);

    expect(doc.outline[0]?.text).toBe('Small');
    expect(MockWorker.instances.length).toBe(0); // routed to sync
  });

  test('at/above the threshold routes to the worker', async () => {
    installMockWorker();
    expect(new TextEncoder().encode(aboveThreshold).length).toBeGreaterThanOrEqual(
      WORKER_BYTE_THRESHOLD,
    );

    const fakeDoc = { marker: 'from-worker' } as unknown;
    const promise = parseMarkdownAuto(aboveThreshold);

    expect(MockWorker.instances.length).toBe(1);
    const worker = MockWorker.instances[0]!;
    expect(worker.posted.length).toBe(1);
    expect(worker.posted[0]!.source).toBe(aboveThreshold);

    worker.reply({ id: worker.posted[0]!.id, doc: fakeDoc as never });
    await expect(promise).resolves.toBe(fakeDoc);
  });

  test('worker error rejects the pending promise (and does not hang)', async () => {
    installMockWorker();
    const promise = parseMarkdownInWorker(aboveThreshold);
    const worker = MockWorker.instances[0]!;

    worker.emit('error', { message: 'worker crashed' });

    await expect(promise).rejects.toThrow(/worker crashed/);
    expect(worker.terminated).toBe(true);
  });

  test('messageerror rejects the pending promise', async () => {
    installMockWorker();
    const promise = parseMarkdownInWorker(aboveThreshold);
    const worker = MockWorker.instances[0]!;

    worker.emit('messageerror', {});

    await expect(promise).rejects.toThrow(/unserializable/);
  });

  test('a worker-reported parse error rejects that request', async () => {
    installMockWorker();
    const promise = parseMarkdownInWorker(aboveThreshold);
    const worker = MockWorker.instances[0]!;

    worker.reply({ id: worker.posted[0]!.id, error: 'boom' });

    await expect(promise).rejects.toThrow(/boom/);
  });

  test('a late error from a replaced worker leaves the live worker untouched', async () => {
    installMockWorker();

    // Request #1 lands on worker #1 (generation 1).
    const p1 = parseMarkdownInWorker(aboveThreshold);
    const w1 = MockWorker.instances[0]!;

    // Worker #1 crashes: it rejects p1 and drops the singleton.
    w1.emit('error', { message: 'worker one crashed' });
    await expect(p1).rejects.toThrow(/worker one crashed/);
    expect(w1.terminated).toBe(true);

    // Request #2 spawns a replacement worker #2 (generation 2).
    const p2 = parseMarkdownInWorker(aboveThreshold);
    expect(MockWorker.instances.length).toBe(2);
    const w2 = MockWorker.instances[1]!;

    // The DEAD worker #1 fires a late error AFTER it was replaced. With
    // generation scoping this is a no-op for the live generation.
    w1.emit('error', { message: 'stale late error from worker one' });

    // Worker #2 is not terminated and its in-flight request still resolves.
    expect(w2.terminated).toBe(false);
    w2.reply({ id: w2.posted[0]!.id, doc: { marker: 'live' } as never });
    await expect(p2).resolves.toEqual({ marker: 'live' });
  });

  test('an injected worker factory is used in place of createMarkdownWorker', async () => {
    // `workerAvailable()` must see a Worker global, but the default
    // `createMarkdownWorker` must NOT be the one that actually runs.
    (globalThis as { Worker?: unknown }).Worker = class {
      constructor() {
        throw new Error('default createMarkdownWorker must not run when a factory is injected');
      }
    };
    resetMarkdownWorker();
    MockWorker.instances = [];

    const built: MockWorker[] = [];
    const factory = (): Worker => {
      const worker = new MockWorker(new URL('file:///injected'), { type: 'module' });
      built.push(worker);
      return worker as unknown as Worker;
    };

    const promise = parseMarkdownInWorker(aboveThreshold, {}, factory);

    expect(built.length).toBe(1);
    const worker = built[0]!;
    expect(worker.posted[0]!.source).toBe(aboveThreshold);

    worker.reply({ id: worker.posted[0]!.id, doc: { marker: 'injected' } as never });
    await expect(promise).resolves.toEqual({ marker: 'injected' });
  });
});
