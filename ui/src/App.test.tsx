import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, test } from 'vitest';

import App from './App';

afterEach(cleanup);

test('App renders the MDReader heading and the shared ReaderPlaceholder', () => {
  render(<App />);
  expect(screen.getByText('MDReader')).toBeTruthy();
  expect(screen.getByText('Reader placeholder')).toBeTruthy();
});

test('App exposes the IPC ping affordance without invoking it on mount', () => {
  render(<App />);
  // The button proves the IPC path is wired; it must not fire on render (no
  // Tauri runtime exists in the jsdom test environment).
  expect(screen.getByRole('button', { name: 'Ping backend' })).toBeTruthy();
  expect(screen.queryByTestId('ping-result')).toBeNull();
});
