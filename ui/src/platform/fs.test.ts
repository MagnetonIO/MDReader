import { describe, expect, it } from 'vitest';

import { parseSaveOutcome } from './fs.js';
import type { SaveOutcome as SaveOutcomeDto } from '../bindings';

function dto(partial: Partial<SaveOutcomeDto> & { status: string }): SaveOutcomeDto {
  return { new_hash: null, mtime: null, current_hash: null, ...partial };
}

describe('parseSaveOutcome — validated discriminated union', () => {
  it('parses a saved outcome', () => {
    expect(parseSaveOutcome(dto({ status: 'saved', new_hash: 'h1', mtime: 42 }))).toEqual({
      status: 'saved',
      newHash: 'h1',
      mtime: 42,
    });
  });

  it('parses a conflict outcome', () => {
    expect(parseSaveOutcome(dto({ status: 'conflict', current_hash: 'hDisk' }))).toEqual({
      status: 'conflict',
      currentHash: 'hDisk',
    });
  });

  it('an unrecognised status is a hard error, not a conflict-with-empty-hash', () => {
    expect(() => parseSaveOutcome(dto({ status: 'exploded' }))).toThrow(/unrecognised outcome status/i);
  });

  it('a saved outcome missing its new hash is an error', () => {
    expect(() => parseSaveOutcome(dto({ status: 'saved', mtime: 1 }))).toThrow(/without a new hash/i);
  });

  it('a conflict outcome missing the on-disk hash is an error', () => {
    expect(() => parseSaveOutcome(dto({ status: 'conflict' }))).toThrow(/without the on-disk hash/i);
  });
});
