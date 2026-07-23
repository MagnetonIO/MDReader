import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, test } from 'vitest';

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
