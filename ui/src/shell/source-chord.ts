/**
 * The source editor's reserved-chord decision, factored out of `SourcePane` so it
 * is pure and testable on BOTH platforms without mounting CodeMirror.
 *
 * Only the platform's PRIMARY modifier is reserved — ⌘ on macOS, Ctrl elsewhere —
 * and only when no other modifier is held. So on macOS Ctrl+E (and Ctrl+S) are
 * left to CodeMirror (Ctrl+E is its native cursor-to-line-end); the toggle is ⌘E
 * and save is ⌘S. Elsewhere the chords are Ctrl+E / Ctrl+S.
 */

export interface ChordEvent {
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  key: string;
}

export type SourceChord = 'save' | 'toggle' | 'find' | 'undo' | 'redo' | null;

export function sourceChord(isMac: boolean, event: ChordEvent): SourceChord {
  const primary = isMac ? event.metaKey : event.ctrlKey;
  const secondary = isMac ? event.ctrlKey : event.metaKey;
  // Exactly the platform's primary modifier, nothing else.
  if (!primary || secondary || event.altKey) return null;
  const key = event.key.toLowerCase();
  if (event.shiftKey) return key === 'z' ? 'redo' : null;
  if (key === 'f') return 'find';
  if (key === 'z') return 'undo';
  if (!isMac && key === 'y') return 'redo';
  if (key === 's') return 'save';
  if (key === 'e') return 'toggle';
  return null;
}
