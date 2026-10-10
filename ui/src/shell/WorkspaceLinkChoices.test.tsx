import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { WorkspaceLinkChoices } from './WorkspaceLinkChoices';

afterEach(cleanup);
const candidates = [
  { path: '/public/notes/project-a/index.md', name: 'index.md' },
  { path: '/public/notes/project-b/index.md', name: 'index.md' },
];

test('compact choices distinguish folders and return exact identities without machine path labels', () => {
  const choose = vi.fn();
  const view = render(<WorkspaceLinkChoices root="/public/notes" candidates={candidates}
    onChoose={choose} onDismiss={vi.fn()} canRestoreFocus={() => true} />);
  expect(view.container.textContent).not.toContain('/public');
  const first = screen.getByRole('button', { name: 'index.md — project-a' });
  expect(document.activeElement).toBe(first);
  fireEvent.click(screen.getByRole('button', { name: 'index.md — project-b' }));
  expect(choose).toHaveBeenCalledExactlyOnceWith(candidates[1]);
});

test('Escape dismisses and restores the originating link without scrolling; changed origins cannot steal focus', () => {
  const origin = document.createElement('a'); origin.href = '#public'; document.body.append(origin); origin.focus();
  const focus = vi.spyOn(origin, 'focus'); const dismiss = vi.fn();
  const view = render(<WorkspaceLinkChoices root="/public/notes" candidates={candidates}
    onChoose={vi.fn()} onDismiss={dismiss} canRestoreFocus={() => true} />);
  fireEvent.keyDown(screen.getByRole('button', { name: 'index.md — project-a' }), { key: 'Escape' });
  expect(dismiss).toHaveBeenCalledTimes(1);
  view.unmount();
  expect(focus).toHaveBeenCalledWith({ preventScroll: true });
  focus.mockClear();
  const stale = render(<WorkspaceLinkChoices root="/public/notes" candidates={candidates}
    onChoose={vi.fn()} onDismiss={vi.fn()} canRestoreFocus={() => false} />);
  stale.unmount(); expect(focus).not.toHaveBeenCalled(); origin.remove();
});

test('Windows context is display-only and outside-root candidates do not expose parent directories', () => {
  const windows = { path: 'C:\\Public\\Notes\\Project\\README.md', name: 'README.md' };
  const choose = vi.fn();
  const view = render(<WorkspaceLinkChoices root={'c:\\public\\notes'} candidates={[windows,
    { path: '/outside/private/location/README.md', name: 'README.md' }]}
    onChoose={choose} onDismiss={vi.fn()} canRestoreFocus={() => false} />);
  fireEvent.click(screen.getByRole('button', { name: 'README.md — Project' }));
  expect(choose).toHaveBeenCalledWith(windows);
  expect(screen.getByRole('button', { name: 'README.md — Selected source' }).title).toBe('README.md');
  expect(view.container.textContent).not.toMatch(/C:|outside|private|location/);
});
