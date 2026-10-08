import { act, cleanup, render } from '@testing-library/react';
import { EditorState } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { afterEach, expect, test, vi } from 'vitest';
import { editorAdmission, type EditorHold } from '../features/editor-preservation/admission';
import { SourcePane } from './SourcePane';
let held: EditorHold | null = null;
afterEach(() => { held?.release(); held = null; cleanup(); });

test('actual CodeMirror rejects document changes synchronously during admission and resumes without losing its buffer', () => {
  const onChange = vi.fn();
  const { container } = render(<SourcePane value="original" onChange={onChange} onSave={() => {}} onToggleMode={() => {}} />);
  const view = EditorView.findFromDOM(container.querySelector<HTMLElement>('.cm-editor')!)!;
  act(() => { held = editorAdmission.hold(); });
  expect(view.state.facet(EditorState.readOnly)).toBe(true);
  expect(view.contentDOM.getAttribute('aria-readonly')).toBe('true');
  act(() => { view.dispatch({ changes: { from: 0, to: 8, insert: 'must not appear' } }); });
  expect(view.state.doc.toString()).toBe('original');
  expect(onChange).not.toHaveBeenCalled();
  act(() => { held!.release(); });
  expect(view.state.facet(EditorState.readOnly)).toBe(false);
  act(() => { view.dispatch({ changes: { from: 0, to: 8, insert: 'ordinary edit' } }); });
  expect(view.state.doc.toString()).toBe('ordinary edit');
  expect(onChange).toHaveBeenCalledExactlyOnceWith('ordinary edit');
});
