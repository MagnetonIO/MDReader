import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { vi } from 'vitest';

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

it('presents Result, Sources, Provenance, and Schema as one document answer', async () => {
  const query = vi.fn().mockResolvedValue(answer);
  render(<DocumentInspector path="/library/notes/a.md" query={query} workspaceKey="/library" />);

  expect(await screen.findByText('A title')).toBeVisible();
  expect(screen.getByText('Complete')).toBeVisible();
  expect(query).toHaveBeenCalledWith('/library/notes/a.md');

  fireEvent.click(screen.getByRole('tab', { name: 'Sources' }));
  expect(screen.getByText('filesystem')).toBeVisible();
  expect(screen.getByText('git')).toBeVisible();
  expect(screen.getByText('frontmatter')).toBeVisible();
  expect(screen.getAllByText('1 scanned · 1 returned')).toHaveLength(3);

  fireEvent.click(screen.getByRole('tab', { name: 'Provenance' }));
  expect(screen.getByText('filesystem.modifiedAt')).toBeVisible();
  expect(screen.getByText('git.modifiedAt')).toBeVisible();
  expect(screen.getByText('selected')).toBeVisible();
  expect(screen.getByText('also observed')).toBeVisible();

  fireEvent.click(screen.getByRole('tab', { name: 'Schema' }));
  expect(screen.getByText('doc · v3')).toBeVisible();
  expect(screen.getByText('blake3:abc')).toBeVisible();
  expect(
    screen.getByText(/changes the semantic interpretation, not the Markdown file/i),
  ).toBeVisible();
});

it('drops a late answer after the active workspace changes', async () => {
  let resolve!: (value: FederatedDocument) => void;
  const query = vi.fn(
    () =>
      new Promise<FederatedDocument>((done) => {
        resolve = done;
      }),
  );
  const rendered = render(
    <DocumentInspector path="/one/a.md" query={query} workspaceKey="/one" />,
  );
  rendered.rerender(
    <DocumentInspector path="/two/b.md" query={query} workspaceKey="/two" />,
  );

  resolve(answer);
  await waitFor(() => expect(screen.queryByText('A title')).not.toBeInTheDocument());
});
