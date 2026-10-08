import { act, cleanup, render } from '@testing-library/react';
import { EditorView } from '@codemirror/view';
import { afterEach, expect, test, vi } from 'vitest';
import { SourcePane } from './SourcePane';
import { useEditorStore } from '../stores/editor';

afterEach(() => { cleanup(); useEditorStore.getState().reset(); });

test('opening a BOM/CRLF source keeps its raw baseline clean and actual edits retain its encoding convention', () => {
  const original = '\ufeff# Welcome\r\n\r\nOriginal bytes.\r\n';
  const store = useEditorStore.getState();
  store.seed('tab', '/welcome.md', original, 'raw-revision', 'source');
  const changed = vi.fn((value: string) => useEditorStore.getState().edit('tab', '/welcome.md', value));
  const { container } = render(<SourcePane value={original} onChange={changed} onSave={() => {}} onToggleMode={() => {}} />);
  const view = EditorView.findFromDOM(container.querySelector<HTMLElement>('.cm-editor')!)!;
  expect(changed).not.toHaveBeenCalled();
  expect(view.state.doc.toString()).toBe(original.replace(/\r\n/g, '\n'));
  expect(useEditorStore.getState().entry('tab', '/welcome.md')).toMatchObject({ buffer: original, savedContent: original, baseHash: 'raw-revision', dirty: false });
  act(() => { view.dispatch({ changes: { from: view.state.doc.length, insert: 'Added.\n' } }); });
  expect(changed).toHaveBeenCalledExactlyOnceWith(original + 'Added.\r\n');
  expect(useEditorStore.getState().entry('tab', '/welcome.md')).toMatchObject({ buffer: original + 'Added.\r\n', savedContent: original, baseHash: 'raw-revision', dirty: true });
});

test('external reloads do not manufacture edits, including mixed newlines and a changed separator', () => {
  const changed = vi.fn();
  const props = { onChange: changed, onSave: () => {}, onToggleMode: () => {} };
  const { container, rerender } = render(<SourcePane value={'\ufeffone\r\ntwo\nthree\r\n'} {...props} />);
  const view = EditorView.findFromDOM(container.querySelector<HTMLElement>('.cm-editor')!)!;
  expect(changed).not.toHaveBeenCalled();
  rerender(<SourcePane value={'\ufeffone\ntwo\nthree\n'} {...props} />);
  expect(changed).not.toHaveBeenCalled();
  expect(view.state.sliceDoc()).toBe('\ufeffone\ntwo\nthree\n');
});
