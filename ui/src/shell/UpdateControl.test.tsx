import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, test, vi } from 'vitest';

import { UpdateControl, type UpdateAdapter } from './UpdateControl.js';

describe('UpdateControl', () => {
  test('does not expose updater UI in store, E2E, dev, or mobile builds', async () => {
    const adapter: UpdateAdapter = {
      capability: vi.fn(async () => false),
      check: vi.fn(),
      install: vi.fn(),
    };
    const { container } = render(<UpdateControl adapter={adapter} />);

    await waitFor(() => expect(adapter.capability).toHaveBeenCalledOnce());
    expect(container.textContent).toBe('');
    expect(adapter.check).not.toHaveBeenCalled();
  });

  test('checks, presents the signed version, and installs only after explicit consent', async () => {
    const adapter: UpdateAdapter = {
      capability: vi.fn(async () => true),
      check: vi.fn(async () => ({
        version: '1.1.0',
        currentVersion: '1.0.0',
        notes: 'Search and indexing improvements.',
      })),
      install: vi.fn(async () => undefined),
    };
    render(<UpdateControl adapter={adapter} />);

    const check = await screen.findByRole('button', { name: 'Check for updates' });
    fireEvent.click(check);
    expect(await screen.findByText('MDReader 1.1.0 is available')).toBeTruthy();
    expect(adapter.install).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'Install update 1.1.0' }));
    await waitFor(() => expect(adapter.install).toHaveBeenCalledWith('1.1.0'));
    expect(await screen.findByText('Update installed. Restart MDReader to finish.')).toBeTruthy();
  });

  test('surfaces a failed check instead of presenting no update', async () => {
    const adapter: UpdateAdapter = {
      capability: vi.fn(async () => true),
      check: vi.fn(async () => {
        throw new Error('signature rejected');
      }),
      install: vi.fn(),
    };
    render(<UpdateControl adapter={adapter} />);

    fireEvent.click(await screen.findByRole('button', { name: 'Check for updates' }));
    expect((await screen.findByRole('alert')).textContent).toBe(
      'Could not check for updates — signature rejected',
    );
  });
});
