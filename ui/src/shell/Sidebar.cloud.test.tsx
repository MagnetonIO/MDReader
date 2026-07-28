import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { readFileSync } from 'node:fs';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';

import type {
  LibraryEntityQueryResponse_Serialize,
  LibrarySourceExecution,
} from '../bindings.js';
import { Sidebar } from './Sidebar.js';

afterEach(cleanup);

function fixtureForMember({
  id,
  registryId,
  name,
  kind,
  editable,
  active,
  title,
  path,
}: {
  id: number;
  registryId: string;
  name: string;
  kind: LibrarySourceExecution['kind'];
  editable: boolean;
  active: boolean;
  title: string;
  path: string;
}): {
  member: LibrarySourceExecution;
  response: LibraryEntityQueryResponse_Serialize;
} {
  const uuid = (offset: number) =>
    `00000000-0000-4000-8000-${String(id + offset).padStart(12, '0')}`;
  const member: LibrarySourceExecution = {
    membership_id: uuid(0),
    registry_id: registryId,
    name,
    kind,
    source_ids: [uuid(1)],
    editable,
    active,
  };
  const sourceVersionId = uuid(2);
  const mappingVersionId = uuid(3);
  const provenanceRuleVersionId = uuid(4);
  const recordKey = `library:${member.membership_id}:${path}`;
  const row = {
    record_key: recordKey,
    fields: { title, path },
    provenance: {
      path: {
        field: 'path',
        provenance_rule_version_id: provenanceRuleVersionId,
        selected: 0,
        considered: [
          {
            source_version_id: sourceVersionId,
            record_key: path,
            mapping_version_id: mappingVersionId,
            physical_attribute: 'path',
            observed_at_ms: 1,
            state: { state: 'observed_value' as const, value: path },
          },
        ],
      },
    },
  };
  const response: LibraryEntityQueryResponse_Serialize = {
    memberships: [member],
    answer: {
      query_id: uuid(5),
      pins: {
        catalog_resolver_id: uuid(6),
        catalog_revision_id: uuid(7),
        schema_version_id: uuid(8),
        physical_schema_version_ids: [uuid(9)],
        mapping_version_ids: [mappingVersionId],
        source_version_ids: [sourceVersionId],
        policy_version_ids: [uuid(10)],
        provenance_rule_version_id: provenanceRuleVersionId,
      },
      completeness: { state: 'complete' },
      sources: [
        {
          source_id: member.source_ids[0]!,
          source_version_id: sourceVersionId,
          freshness_ms: 0,
          outcome: 'ran',
          retry_after_ms: null,
          scanned: 1,
          returned: 1,
        },
      ],
      rows: [row],
      next_cursor: null,
    },
  };

  return { member, response };
}

it('keeps Library Refresh a compact quiet action without changing its behavior', () => {
  const onRefreshLibrary = vi.fn();
  const commonProps = {
    collapsed: false,
    root: '/Library/Active notes',
    files: [],
    recent: [],
    activePath: null,
    loading: false,
    onPickFolder: vi.fn(),
    onOpenFile: vi.fn(),
    onOpenRecent: vi.fn(),
    librarySearch: '',
    onLibrarySearch: vi.fn(),
    onRefreshLibrary,
  };
  const view = render(<Sidebar {...commonProps} />);

  const refresh = screen.getByRole('button', { name: 'Refresh' });
  expect(refresh.classList.contains('library-sources__refresh')).toBe(true);
  fireEvent.click(refresh);
  expect(onRefreshLibrary).toHaveBeenCalledOnce();

  const shellCss = readFileSync(`${process.cwd()}/src/shell/shell.css`, 'utf8');
  const compactRule = shellCss.match(
    /\.library-sources__heading\s+\.library-sources__refresh\s*\{([^}]*)\}/,
  )?.[1];
  expect(compactRule, 'the scoped compact Refresh rule').toBeTruthy();
  expect(compactRule).toContain('padding: 2px var(--space-1);');
  expect(compactRule).toContain('font-size: 0.68rem;');
  expect(compactRule).toContain('line-height: 1.2;');

  view.rerender(<Sidebar {...commonProps} libraryLoading />);
  const reading = screen.getByRole('button', { name: 'Reading…' });
  expect(reading.classList.contains('library-sources__refresh')).toBe(true);
  expect((reading as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(reading);
  expect(onRefreshLibrary).toHaveBeenCalledOnce();
});

it('navigates connected read-through sources from the primary Library rail', () => {
  const dropbox = fixtureForMember({
    id: 20,
    registryId: 'dropbox',
    name: 'Dropbox',
    kind: 'dropbox',
    editable: false,
    active: false,
    title: 'Cloud note.md',
    path: '/notes/cloud.md',
  });
  const open = vi.fn();

  render(
    <Sidebar
      collapsed={false}
      root={null}
      files={[]}
      recent={[]}
      activePath={null}
      loading={false}
      onPickFolder={vi.fn()}
      onOpenFile={vi.fn()}
      onOpenRecent={vi.fn()}
      libraryEvidence={[dropbox.response]}
      librarySearch=""
      onLibrarySearch={vi.fn()}
      onOpenLibrary={open}
      onRefreshLibrary={vi.fn()}
    />,
  );

  expect(screen.getByRole('navigation', { name: 'Library sources' })).toBeTruthy();
  expect(screen.getByText('Read-only')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Open Cloud note.md from Dropbox' }));
  expect(open).toHaveBeenCalledWith(dropbox.member, dropbox.response.answer.rows[0]);
});

it('keeps the active workspace and every supplemental membership across evidence groups', () => {
  const activeLocal = fixtureForMember({
    id: 100,
    registryId: 'active-local-source',
    name: 'Active notes',
    kind: 'local-folder',
    editable: true,
    active: true,
    title: 'workspace.md',
    path: 'notes/workspace.md',
  });
  const inactiveLocal = fixtureForMember({
    id: 120,
    registryId: 'inactive-local-source',
    name: 'Archived notes',
    kind: 'local-folder',
    editable: true,
    active: false,
    title: 'Archived.md',
    path: 'archive/archived.md',
  });
  const dropbox = fixtureForMember({
    id: 140,
    registryId: 'dropbox',
    name: 'Dropbox',
    kind: 'dropbox',
    editable: false,
    active: false,
    title: 'Cloud note.md',
    path: '/notes/cloud.md',
  });
  const connectedCatdb = fixtureForMember({
    id: 160,
    registryId: 'connected-catdb',
    name: 'Research CatDB',
    kind: 'connected-catdb',
    editable: false,
    active: true,
    title: 'Shared research.md',
    path: 'research/shared.md',
  });
  const evidenceGroups = Object.freeze([
    activeLocal.response,
    inactiveLocal.response,
    dropbox.response,
    connectedCatdb.response,
  ]);
  const open = vi.fn();

  render(
    <Sidebar
      collapsed={false}
      root="/Library/Active notes"
      files={[
        {
          path: '/Library/Active notes/notes/workspace.md',
          name: 'workspace.md',
          dir: 'notes',
          mtime: 1,
          size: 12,
          ignored: false,
        },
      ]}
      recent={[]}
      activePath={null}
      loading={false}
      onPickFolder={vi.fn()}
      onOpenFile={vi.fn()}
      onOpenRecent={vi.fn()}
      libraryEvidence={[...evidenceGroups]}
      librarySearch=""
      onLibrarySearch={vi.fn()}
      onOpenLibrary={open}
      onRefreshLibrary={vi.fn()}
    />,
  );

  expect(screen.getByRole('tree', { name: 'Workspace files' })).toBeTruthy();

  const rail = screen.getByRole('navigation', { name: 'Library sources' });
  expect(within(rail).queryByText(activeLocal.member.name)).toBeNull();
  expect(within(rail).getByText(inactiveLocal.member.name)).toBeTruthy();
  expect(within(rail).getByText(dropbox.member.name)).toBeTruthy();
  expect(within(rail).getByText(connectedCatdb.member.name)).toBeTruthy();

  fireEvent.click(
    within(rail).getByRole('button', { name: 'Open Archived.md from Archived notes' }),
  );
  fireEvent.click(
    within(rail).getByRole('button', { name: 'Open Cloud note.md from Dropbox' }),
  );
  fireEvent.click(
    within(rail).getByRole('button', {
      name: 'Open Shared research.md from Research CatDB',
    }),
  );

  expect(open).toHaveBeenNthCalledWith(
    1,
    inactiveLocal.member,
    inactiveLocal.response.answer.rows[0],
  );
  expect(open).toHaveBeenNthCalledWith(2, dropbox.member, dropbox.response.answer.rows[0]);
  expect(open).toHaveBeenNthCalledWith(
    3,
    connectedCatdb.member,
    connectedCatdb.response.answer.rows[0],
  );
  expect(open).toHaveBeenCalledTimes(3);
});

it('keeps the Library document filter available for an active-local-only Library', () => {
  const activeLocal = fixtureForMember({
    id: 180,
    registryId: 'active-local-source',
    name: 'Active notes',
    kind: 'local-folder',
    editable: true,
    active: true,
    title: 'workspace.md',
    path: 'notes/workspace.md',
  });

  render(
    <Sidebar
      collapsed={false}
      root="/Library/Active notes"
      files={[
        {
          path: '/Library/Active notes/notes/workspace.md',
          name: 'workspace.md',
          dir: 'notes',
          mtime: 1,
          size: 12,
          ignored: false,
        },
      ]}
      recent={[]}
      activePath={null}
      loading={false}
      onPickFolder={vi.fn()}
      onOpenFile={vi.fn()}
      onOpenRecent={vi.fn()}
      libraryEvidence={[activeLocal.response]}
      librarySearch=""
      onLibrarySearch={vi.fn()}
      onOpenLibrary={vi.fn()}
      onRefreshLibrary={vi.fn()}
    />,
  );

  const rail = screen.getByRole('navigation', { name: 'Library sources' });
  expect(
    within(rail).getByRole('searchbox', { name: 'Filter Library documents' }),
  ).toBeTruthy();
  expect(within(rail).queryByText(activeLocal.member.name)).toBeNull();
});

it('routes one Library document filter to the active workspace tree and supplemental rows', () => {
  const activeLocal = fixtureForMember({
    id: 200,
    registryId: 'active-local-source',
    name: 'Active notes',
    kind: 'local-folder',
    editable: true,
    active: true,
    title: 'workspace.md',
    path: 'notes/workspace.md',
  });
  const dropbox = fixtureForMember({
    id: 220,
    registryId: 'dropbox',
    name: 'Dropbox',
    kind: 'dropbox',
    editable: false,
    active: false,
    title: 'Cloud note.md',
    path: '/notes/cloud.md',
  });

  function Harness() {
    const [search, setSearch] = useState('');
    return (
      <Sidebar
        collapsed={false}
        root="/Library/Active notes"
        files={[
          {
            path: '/Library/Active notes/notes/workspace.md',
            name: 'workspace.md',
            dir: 'notes',
            mtime: 1,
            size: 12,
            ignored: false,
          },
        ]}
        recent={[]}
        activePath={null}
        loading={false}
        onPickFolder={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenRecent={vi.fn()}
        libraryEvidence={[activeLocal.response, dropbox.response]}
        librarySearch={search}
        onLibrarySearch={setSearch}
        onOpenLibrary={vi.fn()}
        onRefreshLibrary={vi.fn()}
      />
    );
  }

  render(<Harness />);

  const filter = screen.getByRole('searchbox', { name: 'Filter Library documents' });
  const tree = screen.getByRole('tree', { name: 'Workspace files' });
  const rail = screen.getByRole('navigation', { name: 'Library sources' });

  fireEvent.change(filter, { target: { value: 'cloud' } });
  expect(within(tree).getByText('No Library documents match.')).toBeTruthy();
  expect(
    within(rail).getByRole('button', { name: 'Open Cloud note.md from Dropbox' }),
  ).toBeTruthy();

  fireEvent.change(filter, { target: { value: 'workspace' } });
  expect(within(tree).queryByText('No Library documents match.')).toBeNull();
  expect(within(rail).getByText('No Library documents match.')).toBeTruthy();
});

it('keeps a workspace listing error visible after Library evidence arrives', () => {
  const dropbox = fixtureForMember({
    id: 180,
    registryId: 'dropbox',
    name: 'Dropbox',
    kind: 'dropbox',
    editable: false,
    active: false,
    title: 'Cloud note.md',
    path: '/notes/cloud.md',
  });

  render(
    <Sidebar
      collapsed={false}
      root="/Library/Active notes"
      files={[]}
      recent={[]}
      activePath={null}
      loading={false}
      listingError={{
        kind: 'unreadable',
        path: '/Library/Active notes',
        reason: 'Permission denied',
        remedy: 'Allow access to this folder in System Settings.',
      }}
      onPickFolder={vi.fn()}
      onOpenFile={vi.fn()}
      onOpenRecent={vi.fn()}
      libraryEvidence={[dropbox.response]}
      librarySearch=""
      onLibrarySearch={vi.fn()}
      onOpenLibrary={vi.fn()}
      onRefreshLibrary={vi.fn()}
    />,
  );

  const listingError = screen.getByRole('alert', { name: 'Workspace files' });
  expect(within(listingError).getByText('This folder could not be read.')).toBeTruthy();
  expect(within(listingError).getByText('Permission denied')).toBeTruthy();
  expect(
    within(listingError).getByText('Allow access to this folder in System Settings.'),
  ).toBeTruthy();
  expect(screen.getByRole('navigation', { name: 'Library sources' })).toBeTruthy();
});
