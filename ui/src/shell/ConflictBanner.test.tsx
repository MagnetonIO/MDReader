import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ConflictBanner } from './ConflictBanner.js';

afterEach(cleanup);

function handlers() {
  return {
    onReload: vi.fn(),
    onKeepMine: vi.fn(),
    onOverwrite: vi.fn(),
    onCancel: vi.fn(),
  };
}

describe('ConflictBanner — both entry paths', () => {
  it('a save-time conflict explains the file changed since it was opened', () => {
    render(<ConflictBanner source="save" phase="choose" {...handlers()} />);
    expect(screen.getByRole('alert').textContent).toMatch(/changed on disk since you opened it/i);
    expect(screen.getByRole('button', { name: 'Reload' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Keep mine' })).toBeTruthy();
  });

  it('an external change while editing uses the watcher-flavoured copy', () => {
    render(<ConflictBanner source="external" phase="choose" {...handlers()} />);
    expect(screen.getByRole('alert').textContent).toMatch(/changed on disk while you were editing/i);
  });
});

describe('ConflictBanner — choose actions', () => {
  it('Reload and Keep mine call their handlers; Keep mine never overwrites directly', () => {
    const h = handlers();
    render(<ConflictBanner source="save" phase="choose" {...h} />);

    fireEvent.click(screen.getByRole('button', { name: 'Reload' }));
    expect(h.onReload).toHaveBeenCalledOnce();

    fireEvent.click(screen.getByRole('button', { name: 'Keep mine' }));
    expect(h.onKeepMine).toHaveBeenCalledOnce();
    // The overwrite must wait for the explicit second confirmation.
    expect(h.onOverwrite).not.toHaveBeenCalled();
  });
});

describe('ConflictBanner — explicit overwrite confirmation', () => {
  it('the confirm phase asks before overwriting and wires Overwrite / Cancel', () => {
    const h = handlers();
    render(<ConflictBanner source="save" phase="confirm" {...h} />);

    expect(screen.getByRole('alert').textContent).toMatch(/overwrite the on-disk changes\?/i);

    fireEvent.click(screen.getByRole('button', { name: 'Overwrite' }));
    expect(h.onOverwrite).toHaveBeenCalledOnce();

    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(h.onCancel).toHaveBeenCalledOnce();
  });
});
