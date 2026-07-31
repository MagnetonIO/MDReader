import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, test } from 'vitest';

import { FrontmatterCard } from './frontmatter-card.js';

afterEach(cleanup);

describe('FrontmatterCard', () => {
  test('starts collapsed and expands on click', () => {
    render(<FrontmatterCard frontmatter={{ title: 'Hi', author: 'M. Long' }} />);

    // Collapsed: values are not rendered yet.
    expect(screen.queryByText('M. Long')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /Frontmatter/ }));

    // Expanded: keys and values are shown.
    expect(screen.getByText('author')).toBeTruthy();
    expect(screen.getByText('M. Long')).toBeTruthy();
  });

  test('renders the malformed {raw, error} case with a warning and the raw block', () => {
    render(
      <FrontmatterCard
        frontmatter={{ raw: 'title: [unclosed', error: 'bad indentation' }}
        defaultCollapsed={false}
      />,
    );

    expect(screen.getByText(/could not be parsed/)).toBeTruthy();
    expect(screen.getByText('bad indentation')).toBeTruthy();
    expect(screen.getByText(/title: \[unclosed/)).toBeTruthy();
  });
});
