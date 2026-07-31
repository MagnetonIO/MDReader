import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, test } from 'vitest';

import { ReaderPlaceholder } from './reader-placeholder.js';

afterEach(cleanup);

test('ReaderPlaceholder renders its placeholder text', () => {
  render(<ReaderPlaceholder />);
  expect(screen.getByText('Reader placeholder')).toBeTruthy();
});
