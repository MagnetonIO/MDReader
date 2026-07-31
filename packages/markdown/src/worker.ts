import { parseMarkdown } from './pipeline.js';
import type { ParseOptions, ParsedDoc } from './types.js';

/** Message posted to the markdown worker. */
export interface WorkerRequest {
  id: number;
  source: string;
  opts?: ParseOptions;
}

/** Message posted back from the markdown worker. */
export interface WorkerResponse {
  id: number;
  doc?: ParsedDoc;
  error?: string;
}

interface WorkerScope {
  postMessage(message: unknown): void;
  addEventListener(type: 'message', listener: (event: { data: unknown }) => void): void;
}

/**
 * Detect a dedicated-worker global scope. `importScripts` is worker-only, so its
 * presence distinguishes a real worker from the main thread / Node (where
 * importing this module for its types must have no side effects).
 */
function getWorkerScope(): WorkerScope | undefined {
  const scope = globalThis as Partial<WorkerScope> & { importScripts?: unknown };
  if (
    typeof scope.postMessage === 'function' &&
    typeof scope.addEventListener === 'function' &&
    typeof scope.importScripts === 'function'
  ) {
    return scope as WorkerScope;
  }
  return undefined;
}

/**
 * This module is the worker ENTRY: it is the target of
 * `new URL('./worker.js', import.meta.url)` in {@link createMarkdownWorker}
 * (see `worker-client.ts`). It deliberately does NOT contain a `new Worker(...)`
 * of its own — the entry must not reference itself, or a bundler building the
 * worker chunk would recurse on the self-reference (Turbopack in particular
 * stalls on it). The main-thread factory lives separately in the `./worker`
 * subpath so the sync pipeline never drags this self-executing module in.
 */
const workerScope = getWorkerScope();
if (workerScope) {
  workerScope.addEventListener('message', (event) => {
    const { id, source, opts } = event.data as WorkerRequest;
    try {
      const doc = parseMarkdown(source, opts);
      workerScope.postMessage({ id, doc } satisfies WorkerResponse);
    } catch (cause) {
      workerScope.postMessage({
        id,
        error: cause instanceof Error ? cause.message : String(cause),
      } satisfies WorkerResponse);
    }
  });
}
