import { describe, expect, it, vi } from 'vitest';

import { classifyExternalChange, runSave } from './edit-save.js';
import type { SaveOutcome } from './edit-save.js';

describe('classifyExternalChange — self-write suppression vs conflict', () => {
  it('a disk hash equal to our anchor is our own write — suppress', () => {
    // After a save, baseHash === the freshly-written hash; the watcher echo of
    // that very write hashes back to the same value.
    expect(classifyExternalChange('hSaved', 'hSaved', false)).toBe('self');
    // True even if the buffer is (somehow) still dirty — same bytes on disk.
    expect(classifyExternalChange('hSaved', 'hSaved', true)).toBe('self');
  });

  it('a divergent disk with a clean buffer is a safe reload', () => {
    expect(classifyExternalChange('hDisk', 'hBase', false)).toBe('reload');
  });

  it('a divergent disk with unsaved edits is a conflict — never clobber', () => {
    expect(classifyExternalChange('hDisk', 'hBase', true)).toBe('conflict');
  });
});

describe('runSave — outcome normalisation', () => {
  it('a saved outcome carries the fresh hash + mtime', async () => {
    const save = vi.fn(
      async (): Promise<SaveOutcome> => ({ status: 'saved', newHash: 'h1', mtime: 42 }),
    );
    const result = await runSave({
      path: '/ws/note.md',
      buffer: '# New',
      baseHash: 'h0',
      force: false,
      save,
    });
    expect(result).toEqual({ kind: 'saved', newHash: 'h1', mtime: 42 });
    // The guard anchor (baseHash) is what the backend is asked to verify.
    expect(save).toHaveBeenCalledWith('/ws/note.md', '# New', 'h0', { force: false });
  });

  it('a conflict outcome is surfaced (not thrown) with the on-disk hash', async () => {
    const save = vi.fn(
      async (): Promise<SaveOutcome> => ({ status: 'conflict', currentHash: 'hDisk' }),
    );
    const result = await runSave({
      path: '/ws/note.md',
      buffer: 'mine',
      baseHash: 'hStale',
      force: false,
      save,
    });
    expect(result).toEqual({ kind: 'conflict', currentHash: 'hDisk' });
  });

  it('force is threaded through to the backend (explicit overwrite)', async () => {
    const save = vi.fn(
      async (): Promise<SaveOutcome> => ({ status: 'saved', newHash: 'h2', mtime: 1 }),
    );
    await runSave({ path: '/ws/n.md', buffer: 'x', baseHash: 'hStale', force: true, save });
    expect(save).toHaveBeenCalledWith('/ws/n.md', 'x', 'hStale', { force: true });
  });

  it('a thrown backend/IPC failure becomes an error result', async () => {
    const save = vi.fn(async (): Promise<SaveOutcome> => {
      throw new Error('path escapes the workspace');
    });
    const result = await runSave({
      path: '/etc/passwd',
      buffer: 'x',
      baseHash: 'h',
      force: false,
      save,
    });
    expect(result).toEqual({ kind: 'error', message: 'path escapes the workspace' });
  });
});
