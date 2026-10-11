import { cleanup, fireEvent, render, renderHook, screen, act } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { EditorToolbar } from './EditorToolbar';
import { useEditorView } from './useEditorView';
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
test('editor views and Save are direct actions; copy/saved-version/format are dismissed through overflow', () => {
  const onSetMode = vi.fn(), onRead = vi.fn(), onSave = vi.fn(), onSaveCopy = vi.fn();
  const commandsRef = { current: { format: vi.fn(() => true), focus: vi.fn() } };
  const view = render(<EditorToolbar title="Public.md" mode="source" dirty saving={false} saveDisabled={false} saveLabel="Save" commandsRef={commandsRef}
    onSetMode={onSetMode} onRead={onRead} onSave={onSave} onSaveCopy={onSaveCopy} />);
  expect(screen.getByRole('button', { name: 'Edit' }).getAttribute('aria-pressed')).toBe('true');
  fireEvent.click(screen.getByRole('button', { name: 'Preview' })); expect(onSetMode).toHaveBeenCalledWith('preview');
  fireEvent.click(screen.getByRole('button', { name: 'Save' })); expect(onSave).toHaveBeenCalledOnce();
  const summary = screen.getByLabelText('More editor actions'), menu = summary.closest('details')!;
  expect(menu.open).toBe(false); menu.open = true;
  fireEvent.click(screen.getByRole('button', { name: 'Bold' }));
  expect(commandsRef.current.format).toHaveBeenCalledWith('bold'); expect(commandsRef.current.focus).toHaveBeenCalledOnce(); expect(menu.open).toBe(false);
  menu.open = true; fireEvent.click(screen.getByRole('button', { name: 'Save a copy…' })); expect(onSaveCopy).toHaveBeenCalledOnce(); expect(onRead).not.toHaveBeenCalled();
  menu.open = true; fireEvent.keyDown(menu, { key: 'Escape' }); expect(menu.open).toBe(false); expect(document.activeElement).toBe(summary);
  view.rerender(<EditorToolbar title="Public.md" mode="preview" narrow dirty saving={false} saveDisabled={false} saveLabel="Save" commandsRef={commandsRef}
    onSetMode={onSetMode} onRead={onRead} onSave={onSave} onSaveCopy={onSaveCopy} />);
  expect(screen.queryByRole('button', { name: 'Split' })).toBeNull(); expect(screen.queryByRole('button', { name: 'Bold' })).toBeNull();
});
test('narrow layout adapts Split without changing the remembered desktop view', () => {
  let changed!: () => void;
  const media = { matches: false, addEventListener: vi.fn((_event, listener) => { changed = listener; }), removeEventListener: vi.fn() };
  vi.stubGlobal('matchMedia', () => media);
  const view = renderHook(() => useEditorView('split'));
  expect(view.result.current.view).toBe('split');
  act(() => { media.matches = true; changed(); }); expect(view.result.current.view).toBe('source');
  act(() => { media.matches = false; changed(); }); expect(view.result.current.view).toBe('split');
  view.unmount(); expect(media.removeEventListener).toHaveBeenCalled();
});
