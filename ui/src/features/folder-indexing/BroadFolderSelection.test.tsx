import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { BroadFolderSelection } from './BroadFolderSelection';
afterEach(cleanup);
test('home/disk selection requires explicit confirmation and changing scope clears that confirmation', () => {
  const choose = vi.fn(); render(<BroadFolderSelection busy={false} onChoose={choose} />);
  const details = screen.getByText('Advanced folder selection').closest('details')!;
  details.open = true;
  const button = screen.getByRole('button', { name: 'Choose folder…' }) as HTMLButtonElement;
  expect(button.disabled).toBe(true); expect(choose).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('checkbox'));
  fireEvent.click(button); expect(choose).toHaveBeenCalledExactlyOnceWith('home');
  fireEvent.change(screen.getByRole('combobox', { name: 'Scope' }), { target: { value: 'disk' } });
  expect(button.disabled).toBe(true);
  fireEvent.click(screen.getByRole('checkbox')); fireEvent.click(button);
  expect(choose).toHaveBeenLastCalledWith('disk');
});
