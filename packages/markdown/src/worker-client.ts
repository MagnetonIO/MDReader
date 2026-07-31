import { parseMarkdown } from './pipeline.js';
import { shouldUseWorker } from './thresholds.js';
import type { ParseOptions, ParsedDoc } from './types.js';
import type { WorkerRequest, WorkerResponse } from './worker.js';

/**
 * Create a `Worker` running the markdown parse pipeline (`./worker.js`, the
 * self-executing entry). This factory is the reason the worker lives on its own
 * `@mdreader/markdown/worker` subpath rather than the package root: the
 * construction is written as the LITERAL
 * `new Worker(new URL('./worker.js', import.meta.url), { type: 'module' })`
 * form on purpose so bundlers (Vite/webpack/Turbopack) recognise that exact
 * syntactic shape and emit a dedicated worker chunk with a resolved URL. Any
 * indirection (a captured constructor variable) defeats that static detection
 * and leaves the bundler inlining a raw `worker.js` with an unresolved relative
 * import. Because the literal must never enter a consumer that does not use the
 * worker (Next.js/Turbopack stalls building a server bundle that contains it),
 * it is confined to this subpath and is NOT re-exported from the package root.
 * Tests substitute a fake Worker via the `createWorker` factory parameter on
 * `parseMarkdownInWorker` / `parseMarkdownAuto` rather than reaching in here.
 *
 * Throws where Web Workers are unavailable (Node/SSR/tests); callers that want a
 * transparent fallback should use `parseMarkdownAuto` / `parseMarkdownInWorker`.
 */
export function createMarkdownWorker(): Worker {
  if (typeof Worker === 'undefined') {
    throw new Error('Web Workers are not available in this environment');
  }
  return new Worker(new URL('./worker.js', import.meta.url), { type: 'module' });
}

/** Factory that constructs the parse worker; injectable for tests. */
export type WorkerFactory = () => Worker;

function workerAvailable(): boolean {
  return typeof (globalThis as { Worker?: unknown }).Worker === 'function';
}

interface Pending {
  resolve: (doc: ParsedDoc) => void;
  reject: (error: Error) => void;
  /** The worker generation that owns this request. */
  generation: number;
}

let singleton: Worker | null = null;
const pending = new Map<number, Pending>();
let nextId = 0;
/**
 * Monotonic worker generation. A fresh worker is a new generation; every
 * listener and every pending request captures the generation it belongs to, so
 * a *replaced* worker firing a late `error` can only ever affect its own
 * generation — never the live worker or its in-flight requests.
 */
let currentGeneration = 0;

/**
 * Reject and drop every in-flight request that belongs to `generation`. Only
 * tears down the singleton if that generation is still the current one — a
 * stale worker's late failure must not terminate its replacement.
 */
function failGeneration(generation: number, error: Error): void {
  for (const [id, entry] of pending) {
    if (entry.generation !== generation) continue;
    entry.reject(error);
    pending.delete(id);
  }
  if (generation === currentGeneration && singleton) {
    singleton.terminate?.();
    singleton = null;
  }
}

function getWorker(createWorker: WorkerFactory): { worker: Worker; generation: number } {
  if (singleton) return { worker: singleton, generation: currentGeneration };

  const generation = ++currentGeneration;
  const worker = createWorker();

  worker.addEventListener('message', (event: MessageEvent<WorkerResponse>) => {
    const data = event.data;
    const entry = pending.get(data.id);
    // Ignore messages from a superseded worker, or for an already-settled id.
    if (!entry || entry.generation !== generation) return;
    pending.delete(data.id);
    if (data.error) {
      entry.reject(new Error(data.error));
    } else if (data.doc) {
      entry.resolve(data.doc);
    } else {
      entry.reject(new Error('Markdown worker returned no document'));
    }
  });
  // A crashed worker (uncaught error) or an unclonable message must reject the
  // pending promises instead of leaving them to hang forever — but only this
  // generation's, so a late error from a replaced worker is a no-op for the live
  // one.
  worker.addEventListener('error', (event: ErrorEvent) => {
    failGeneration(generation, new Error(event.message || 'Markdown worker error'));
  });
  worker.addEventListener('messageerror', () => {
    failGeneration(generation, new Error('Markdown worker received an unserializable message'));
  });

  singleton = worker;
  return { worker, generation };
}

/**
 * Parse markdown off the main thread. Same API and result shape as
 * {@link parseMarkdown}. Intended for sources at/above
 * {@link WORKER_BYTE_THRESHOLD}.
 *
 * A lazily-created singleton worker is reused across calls. In environments
 * without Web Workers (Node, SSR, tests) this transparently falls back to the
 * synchronous path so the API is universally callable. Pass `createWorker` to
 * substitute the worker construction (tests); production uses the default
 * {@link createMarkdownWorker}, whose literal `new Worker(new URL(...))` the
 * bundler can statically detect.
 */
export function parseMarkdownInWorker(
  source: string,
  opts: ParseOptions = {},
  createWorker: WorkerFactory = createMarkdownWorker,
): Promise<ParsedDoc> {
  if (!workerAvailable()) {
    return Promise.resolve(parseMarkdown(source, opts));
  }

  const { worker, generation } = getWorker(createWorker);
  const id = nextId++;

  return new Promise<ParsedDoc>((resolve, reject) => {
    pending.set(id, { resolve, reject, generation });
    try {
      worker.postMessage({ id, source, opts } satisfies WorkerRequest);
    } catch (cause) {
      pending.delete(id);
      reject(cause instanceof Error ? cause : new Error(String(cause)));
    }
  });
}

/**
 * The documented entry point that routes by size: sources below
 * {@link WORKER_BYTE_THRESHOLD} (64 KiB) parse synchronously on the calling
 * thread; larger sources parse in a Web Worker when one is available, falling
 * back to synchronous parsing otherwise. Always returns a promise so callers
 * have one uniform shape. `createWorker` is forwarded to
 * {@link parseMarkdownInWorker} for test injection.
 */
export function parseMarkdownAuto(
  source: string,
  opts: ParseOptions = {},
  createWorker: WorkerFactory = createMarkdownWorker,
): Promise<ParsedDoc> {
  const byteLength = new TextEncoder().encode(source).length;
  if (shouldUseWorker(byteLength) && workerAvailable()) {
    return parseMarkdownInWorker(source, opts, createWorker);
  }
  return Promise.resolve(parseMarkdown(source, opts));
}

/**
 * Test-only: tear down the singleton worker and clear pending state. Not part of
 * the package's public surface (not re-exported from the index). The generation
 * counter is intentionally left monotonic so a stale listener can never collide
 * with a future generation.
 */
export function resetMarkdownWorker(): void {
  pending.clear();
  if (singleton) {
    singleton.terminate?.();
    singleton = null;
  }
  nextId = 0;
}
