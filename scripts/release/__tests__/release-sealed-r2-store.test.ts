import { mkdtemp, open, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { getHeapStatistics } from 'node:v8';

import { describe, expect, test, vi } from 'vitest';

import type { PublicationEntry } from '../release-pipeline';
import * as sealedR2Store from '../sealed-r2-store';
import {
  R2FetchStore,
  type R2Fetch,
} from '../sealed-r2-store';

const CREDENTIALS = {
  accountId: '0123456789abcdef0123456789abcdef',
  accessKeyId: 'test-access-key',
  secretAccessKey: 'test-secret-key',
};
const BUCKET = 'mdreader-downloads';

function entry(key: string, immutable: boolean): PublicationEntry {
  return {
    kind: immutable ? 'artifact' : 'pointer',
    key,
    immutable,
    localPath: null,
    contents: null,
  };
}

function successfulResponse(
  body: BodyInit | null,
  headers: Record<string, string>,
): Response {
  return new Response(body, {
    status: 200,
    headers: {
      etag: '"etag-1"',
      ...headers,
    },
  });
}

async function settleWithin(
  promise: Promise<unknown>,
  milliseconds = 25,
): Promise<{ kind: 'fulfilled' | 'rejected' | 'pending'; message?: string }> {
  return Promise.race([
    promise.then(
      () => ({ kind: 'fulfilled' as const }),
      (error: unknown) => ({
        kind: 'rejected' as const,
        message: error instanceof Error ? error.message : String(error),
      }),
    ),
    new Promise<{ kind: 'pending' }>((resolve) => {
      setTimeout(() => resolve({ kind: 'pending' }), milliseconds);
    }),
  ]);
}

describe('sealed publisher R2 transport', () => {
  test('refuses redirects instead of forwarding a signed GET to another origin', async () => {
    const fetchImplementation: R2Fetch = async (_input, init) =>
      init?.redirect === 'error'
        ? new Response(null, {
            status: 302,
            headers: { location: 'https://evil.example/capture' },
          })
        : successfulResponse('evil', { 'content-length': '4' });
    const store = new R2FetchStore(
      CREDENTIALS,
      BUCKET,
      fetchImplementation,
    );

    await expect(
      store.read(
        entry('releases/v1.0.0/MDReader.dmg', true),
        4,
      ),
    ).rejects.toThrow(/redirect/);
  });

  test('refuses redirects instead of forwarding a signed PUT body', async () => {
    const fetchImplementation: R2Fetch = async (_input, init) =>
      init?.redirect === 'error'
        ? new Response(null, {
            status: 307,
            headers: { location: 'https://evil.example/capture' },
          })
        : new Response(null, { status: 200 });
    const store = new R2FetchStore(
      CREDENTIALS,
      BUCKET,
      fetchImplementation,
    );

    await expect(
      store.put(
        entry('releases/v1.0.0/MDReader.dmg', true),
        Buffer.from('body'),
      ),
    ).rejects.toThrow(/redirect/);
  });

  test('rejects missing and contradictory object lengths', async () => {
    const missing = new R2FetchStore(
      CREDENTIALS,
      BUCKET,
      async () => successfulResponse('body', {}),
    );
    await expect(
      missing.read(entry('releases/v1.0.0/a', true), 4),
    ).rejects.toThrow(/Content-Length/);

    const contradictory = new R2FetchStore(
      CREDENTIALS,
      BUCKET,
      async () => successfulResponse('no', { 'content-length': '3' }),
    );
    await expect(
      contradictory.read(entry('releases/v1.0.0/a', true), 2),
    ).rejects.toThrow(/Content-Length|length/);
  });

  test.each([
    [
      'missing ETag',
      entry('releases/v1.0.0/a', true),
      1,
      { 'content-length': '1' },
      false,
      /ETag/,
    ],
    [
      'missing Content-Length',
      entry('releases/v1.0.0/a', true),
      1,
      {},
      true,
      /Content-Length/,
    ],
    [
      'invalid Content-Length',
      entry('releases/v1.0.0/a', true),
      1,
      { 'content-length': 'invalid' },
      true,
      /Content-Length/,
    ],
    [
      'oversized stable pointer',
      entry('channels/stable/manifest.json', false),
      1,
      { 'content-length': String(64 * 1024 + 1) },
      true,
      /byte limit|exceeds/,
    ],
    [
      'invalid desired length',
      entry('releases/v1.0.0/a', true),
      -1,
      { 'content-length': '1' },
      true,
      /desired byte length/,
    ],
  ] as const)(
    'cancels the response body on the %s fail-closed path',
    async (_label, publicationEntry, desiredBytes, headers, includeEtag, error) => {
      let cancelled = false;
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          controller.enqueue(Buffer.from('x'));
        },
        cancel() {
          cancelled = true;
        },
      });
      const store = new R2FetchStore(
        CREDENTIALS,
        BUCKET,
        async () =>
          new Response(body, {
            status: 200,
            headers: {
              ...headers,
              ...(includeEtag ? { etag: '"etag-1"' } : {}),
            },
          }),
      );

      await expect(
        store.read(publicationEntry, desiredBytes),
      ).rejects.toThrow(error);
      await Promise.resolve();
      expect(cancelled).toBe(true);
    },
  );

  test('cancels a chunked immutable response at desired-size-plus-one without waiting for EOF', async () => {
    let cancelled = false;
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(Buffer.from('12345'));
      },
      cancel() {
        cancelled = true;
      },
    });
    const store = new R2FetchStore(
      CREDENTIALS,
      BUCKET,
      async () =>
        successfulResponse(body, {
          // Desired bytes are 3, so the reviewed read ceiling is 4.
          'content-length': '4',
        }),
    );

    const outcome = await settleWithin(
      store.read(entry('releases/v1.0.0/a', true), 3),
    );
    expect(outcome).toMatchObject({
      kind: 'rejected',
      message: expect.stringMatching(/byte limit|bounded|too large/),
    });
    expect(cancelled).toBe(true);
  });

  test('caps stable pointer responses at 64 KiB', async () => {
    const body = Buffer.alloc(64 * 1024 + 1, 0x61);
    const store = new R2FetchStore(
      CREDENTIALS,
      BUCKET,
      async () =>
        successfulResponse(body, {
          'content-length': String(body.byteLength),
        }),
    );

    await expect(
      store.read(entry('channels/stable/manifest.json', false), 128),
    ).rejects.toThrow(/stable pointer|byte limit|too large/);
  });

  test('applies one deadline through response headers', async () => {
    vi.useFakeTimers();
    try {
      const fetchImplementation: R2Fetch = async (_input, init) =>
        new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener(
            'abort',
            () => reject(init.signal?.reason),
            { once: true },
          );
        });
      const store = new R2FetchStore(
        CREDENTIALS,
        BUCKET,
        fetchImplementation,
      );
      let outcome:
        | { kind: 'fulfilled' }
        | { kind: 'rejected'; message: string }
        | undefined;
      void store
        .read(entry('channels/stable/manifest.json', false), 128)
        .then(
          () => {
            outcome = { kind: 'fulfilled' };
          },
          (error: unknown) => {
            outcome = {
              kind: 'rejected',
              message: error instanceof Error ? error.message : String(error),
            };
          },
        );

      await vi.advanceTimersByTimeAsync(30_001);
      await Promise.resolve();
      expect(outcome).toMatchObject({
        kind: 'rejected',
        message: expect.stringMatching(/deadline|timed out/),
      });
    } finally {
      vi.useRealTimers();
    }
  });

  test('applies the publication deadline while a signed PUT is stalled', async () => {
    vi.useFakeTimers();
    try {
      const fetchImplementation: R2Fetch = async (_input, init) =>
        new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener(
            'abort',
            () => reject(init.signal?.reason),
            { once: true },
          );
        });
      const store = new R2FetchStore(
        CREDENTIALS,
        BUCKET,
        fetchImplementation,
      );
      let outcome:
        | { kind: 'fulfilled' }
        | { kind: 'rejected'; message: string }
        | undefined;
      void store
        .put(
          entry('releases/v1.0.0/MDReader.dmg', true),
          Buffer.from('body'),
        )
        .then(
          () => {
            outcome = { kind: 'fulfilled' };
          },
          (error: unknown) => {
            outcome = {
              kind: 'rejected',
              message: error instanceof Error ? error.message : String(error),
            };
          },
        );

      await vi.advanceTimersByTimeAsync(30_001);
      await Promise.resolve();
      expect(outcome).toMatchObject({
        kind: 'rejected',
        message: expect.stringMatching(/deadline|timed out/),
      });
    } finally {
      vi.useRealTimers();
    }
  });

  test('applies the same deadline through the streamed body and cancels it', async () => {
    vi.useFakeTimers();
    try {
      let cancelled = false;
      const body = new ReadableStream<Uint8Array>({
        cancel() {
          cancelled = true;
        },
      });
      const store = new R2FetchStore(
        CREDENTIALS,
        BUCKET,
        async () =>
          successfulResponse(body, {
            'content-length': '1',
          }),
      );
      let outcome:
        | { kind: 'fulfilled' }
        | { kind: 'rejected'; message: string }
        | undefined;
      void store
        .read(entry('releases/v1.0.0/a', true), 1)
        .then(
          () => {
            outcome = { kind: 'fulfilled' };
          },
          (error: unknown) => {
            outcome = {
              kind: 'rejected',
              message: error instanceof Error ? error.message : String(error),
            };
          },
        );

      await vi.advanceTimersByTimeAsync(30_001);
      await Promise.resolve();
      expect(outcome).toMatchObject({
        kind: 'rejected',
        message: expect.stringMatching(/deadline|timed out/),
      });
      expect(cancelled).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  test('refuses an oversized sealed plan before buffering it into the publisher', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-sealed-plan-limit-'));
    const path = join(root, 'oversized-plan.json');
    try {
      const file = await open(path, 'w');
      try {
        await file.truncate(352 * 1024 * 1024 + 1);
      } finally {
        await file.close();
      }
      const readBoundedFile = (
        sealedR2Store as typeof sealedR2Store & {
          readBoundedFile?: (candidate: string) => Promise<Uint8Array>;
        }
      ).readBoundedFile;
      expect(readBoundedFile).toBeTypeOf('function');
      if (!readBoundedFile) return;
      await expect(readBoundedFile(path)).rejects.toThrow(
        /sealed publication plan.*byte limit|too large/i,
      );
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  test('reads an in-envelope sealed plan without duplicating or truncating bytes', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-sealed-plan-read-'));
    const path = join(root, 'plan.json');
    const expected = Buffer.from('{"schemaVersion":1,"entries":[]}\n');
    try {
      await writeFile(path, expected);
      await expect(sealedR2Store.readBoundedFile(path)).resolves.toEqual(
        expected,
      );
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });

  test('refuses to parse the body-complete plan under a constrained Node heap', () => {
    const assertMemoryEnvelope = (
      sealedR2Store as typeof sealedR2Store & {
        assertPublisherMemoryEnvelope?: (heapLimit?: number) => void;
      }
    ).assertPublisherMemoryEnvelope;
    expect(assertMemoryEnvelope).toBeTypeOf('function');
    if (!assertMemoryEnvelope) return;
    expect(() => assertMemoryEnvelope(512 * 1024 * 1024)).toThrow(
      /Node heap|memory envelope/,
    );
    expect(() =>
      assertMemoryEnvelope(getHeapStatistics().heap_size_limit),
    ).not.toThrow();
  });
});
