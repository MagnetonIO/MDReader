import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';

import { MermaidBlock } from './mermaid-block.js';

const { initializeMock, renderMock } = vi.hoisted(() => ({
  initializeMock: vi.fn(),
  renderMock: vi.fn(),
}));

vi.mock('mermaid', () => ({
  default: { initialize: initializeMock, render: renderMock },
}));

afterEach(() => {
  cleanup();
  document.documentElement.removeAttribute('data-theme');
});

beforeEach(() => {
  initializeMock.mockReset();
  renderMock.mockReset();
});

describe('MermaidBlock', () => {
  test('falls back to the diagram source when rendering fails', async () => {
    renderMock.mockRejectedValue(new Error('mermaid boom'));
    // jsdom has no IntersectionObserver, so the block renders immediately.
    const source = 'graph TD; A-->B; B-->C';
    const { container } = render(<MermaidBlock source={source} />);

    // On failure the diagram container is dropped and the bare source remains.
    await waitFor(() => {
      expect(container.querySelector('.mdreader-mermaid')).toBeNull();
    });
    expect(container.querySelector('.mdreader-mermaid__source')).not.toBeNull();
    expect(screen.getByText(/graph TD; A-->B/)).toBeTruthy();
  });

  test('re-renders with the new theme when [data-theme] changes on <html>', async () => {
    let counter = 0;
    renderMock.mockImplementation(async () => ({ svg: `<svg data-n="${(counter += 1)}"></svg>` }));

    document.documentElement.setAttribute('data-theme', 'light');
    render(<MermaidBlock source="graph TD; A-->B" />);

    await waitFor(() => expect(renderMock).toHaveBeenCalledTimes(1));
    expect(initializeMock).toHaveBeenLastCalledWith(
      expect.objectContaining({ theme: 'default' }),
    );

    // A manual theme toggle stamps [data-theme] onto <html>; the MutationObserver
    // must pick it up and re-render the diagram with the dark theme.
    act(() => {
      document.documentElement.setAttribute('data-theme', 'dark');
    });

    await waitFor(() => expect(renderMock).toHaveBeenCalledTimes(2));
    expect(initializeMock).toHaveBeenLastCalledWith(expect.objectContaining({ theme: 'dark' }));
  });
});
