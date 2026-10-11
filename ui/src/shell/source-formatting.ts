import { EditorSelection, type EditorState, type TransactionSpec } from '@codemirror/state';
import type { ChordEvent } from './source-chord';

export type FormatCommand = 'bold' | 'italic' | 'link' | 'bullet-list' | 'ordered-list';

export function formatChord(isMac: boolean, event: ChordEvent): FormatCommand | null {
  const primary = isMac ? event.metaKey : event.ctrlKey;
  if (!primary || (isMac ? event.ctrlKey : event.metaKey) || event.altKey) return null;
  const key = event.key.toLowerCase();
  if (event.shiftKey) {
    if (key === '7' || key === '&') return 'ordered-list';
    if (key === '8' || key === '*') return 'bullet-list';
    return null;
  }
  return key === 'b' ? 'bold' : key === 'i' ? 'italic' : key === 'k' ? 'link' : null;
}

/** One CodeMirror transaction, so formatting participates in native undo/redo. */
export function formatTransaction(state: EditorState, command: FormatCommand): TransactionSpec {
  if (command === 'bullet-list' || command === 'ordered-list') {
    const lines = new Set<number>();
    for (const range of state.selection.ranges) {
      const first = state.doc.lineAt(range.from).number;
      const last = state.doc.lineAt(range.to > range.from ? range.to - 1 : range.to).number;
      for (let number = first; number <= last; number++) lines.add(number);
    }
    const sorted = [...lines].sort((a, b) => a - b);
    return { changes: sorted.map((number, index) => {
      const line = state.doc.line(number);
      const match = /^(\s*)(?:[-+*]|\d+[.)])\s+/.exec(line.text);
      const indent = match?.[1] ?? /^\s*/.exec(line.text)![0];
      const prefix = command === 'bullet-list' ? '- ' : `${index + 1}. `;
      return { from: line.from, to: line.from + (match?.[0].length ?? indent.length), insert: indent + prefix };
    }), userEvent: 'input.format' };
  }
  const marker = command === 'bold' ? '**' : '*';
  const transaction = state.changeByRange((range) => {
    const text = state.sliceDoc(range.from, range.to);
    const wrapped = command === 'link' ? `[${text || 'link text'}](url)` : marker + text + marker;
    const from = range.from + (command === 'link' ? 1 : marker.length);
    const to = from + (command === 'link' ? (text || 'link text').length : text.length);
    return { changes: { from: range.from, to: range.to, insert: wrapped }, range: EditorSelection.range(from, to) };
  });
  return { ...transaction, userEvent: 'input.format', scrollIntoView: true };
}
