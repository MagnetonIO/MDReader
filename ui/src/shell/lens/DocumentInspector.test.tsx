import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';

import type { FederatedDocument } from '../../platform/semantics.js';
import { DocumentInspector } from './DocumentInspector.js';

const answer: FederatedDocument = {
  queryId: 'query-1',
  completeness: 'complete',
  schema: {
    name: 'doc',
    version: 3,
    checksum: 'blake3:abc',
    entities: ['Document'],
    arrowCount: 0,
    attributeCount: 6,
    equationCount: 0,
    approvalRequired: false,
    objectCount: 0,
    proposalCount: 0,
  },
  document: {
    path: 'notes/a.md',
    fields: [
      {
        name: 'title',
        value: '"A title"',
        winner: 'frontmatter',
        explanation: '`title` answered by `frontmatter.title`',
        providers: [
          {
            source: 'frontmatter',
            attribute: 'title',
            verdict: 'observed',
            value: '"A title"',
            reason: null,
            declaredFallback: ['YAML `title:`', 'first H1'],
          },
        ],
      },
      {
        name: 'modifiedAt',
        value: '1720000000000',
        winner: 'filesystem',
        explanation:
          '`modifiedAt` answered by `filesystem.modifiedAt`; `git.modifiedAt` also observed a value',
        providers: [
          {
            source: 'filesystem',
            attribute: 'modifiedAt',
            verdict: 'observed',
            value: '1720000000000',
            reason: null,
            declaredFallback: [],
          },
          {
            source: 'git',
            attribute: 'modifiedAt',
            verdict: 'observed',
            value: '1710000000000',
            reason: null,
            declaredFallback: [],
          },
        ],
      },
    ],
  },
  sources: [
    {
      source: 'filesystem',
      outcome: 'ran',
      execution: 'virtual',
      freshness: 'fresh',
      ageMs: 0,
      scanned: 1,
      returned: 1,
      pushed: ['path'],
      residual: [],
      refusal: null,
    },
    {
      source: 'git',
      outcome: 'ran',
      execution: 'materialized',
      freshness: 'fresh',
      ageMs: 0,
      scanned: 1,
      returned: 1,
      pushed: ['path'],
      residual: [],
      refusal: null,
    },
    {
      source: 'frontmatter',
      outcome: 'ran',
      execution: 'virtual',
      freshness: 'fresh',
      ageMs: 0,
      scanned: 1,
      returned: 1,
      pushed: ['path'],
      residual: [],
      refusal: null,
    },
  ],
};

afterEach(cleanup);

it('presents Result, Sources, Provenance, and Schema as one document answer', async () => {
  const query = vi.fn().mockResolvedValue(answer);
  render(<DocumentInspector path="/library/notes/a.md" query={query} workspaceKey="/library" />);

  expect(await screen.findByText('A title')).toBeTruthy();
  expect(screen.getByText('Complete')).toBeTruthy();
  expect(query).toHaveBeenCalledWith('/library/notes/a.md');

  fireEvent.click(screen.getByRole('tab', { name: 'Sources' }));
  expect(screen.getByText('filesystem')).toBeTruthy();
  expect(screen.getByText('git')).toBeTruthy();
  expect(screen.getByText('frontmatter')).toBeTruthy();
  expect(screen.getAllByText('1 scanned · 1 returned')).toHaveLength(3);

  fireEvent.click(screen.getByRole('tab', { name: 'Provenance' }));
  expect(screen.getByText('filesystem.modifiedAt')).toBeTruthy();
  expect(screen.getByText('git.modifiedAt')).toBeTruthy();
  expect(screen.getAllByText('selected')).toHaveLength(2);
  expect(screen.getByText('also observed')).toBeTruthy();

  fireEvent.click(screen.getByRole('tab', { name: 'Schema' }));
  expect(screen.getByText('doc · v3')).toBeTruthy();
  expect(screen.getByText('blake3:abc')).toBeTruthy();
  expect(
    screen.getByText(/changes the semantic interpretation, not the Markdown file/i),
  ).toBeTruthy();
});

it('drops a late answer after the active workspace changes', async () => {
  const pending: Array<(value: FederatedDocument) => void> = [];
  const query = vi.fn(
    () =>
      new Promise<FederatedDocument>((done) => {
        pending.push(done);
      }),
  );
  const rendered = render(
    <DocumentInspector path="/one/a.md" query={query} workspaceKey="/one" />,
  );
  rendered.rerender(
    <DocumentInspector path="/two/b.md" query={query} workspaceKey="/two" />,
  );

  pending[0]!(answer);
  await waitFor(() => expect(screen.queryByText('A title')).toBeNull());
});

it('retires and cancels a request when the same root advances to a new generation', async () => {
  const pending: Array<(value: FederatedDocument) => void> = [];
  const query = vi.fn(
    () =>
      new Promise<FederatedDocument>((done) => {
        pending.push(done);
      }),
  );
  const cancel = vi.fn(async () => {});
  const rendered = render(
    <DocumentInspector
      {...{
        path: '/same/a.md',
        query,
        cancel,
        workspaceKey: '/same',
        workspaceGeneration: 7,
      }}
    />,
  );

  rendered.rerender(
    <DocumentInspector
      {...{
        path: '/same/a.md',
        query,
        cancel,
        workspaceKey: '/same',
        workspaceGeneration: 8,
      }}
    />,
  );

  await waitFor(() => expect(query).toHaveBeenCalledTimes(2));
  await waitFor(() => expect(cancel).toHaveBeenCalled());
  pending[0]!(answer);
  await waitFor(() => expect(screen.queryByText('A title')).toBeNull());
});

it('implements the tab and tabpanel relationships plus arrow-key navigation', async () => {
  render(
    <DocumentInspector
      path="/library/notes/a.md"
      query={vi.fn().mockResolvedValue(answer)}
      workspaceKey="/library"
    />,
  );
  await screen.findByText('A title');

  const result = screen.getByRole('tab', { name: 'Result' });
  const sources = screen.getByRole('tab', { name: 'Sources' });
  const panel = screen.getByRole('tabpanel');
  expect(result.getAttribute('aria-controls')).toBe(panel.id);
  expect(panel.getAttribute('aria-labelledby')).toBe(result.id);
  expect(result.tabIndex).toBe(0);
  expect(sources.tabIndex).toBe(-1);

  result.focus();
  fireEvent.keyDown(result, { key: 'ArrowRight' });
  expect(document.activeElement).toBe(sources);
  expect(sources.getAttribute('aria-selected')).toBe('true');
  expect(screen.getByRole('tabpanel').getAttribute('aria-labelledby')).toBe(sources.id);
});
