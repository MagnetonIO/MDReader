import type { ChordEvent } from './source-chord';

/** Match printed plus/equal/minus, including the shifted plus and keypad keys. */
export function textSizeChord(isMac: boolean, event: ChordEvent & { code?: string }): 'increase' | 'decrease' | null {
  const primary = isMac ? event.metaKey : event.ctrlKey;
  const secondary = isMac ? event.ctrlKey : event.metaKey;
  if (!primary || secondary || event.altKey) return null;
  if (event.key === '+' || event.key === '=' || event.code === 'NumpadAdd') return 'increase';
  if (event.key === '-' || event.code === 'NumpadSubtract') return 'decrease';
  return null;
}
