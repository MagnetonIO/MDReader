import { describe, expect, it } from 'vitest';
import { EditorSelection, EditorState } from '@codemirror/state';
import { history, undo } from '@codemirror/commands';
import { formatChord, formatTransaction } from './source-formatting';
import { textSizeChord } from './text-size-chord';

const ev = (key: string, extra = {}) => ({ key, metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, ...extra });
describe('desktop Markdown shortcuts and formatting', () => {
  it('reserves only the host modifier including shifted plus and keypad', () => {
    expect(textSizeChord(true, ev('+', { metaKey: true, shiftKey: true }))).toBe('increase');
    expect(textSizeChord(false, ev('=', { ctrlKey: true }))).toBe('increase');
    expect(textSizeChord(false, ev('-', { ctrlKey: true }))).toBe('decrease');
    expect(textSizeChord(true, ev('Add', { metaKey: true, code: 'NumpadAdd' }))).toBe('increase');
    expect(textSizeChord(true, ev('+', { ctrlKey: true }))).toBeNull();
    expect(textSizeChord(false, ev('+', { metaKey: true }))).toBeNull();
    expect(textSizeChord(false, ev('+', { ctrlKey: true, altKey: true }))).toBeNull();
    expect(textSizeChord(false, ev('+'))).toBeNull();
    expect(formatChord(true, ev('k', { metaKey: true }))).toBe('link');
    expect(formatChord(false, ev('*', { ctrlKey: true, shiftKey: true }))).toBe('bullet-list');
  });
  it('formats independent selections as one undoable edit', () => {
    let state = EditorState.create({ doc: 'one two', selection: EditorSelection.create([EditorSelection.range(0, 3), EditorSelection.range(4, 7)]), extensions: [EditorState.allowMultipleSelections.of(true), history()] });
    state = state.update(formatTransaction(state, 'bold')).state;
    expect(state.doc.toString()).toBe('**one** **two**');
    expect(state.selection.ranges.map((r) => state.sliceDoc(r.from, r.to))).toEqual(['one', 'two']);
    undo({ state, dispatch: (transaction) => { state = transaction.state; } });
    expect(state.doc.toString()).toBe('one two');
  });
  it('changes only selected list lines and retains indentation', () => {
    const state = EditorState.create({ doc: 'intro\n  - first\n  - second\noutro', selection: EditorSelection.range(6, 28) });
    expect(state.update(formatTransaction(state, 'ordered-list')).state.doc.toString()).toBe('intro\n  1. first\n  2. second\noutro');
  });
});
