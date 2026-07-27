import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';

import { useTabsStore } from '../stores/tabs.js';
import { TabStrip } from './TabStrip.js';

afterEach(() => {
  cleanup();
  useTabsStore.getState().reset();
});

it('announces local-tab activation and close before changing the tab model', () => {
  useTabsStore.getState().openTab({ path: '/notes/one.md', name: 'one.md' });
  const oneId = useTabsStore.getState().activeTabId;
  useTabsStore.getState().openTab({ path: '/notes/two.md', name: 'two.md' });
  const twoId = useTabsStore.getState().activeTabId;
  const activeAtIntent: Array<string | null> = [];
  const calls: string[] = [];
  const onLocalIntent = vi.fn(() => {
    calls.push('local');
    activeAtIntent.push(useTabsStore.getState().activeTabId);
  });
  const onRequestClose = vi.fn(() => calls.push('close'));

  render(<TabStrip onLocalIntent={onLocalIntent} onRequestClose={onRequestClose} />);

  fireEvent.click(screen.getByRole('tab', { name: /one\.md/i }));
  expect(onLocalIntent).toHaveBeenCalledTimes(1);
  expect(activeAtIntent).toEqual([twoId]);
  expect(useTabsStore.getState().activeTabId).toBe(oneId);

  fireEvent.click(screen.getByRole('button', { name: 'Close one.md' }));
  expect(onLocalIntent).toHaveBeenCalledTimes(2);
  expect(onRequestClose).toHaveBeenCalledOnce();
  expect(activeAtIntent).toEqual([twoId, oneId]);
  expect(calls.slice(-2)).toEqual(['local', 'close']);
});
