import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';

import { StatusBar } from './StatusBar.js';
import type { ImportState } from '../platform/fs.js';

afterEach(cleanup);

function importing(filesTotal: number): ImportState {
  return { phase: 'importing', filesDone: 0, filesTotal, chunks: 0, error: null };
}

describe('StatusBar index indicator', () => {
  test('while importing it shows a spinner and the file total', () => {
    const { container } = render(
      <StatusBar
        words={null}
        watcher="watching"
        fileCount={12}
        fromIndex={false}
        importState={importing(12)}
      />,
    );
    expect(screen.getByText(/Indexing 12 files/)).toBeTruthy();
    expect(container.querySelector('.statusbar__spinner')).not.toBeNull();
  });

  test('a model change is reported as a re-embed, not a generic index pass', () => {
    const { container } = render(
      <StatusBar
        words={null}
        watcher="watching"
        fileCount={1200}
        fromIndex={true}
        importState={{
          phase: 'reembedding',
          filesDone: 0,
          filesTotal: 1200,
          chunks: 0,
          error: null,
        }}
      />,
    );
    const label = screen.getByText(/Re-embedding 1,200 files/);
    expect(label).toBeTruthy();
    // And it says WHY, so a long pass is not mistaken for a stuck one.
    expect(label.closest('span')?.getAttribute('title')).toMatch(/embedding model changed/);
    expect(container.querySelector('.statusbar__spinner')).not.toBeNull();
    // It is not a failure: no Retry affordance.
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull();
  });

  test('while loading the model it shows a spinner and a loading label', () => {
    const { container } = render(
      <StatusBar
        words={null}
        watcher="watching"
        fileCount={0}
        fromIndex={false}
        importState={{ phase: 'loading', filesDone: 0, filesTotal: 0, chunks: 0, error: null }}
      />,
    );
    expect(screen.getByText(/Loading model/)).toBeTruthy();
    expect(container.querySelector('.statusbar__spinner')).not.toBeNull();
  });

  test('on done it reports the indexed file count (no spinner)', () => {
    const { container } = render(
      <StatusBar
        words={200}
        watcher="watching"
        fileCount={12}
        fromIndex={true}
        importState={{ phase: 'done', filesDone: 12, filesTotal: 12, chunks: 34, error: null }}
      />,
    );
    expect(screen.getByText('Indexed 12 files')).toBeTruthy();
    expect(container.querySelector('.statusbar__spinner')).toBeNull();
  });

  test('on error it reports the failure', () => {
    render(
      <StatusBar
        words={null}
        watcher="idle"
        fileCount={0}
        fromIndex={false}
        importState={{ phase: 'error', filesDone: 0, filesTotal: 3, chunks: 0, error: 'boom' }}
      />,
    );
    expect(screen.getByText('Index failed')).toBeTruthy();
  });

  test('on error it offers a Retry that re-runs the import', () => {
    const onRetryIndex = vi.fn();
    render(
      <StatusBar
        words={null}
        watcher="idle"
        fileCount={0}
        fromIndex={false}
        importState={{
          phase: 'error',
          filesDone: 0,
          filesTotal: 0,
          chunks: 0,
          error: 'model download timed out',
        }}
        onRetryIndex={onRetryIndex}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(onRetryIndex).toHaveBeenCalledTimes(1);
  });

  test('no Retry button appears when the index is not in the error phase', () => {
    render(
      <StatusBar
        words={null}
        watcher="watching"
        fileCount={3}
        fromIndex={true}
        importState={{ phase: 'done', filesDone: 3, filesTotal: 3, chunks: 6, error: null }}
        onRetryIndex={() => {}}
      />,
    );
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull();
  });

  /**
   * The reported bug's whole point: a model that could not be loaded is a real,
   * nameable condition, and the status bar must name it — not fall back to a
   * label that reads as "nothing is wrong" (Indexed / Local files) and not blame
   * the index build (Index failed).
   *
   * Revert-RED: delete the `model-unavailable` branch from `indexIndicator` and
   * the phase falls through to the idle label — `getByText('Embedding model
   * unavailable')` fails, and the "does not read as" assertions below catch the
   * `Local files` it lands on instead.
   */
  test('a model that could not load is named, with its reason, distinctly', () => {
    render(
      <StatusBar
        words={null}
        watcher="idle"
        fileCount={49}
        fromIndex={false}
        importState={{
          phase: 'model-unavailable',
          filesDone: 0,
          filesTotal: 0,
          chunks: 0,
          error: 'the embedding model could not be loaded — timed out after 120s',
        }}
      />,
    );
    const label = screen.getByText('Embedding model unavailable');
    expect(label).toBeTruthy();

    // It carries the reason the backend actually observed — verbatim.
    const title = label.closest('span')?.getAttribute('title') ?? '';
    expect(title).toContain('the embedding model could not be loaded');
    expect(title).toContain('timed out after 120s');
    // ...and says what still works, so "unavailable" is not read as "broken".
    expect(title).toMatch(/read and text-search/);

    // It is not any of the labels that would misdescribe it.
    expect(screen.queryByText('Index failed')).toBeNull();
    expect(screen.queryByText('Local files')).toBeNull();
    expect(screen.queryByText('Indexed')).toBeNull();
    // The workspace is still open: the file count keeps reporting its files.
    expect(screen.getByText('49 files')).toBeTruthy();
  });

  test('the model-unavailable retry is labelled as a model retry, not an index one', () => {
    const onRetryIndex = vi.fn();
    render(
      <StatusBar
        words={null}
        watcher="idle"
        fileCount={49}
        fromIndex={false}
        importState={{
          phase: 'model-unavailable',
          filesDone: 0,
          filesTotal: 0,
          chunks: 0,
          error: 'the embedding model could not be loaded — timed out after 120s',
        }}
        onRetryIndex={onRetryIndex}
      />,
    );
    // Distinct from the index-error "Retry": what needs re-attempting is the load.
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Retry model load' }));
    expect(onRetryIndex).toHaveBeenCalledTimes(1);
  });

  test('idle falls back to the index/local indicator', () => {
    const { rerender } = render(
      <StatusBar words={null} watcher="idle" fileCount={0} fromIndex={false} importState={null} />,
    );
    expect(screen.getByText('Local files')).toBeTruthy();

    rerender(
      <StatusBar words={null} watcher="idle" fileCount={5} fromIndex={true} importState={null} />,
    );
    expect(screen.getByText('Indexed')).toBeTruthy();
  });
});
