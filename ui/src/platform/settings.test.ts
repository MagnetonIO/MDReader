import { describe, expect, it, vi } from 'vitest';

import type { DiagnosticEvent } from '../bindings';
import { recordDiagnosticEventNonBlocking } from './settings.js';

describe('local diagnostics transport', () => {
  it('never rejects or stalls the product action that emitted the event', async () => {
    const event: DiagnosticEvent = { kind: 'application-started' };
    const write = vi.fn(async () => {
      throw new Error('disk full');
    });

    expect(() => recordDiagnosticEventNonBlocking(event, write)).not.toThrow();
    await Promise.resolve();

    expect(write).toHaveBeenCalledWith(event);
  });

  it('does not retain arbitrary payloads outside the closed event vocabulary', () => {
    const source = recordDiagnosticEventNonBlocking.toString();

    expect(source).not.toMatch(/query|document|content|path/i);
  });
});
