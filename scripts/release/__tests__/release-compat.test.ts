import { describe, expect, test } from 'vitest';

import {
  assertGlibcCompatibility,
  highestRequiredGlibc,
} from '../platform-compat';

describe('release platform compatibility', () => {
  test('derives the highest GLIBC symbol required by the built payload', () => {
    const readelf = `
      0x0010: Name: GLIBC_2.17
      0x0020: Name: GLIBC_2.31
      0x0030: Name: GLIBC_2.28
    `;
    expect(highestRequiredGlibc(readelf)).toBe('2.31');
    expect(() => assertGlibcCompatibility(readelf, '2.31')).not.toThrow();
  });

  test('refuses artifacts built against a newer libc than the published minimum', () => {
    expect(() =>
      assertGlibcCompatibility('Name: GLIBC_2.34', '2.31'),
    ).toThrow(/requires GLIBC 2.34/);
  });

  test('refuses to claim compatibility when no GLIBC requirement was measured', () => {
    expect(() => assertGlibcCompatibility('not an ELF version report', '2.31')).toThrow(
      /no GLIBC symbols/,
    );
  });
});
