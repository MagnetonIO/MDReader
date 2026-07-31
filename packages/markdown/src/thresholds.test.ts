import { describe, expect, test } from 'vitest';

import {
  VIRTUALIZATION_BLOCK_THRESHOLD,
  WORKER_BYTE_THRESHOLD,
  shouldUseWorker,
  shouldVirtualize,
} from './thresholds.js';

describe('threshold logic', () => {
  test('virtualization threshold is 300 blocks', () => {
    expect(VIRTUALIZATION_BLOCK_THRESHOLD).toBe(300);
  });

  test('shouldVirtualize triggers strictly above the threshold', () => {
    expect(shouldVirtualize({ blockCount: 299 })).toBe(false);
    expect(shouldVirtualize({ blockCount: 300 })).toBe(false);
    expect(shouldVirtualize({ blockCount: 301 })).toBe(true);
  });

  test('worker byte threshold is 64 KiB', () => {
    expect(WORKER_BYTE_THRESHOLD).toBe(64 * 1024);
  });

  test('shouldUseWorker triggers at or above the threshold', () => {
    expect(shouldUseWorker(WORKER_BYTE_THRESHOLD - 1)).toBe(false);
    expect(shouldUseWorker(WORKER_BYTE_THRESHOLD)).toBe(true);
    expect(shouldUseWorker(WORKER_BYTE_THRESHOLD + 1)).toBe(true);
  });
});
