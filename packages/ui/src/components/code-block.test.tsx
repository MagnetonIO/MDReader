import { cleanup, render, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test } from 'vitest';

import { CodeBlock } from './code-block.js';

afterEach(cleanup);

describe('CodeBlock', () => {
  test('renders a plain <pre> immediately, before Shiki resolves', () => {
    const { container } = render(<CodeBlock code="const x = 1;" lang="js" />);
    // The synchronous first paint keeps the raw source visible (SSR-safe).
    expect(container.querySelector('pre')?.textContent).toContain('const x = 1;');
    expect(container.querySelector('.mdreader-code__lang')?.textContent).toBe('js');
  });

  test('upgrades to Shiki dual-theme output carrying both color lanes', async () => {
    const { container } = render(<CodeBlock code="const answer = 42;" lang="js" />);

    // Once the lazy highlighter resolves it swaps in the `.shiki` block whose
    // spans carry both `--shiki-light` and `--shiki-dark` custom properties —
    // the two lanes the ui stylesheet maps to the active theme.
    await waitFor(
      () => {
        expect(container.querySelector('.shiki')).not.toBeNull();
      },
      { timeout: 15000 },
    );

    const html = container.innerHTML;
    expect(html).toContain('--shiki-light');
    expect(html).toContain('--shiki-dark');
  }, 20000);

  test('preserves provenance stamps on the wrapper', () => {
    const { container } = render(
      <CodeBlock code="x" lang="rust" dataLineStart="4" dataLineEnd="6" />,
    );
    const wrapper = container.querySelector('.mdreader-code');
    expect(wrapper?.getAttribute('data-line-start')).toBe('4');
    expect(wrapper?.getAttribute('data-line-end')).toBe('6');
  });
});
