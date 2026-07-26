const MAX_REQUEST_BYTES = 64 * 1024;
const MAX_RESPONSE_BYTES = 2 * 1024 * 1024;
const DEFAULT_TIMEOUT_MS = 8_000;

export interface ForwardLimits {
  timeoutMs?: number;
}

type UpstreamFetch = (request: Request) => Promise<Response>;

function jsonError(error: string, message: string, status: number): Response {
  return Response.json(
    { error, message },
    {
      status,
      headers: {
        'cache-control': 'no-store',
        'x-content-type-options': 'nosniff',
      },
    },
  );
}

async function readBodyBounded(
  request: Request,
  signal: AbortSignal,
): Promise<ArrayBuffer | null> {
  if (!request.body) return null;
  const reader = request.body.getReader();
  const cancel = () => void reader.cancel('request deadline exceeded');
  signal.addEventListener('abort', cancel, { once: true });
  const chunks: Uint8Array[] = [];
  let total = 0;
  try {
    for (;;) {
      const part = await reader.read();
      if (signal.aborted) throw new DOMException('demo deadline exceeded', 'TimeoutError');
      if (part.done) break;
      total += part.value.byteLength;
      if (total > MAX_REQUEST_BYTES) {
        await reader.cancel('request limit exceeded');
        throw new RangeError('request_too_large');
      }
      chunks.push(part.value);
    }
  } finally {
    signal.removeEventListener('abort', cancel);
    reader.releaseLock();
  }
  const body = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return body.buffer;
}

async function readResponseBounded(
  response: Response,
  signal: AbortSignal,
): Promise<Response> {
  const declared = response.headers.get('content-length');
  if (declared !== null) {
    const size = Number(declared);
    if (!Number.isSafeInteger(size) || size < 0 || size > MAX_RESPONSE_BYTES) {
      await response.body?.cancel('response limit exceeded');
      throw new RangeError('response_too_large');
    }
  }
  if (!response.body) return new Response(null, { status: response.status });

  const reader = response.body.getReader();
  const cancel = () => void reader.cancel('response deadline exceeded');
  signal.addEventListener('abort', cancel, { once: true });
  const chunks: Uint8Array[] = [];
  let total = 0;
  try {
    for (;;) {
      const part = await reader.read();
      if (signal.aborted) throw new DOMException('demo deadline exceeded', 'TimeoutError');
      if (part.done) break;
      total += part.value.byteLength;
      if (total > MAX_RESPONSE_BYTES) {
        await reader.cancel('response limit exceeded');
        throw new RangeError('response_too_large');
      }
      chunks.push(part.value);
    }
  } finally {
    signal.removeEventListener('abort', cancel);
    reader.releaseLock();
  }
  const body = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  const headers = new Headers();
  for (const name of ['content-type', 'cache-control']) {
    const value = response.headers.get(name);
    if (value) headers.set(name, value);
  }
  headers.set('content-length', String(total));
  headers.set('x-content-type-options', 'nosniff');
  return new Response(body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  });
}

/**
 * Forward one already-allowlisted request with request, response, and elapsed
 * limits that apply on the direct Container Worker path.
 */
export async function forwardPublicRequest(
  request: Request,
  upstream: UpstreamFetch,
  limits: ForwardLimits = {},
): Promise<Response> {
  const timeoutMs = limits.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  if (!Number.isSafeInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > DEFAULT_TIMEOUT_MS) {
    return jsonError('invalid_limit', 'The request limit is invalid.', 500);
  }
  const declared = request.headers.get('content-length');
  if (declared !== null) {
    const size = Number(declared);
    if (!Number.isSafeInteger(size) || size < 0 || size > MAX_REQUEST_BYTES) {
      return jsonError('request_too_large', 'The request exceeds the demo limit.', 413);
    }
  }

  const controller = new AbortController();
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const operation = (async () => {
      const body = await readBodyBounded(request, controller.signal);
      const headers = new Headers(request.headers);
      headers.delete('content-length');
      const bounded = new Request(request.url, {
        method: request.method,
        headers,
        body,
        signal: controller.signal,
      });
      const response = await upstream(bounded);
      return readResponseBounded(response, controller.signal);
    })();
    const timeout = new Promise<never>((_, reject) => {
      timer = setTimeout(() => {
        controller.abort('demo deadline exceeded');
        reject(new DOMException('demo deadline exceeded', 'TimeoutError'));
      }, timeoutMs);
    });
    return await Promise.race([operation, timeout]);
  } catch (error) {
    if (error instanceof RangeError && error.message === 'request_too_large') {
      return jsonError('request_too_large', 'The request exceeds the demo limit.', 413);
    }
    if (error instanceof RangeError && error.message === 'response_too_large') {
      return jsonError('response_too_large', 'The demo response exceeded its limit.', 502);
    }
    if (error instanceof DOMException && error.name === 'TimeoutError') {
      return jsonError('upstream_timeout', 'The demo request exceeded its time limit.', 504);
    }
    return jsonError('upstream_failure', 'The demo runtime could not answer.', 502);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
    controller.abort('request complete');
  }
}
