import type { DocStats } from './types.js';

/**
 * Source size (UTF-8 bytes) at or above which parsing should be offloaded to a
 * worker to keep the main thread responsive. Below this, the synchronous path
 * is used.
 */
export const WORKER_BYTE_THRESHOLD = 64 * 1024;

/**
 * Top-level block count above which the reader should virtualize rendering
 * (render only visible blocks) instead of rendering everything.
 */
export const VIRTUALIZATION_BLOCK_THRESHOLD = 300;

/** Whether a document with the given stats should be virtualized. */
export function shouldVirtualize(stats: Pick<DocStats, 'blockCount'>): boolean {
  return stats.blockCount > VIRTUALIZATION_BLOCK_THRESHOLD;
}

/** Whether a source of the given UTF-8 byte length should parse in a worker. */
export function shouldUseWorker(byteLength: number): boolean {
  return byteLength >= WORKER_BYTE_THRESHOLD;
}
