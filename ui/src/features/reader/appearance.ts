import type { CSSProperties } from 'react';
import type { ReadingFont } from '../../stores/settings';

/** Restyle the document without remounting its reader or changing its revision. */
export function readingAppearance(font: ReadingFont, scale: number): CSSProperties {
  return {
    '--text-0': `calc(1rem * ${scale})`,
    ...(font === 'sans' ? { '--font-reading-serif': 'var(--font-reading-sans)' } : {}),
  } as CSSProperties;
}
