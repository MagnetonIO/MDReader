import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import type { NativeProvidersBackend } from '../../platform/providers';
import { DriveFolderBrowser } from './DriveFolderBrowser';

afterEach(cleanup);

test('folders beyond 1,000 remain reachable and previous pages are available without retaining a growing listing', async () => {
  const browseFolders = vi.fn(async (_source: string, _folder: string, cursor: string | null) => {
    const page = cursor === null ? 0 : Number(cursor);
    return { operation: 'browse_folders' as const,
      folders: Array.from({ length: page === 4 ? 100 : 256 }, (_, index) => ({
        id: `folder-${page * 256 + index}`, name: `Folder ${page * 256 + index}`,
      })), nextCursor: page === 4 ? null : String(page + 1) };
  });
  const onChoose = vi.fn();
  render(<DriveFolderBrowser sourceId="admitted-drive" backend={{ browseFolders } as unknown as NativeProvidersBackend} busy={false} onChoose={onChoose} />);
  await screen.findByRole('button', { name: 'Browse Folder 0' });
  for (let page = 1; page <= 4; page++) {
    fireEvent.click(screen.getByRole('button', { name: 'Next folders' }));
    await screen.findByRole('button', { name: `Browse Folder ${page * 256}` });
    expect(screen.queryByRole('button', { name: 'Browse Folder 0' })).toBeNull();
  }
  expect(screen.getByRole('button', { name: 'Browse Folder 1123' })).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Next folders' })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Previous folders' }));
  await screen.findByRole('button', { name: 'Browse Folder 768' });
  expect(screen.queryByRole('button', { name: 'Browse Folder 1123' })).toBeNull();
  await waitFor(() => expect(browseFolders).toHaveBeenLastCalledWith('admitted-drive', 'root', '3'));
  expect(onChoose).not.toHaveBeenCalled();
});
