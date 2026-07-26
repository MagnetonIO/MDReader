import { open } from 'node:fs/promises';
import { getHeapStatistics } from 'node:v8';

import type { PublicationEntry } from './release-pipeline';
import {
  type ConditionalReleaseStore,
  type R2ObjectState,
} from './publication-policy';
import {
  createSignedR2Request,
  type R2Credentials,
} from './r2-sigv4';

export type R2Fetch = (
  input: string | URL | Request,
  init?: RequestInit,
) => Promise<Response>;

const REQUEST_DEADLINE_MS = 30_000;
const STABLE_POINTER_MAX_BYTES = 64 * 1024;
const SEALED_PLAN_MAX_BYTES = 352 * 1024 * 1024;
const FILE_READ_CHUNK_BYTES = 64 * 1024;
const MINIMUM_PUBLISHER_HEAP_BYTES = 3 * 1024 * 1024 * 1024;

function requestError(label: string, message: string): Error {
  return new Error(`${label}: ${message}`);
}

export function assertPublisherMemoryEnvelope(
  heapLimit = getHeapStatistics().heap_size_limit,
): void {
  // Worst case keeps the bounded plan buffer, its JSON text/object, up to
  // 256 MiB of decoded desired bodies, one 128 MiB comparison response, and
  // parser/runtime overhead. Refuse constrained Node processes instead of
  // relying on an outer runner timeout or an out-of-memory crash.
  if (
    !Number.isSafeInteger(heapLimit) ||
    heapLimit < MINIMUM_PUBLISHER_HEAP_BYTES
  ) {
    throw new Error(
      `sealed publisher Node heap is below the reviewed memory envelope of ${MINIMUM_PUBLISHER_HEAP_BYTES} bytes`,
    );
  }
}

export async function readBoundedFile(path: string): Promise<Uint8Array> {
  const file = await open(path, 'r');
  try {
    const details = await file.stat();
    if (details.size > SEALED_PLAN_MAX_BYTES) {
      throw new Error(
        `sealed publication plan exceeds the ${SEALED_PLAN_MAX_BYTES}-byte limit`,
      );
    }
    const body = Buffer.allocUnsafe(details.size);
    let total = 0;
    while (total < body.byteLength) {
      const { bytesRead } = await file.read(
        body,
        total,
        Math.min(FILE_READ_CHUNK_BYTES, body.byteLength - total),
        null,
      );
      if (bytesRead === 0) break;
      total += bytesRead;
    }
    const growthProbe = Buffer.allocUnsafe(1);
    const { bytesRead: grewAfterStat } = await file.read(
      growthProbe,
      0,
      growthProbe.byteLength,
      null,
    );
    if (grewAfterStat !== 0) {
      throw new Error(
        `sealed publication plan exceeds its reviewed ${details.size}-byte size`,
      );
    }
    return body.subarray(0, total);
  } finally {
    await file.close();
  }
}

function declaredContentLength(
  response: Response,
  label: string,
  limit: number,
): number {
  const raw = response.headers.get('content-length');
  if (raw === null) {
    throw requestError(label, 'missing Content-Length');
  }
  if (!/^(0|[1-9][0-9]*)$/.test(raw)) {
    throw requestError(label, 'has an invalid Content-Length');
  }
  const length = Number(raw);
  if (!Number.isSafeInteger(length) || length > limit) {
    throw requestError(
      label,
      `declared body exceeds the ${limit}-byte limit`,
    );
  }
  return length;
}

function readChunk(
  reader: ReadableStreamDefaultReader<Uint8Array>,
  signal: AbortSignal,
  label: string,
): Promise<ReadableStreamReadResult<Uint8Array>> {
  if (signal.aborted) {
    return Promise.reject(
      signal.reason ?? requestError(label, 'publication deadline exceeded'),
    );
  }
  return new Promise((resolve, reject) => {
    const abort = () => {
      reject(
        signal.reason ?? requestError(label, 'publication deadline exceeded'),
      );
    };
    signal.addEventListener('abort', abort, { once: true });
    void reader.read().then(resolve, reject).finally(() => {
      signal.removeEventListener('abort', abort);
    });
  });
}

async function boundedBody(
  response: Response,
  label: string,
  limit: number,
  controller: AbortController,
): Promise<Uint8Array> {
  const declared = declaredContentLength(response, label, limit);
  if (response.body === null) {
    throw requestError(label, 'has no response body');
  }
  const reader = response.body.getReader();
  const body = new Uint8Array(declared);
  let received = 0;
  try {
    for (;;) {
      const chunk = await readChunk(reader, controller.signal, label);
      if (chunk.done) break;
      const offset = received;
      received += chunk.value.byteLength;
      if (received > limit) {
        const error = requestError(
          label,
          `streamed body exceeds the ${limit}-byte limit`,
        );
        controller.abort(error);
        void reader.cancel(error).catch(() => {});
        throw error;
      }
      if (received > declared) {
        throw requestError(
          label,
          `streamed length ${received} contradicts Content-Length ${declared}`,
        );
      }
      body.set(chunk.value, offset);
    }
    if (received !== declared) {
      throw requestError(
        label,
        `streamed length ${received} contradicts Content-Length ${declared}`,
      );
    }
    return body;
  } catch (error) {
    void reader.cancel(error).catch(() => {});
    throw error;
  } finally {
    try {
      reader.releaseLock();
    } catch {
      // A pending read retains its lock until cancellation settles.
    }
  }
}

export class R2FetchStore implements ConditionalReleaseStore {
  constructor(
    private readonly credentials: R2Credentials,
    private readonly bucket: string,
    private readonly fetchImplementation: R2Fetch = fetch,
  ) {}

  private async withinDeadline<T>(
    label: string,
    operation: (controller: AbortController) => Promise<T>,
  ): Promise<T> {
    const controller = new AbortController();
    let deadlineExpired = false;
    const timeout = setTimeout(() => {
      deadlineExpired = true;
      controller.abort(requestError(label, 'publication deadline exceeded'));
    }, REQUEST_DEADLINE_MS);
    try {
      return await operation(controller);
    } catch (error) {
      if (deadlineExpired) {
        throw requestError(label, 'publication deadline exceeded');
      }
      throw error;
    } finally {
      clearTimeout(timeout);
    }
  }

  async read(
    entry: PublicationEntry,
    desiredBytes: number,
  ): Promise<R2ObjectState | null> {
    const key = entry.key;
    const request = createSignedR2Request(
      this.credentials,
      this.bucket,
      'GET',
      key,
      new Uint8Array(),
      {},
    );
    return this.withinDeadline(`R2 read for ${key}`, async (controller) => {
      const response = await this.fetchImplementation(request.url, {
        ...request.init,
        redirect: 'error',
        signal: controller.signal,
      });
      let bodyRead = false;
      try {
        if (response.status >= 300 && response.status < 400) {
          throw new Error(`R2 read refused a redirect for ${key}`);
        }
        if (response.status === 404) return null;
        if (!response.ok) {
          throw new Error(`R2 read failed for ${key}: HTTP ${response.status}`);
        }
        const etag = response.headers.get('etag');
        if (!etag) throw new Error(`R2 object ${key} has no ETag`);
        if (
          !Number.isSafeInteger(desiredBytes) ||
          desiredBytes < 0 ||
          desiredBytes === Number.MAX_SAFE_INTEGER
        ) {
          throw new Error(`R2 object ${key} has an invalid desired byte length`);
        }
        const limit = entry.immutable
          ? desiredBytes + 1
          : STABLE_POINTER_MAX_BYTES;
        const body = await boundedBody(
          response,
          `R2 object ${key}`,
          limit,
          controller,
        );
        bodyRead = true;
        return { etag, body };
      } finally {
        if (!bodyRead) {
          void response.body?.cancel().catch(() => {});
        }
      }
    });
  }

  async put(
    entry: PublicationEntry,
    body: Uint8Array,
    expectedEtag?: string | null,
  ): Promise<void> {
    if (!entry.immutable && expectedEtag === undefined) {
      throw new Error(
        `mutable release pointer ${entry.key} requires an ETag condition`,
      );
    }
    const headers: Record<string, string> = {
      'content-type': entry.key.endsWith('.json')
        ? 'application/json'
        : entry.key.endsWith('.sig')
          ? 'text/plain; charset=utf-8'
          : 'application/octet-stream',
      'cache-control': entry.immutable
        ? 'public, max-age=31536000, immutable'
        : 'no-cache',
      ...(entry.immutable || expectedEtag === null
        ? { 'if-none-match': '*' }
        : expectedEtag
          ? { 'if-match': expectedEtag }
          : {}),
    };
    const request = createSignedR2Request(
      this.credentials,
      this.bucket,
      'PUT',
      entry.key,
      body,
      headers,
    );
    await this.withinDeadline(
      `R2 write for ${entry.key}`,
      async (controller) => {
        const response = await this.fetchImplementation(request.url, {
          ...request.init,
          redirect: 'error',
          signal: controller.signal,
        });
        if (response.status >= 300 && response.status < 400) {
          void response.body?.cancel().catch(() => {});
          throw new Error(`R2 write refused a redirect for ${entry.key}`);
        }
        if (!response.ok) {
          void response.body?.cancel().catch(() => {});
          throw new Error(
            `R2 write failed for ${entry.key}: HTTP ${response.status}`,
          );
        }
        void response.body?.cancel().catch(() => {});
      },
    );
  }
}
