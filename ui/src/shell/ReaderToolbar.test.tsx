import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { ReaderToolbar } from './ReaderToolbar';
afterEach(cleanup);
const props = {
  docRef: { path: '/public/plans/plan.md', name: 'plan.md' }, root: '/public/plans', outlineVisible: false,
  onToggleOutline: vi.fn(), hasOutline: false, mode: 'read' as const, onSetMode: vi.fn(), canEdit: true,
};
test('a read-only source offers no Edit entry; a writable one does', () => {
  const view = render(<ReaderToolbar {...props} readOnly />);
  expect(screen.queryByRole('button', { name: 'Edit' })).toBeNull();
  view.rerender(<ReaderToolbar {...props} />);
  expect(screen.getByRole('button', { name: 'Edit' })).toBeTruthy();
});
