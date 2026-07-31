import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import type { OutlineItem } from '@mdreader/markdown';

import { Outline } from './outline.js';

afterEach(cleanup);

const ITEMS: OutlineItem[] = [
  { depth: 1, text: 'Intro', slug: 'intro', lineStart: 1 },
  { depth: 2, text: 'Details', slug: 'details', lineStart: 5 },
];

describe('Outline', () => {
  test('lists headings and marks the active slug', () => {
    render(<Outline items={ITEMS} activeSlug="details" />);
    expect(screen.getByRole('link', { name: 'Intro' }).getAttribute('href')).toBe('#intro');
    expect(screen.getByRole('link', { name: 'Details' }).getAttribute('aria-current')).toBe(
      'location',
    );
  });

  test('reports selections', () => {
    const onSelect = vi.fn();
    render(<Outline items={ITEMS} onSelect={onSelect} />);
    fireEvent.click(screen.getByRole('link', { name: 'Intro' }));
    expect(onSelect).toHaveBeenCalledWith(ITEMS[0]);
  });

  test('renders nothing when there are no items', () => {
    const { container } = render(<Outline items={[]} />);
    expect(container.querySelector('nav')).toBeNull();
  });
});
