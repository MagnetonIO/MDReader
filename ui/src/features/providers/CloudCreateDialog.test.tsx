import { fireEvent, render, screen, cleanup } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { CloudCreateDialog } from './CloudCreateDialog';
import type { ProviderSource } from '../../platform/providers';
afterEach(cleanup);
const source: ProviderSource = { sourceId: 'public', readerSourceId: null, name: 'Public account', kind: 'dropbox', enabled: true, health: 'ready',
  selectedScope: { kind: 'scoped_root', path: '/Notes', recursive: true }, coverage: { kind: 'never_indexed' },
  capabilities: { read: true, list: true, changes: false, assets: false, guardedWrite: true } };
test('pending dialog retains filename and exposes Check status, with Escape/Close available even in flight', () => {
  const onClose = vi.fn(), onCreate = vi.fn(), onInspect = vi.fn();
  render(<CloudCreateDialog source={source} filename="Public.md" busy={false} pending retry={false} error={null} onName={vi.fn()} onClose={onClose} onCreate={onCreate} onInspect={onInspect} />);
  expect(screen.getByRole('dialog').getAttribute('aria-labelledby')).toBeTruthy();
  expect(screen.getByText('Public account · /Notes')).toBeTruthy();
  expect((screen.getByRole('textbox') as HTMLInputElement).disabled).toBe(true);
  expect(screen.queryByRole('button', { name: 'Create document' })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Check status' }));
  expect(onInspect).toHaveBeenCalledOnce();
  fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
  expect(onClose).toHaveBeenCalledOnce();
  expect(onCreate).not.toHaveBeenCalled();
});
test('invalid name cannot submit and a dispatched operation can close without cancelling its controller', () => {
  const onClose = vi.fn(), onCreate = vi.fn();
  const view = render(<CloudCreateDialog source={source} filename="../Private.md" busy={false} pending={false} retry={false} error={null} onName={vi.fn()} onClose={onClose} onCreate={onCreate} onInspect={vi.fn()} />);
  expect((screen.getByRole('button', { name: 'Create document' }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.submit(screen.getByRole('textbox').closest('form')!);
  expect(onCreate).not.toHaveBeenCalled();
  view.rerender(<CloudCreateDialog source={source} filename="Public.md" busy pending retry={false} error={null} onName={vi.fn()} onClose={onClose} onCreate={onCreate} onInspect={vi.fn()} />);
  fireEvent.click(screen.getByRole('button', { name: 'Close' }));
  expect(onClose).toHaveBeenCalledOnce();
});
