import { describe, expect, test } from 'vitest';

import {
  buildBreadcrumb,
  cycleIntent,
  filterRelated,
  intentPlan,
  parseSnippet,
  sortHits,
  type RelatedHit,
  type SearchHit,
} from './model.js';

function hit(partial: Partial<SearchHit> & { chunkId: string }): SearchHit {
  return {
    documentId: 'doc',
    path: `/ws/${partial.chunkId}.md`,
    filePath: `${partial.chunkId}.md`,
    name: `${partial.chunkId}.md`,
    headingPath: [],
    startLine: 1,
    endLine: 2,
    score: 0,
    rrfScore: 0,
    boosts: { title: 0, path: 0, recency: 0, graph: 0 },
    lexRank: null,
    semRank: null,
    snippet: '',
    mtime: 0,
    explain: null,
    ...partial,
  };
}

describe('intentPlan', () => {
  test('maps each intent to its engine plan', () => {
    expect(intentPlan('best')).toMatchObject({ mode: 'search', lexicalOnly: false, recencySort: false });
    expect(intentPlan('exact')).toMatchObject({ mode: 'search', lexicalOnly: true });
    expect(intentPlan('semantic')).toMatchObject({ mode: 'search', lexicalOnly: false });
    expect(intentPlan('recent')).toMatchObject({ mode: 'search', recencySort: true });
    expect(intentPlan('related')).toMatchObject({ mode: 'related' });
  });
});

describe('cycleIntent', () => {
  test('cycles forward and backward, wrapping', () => {
    expect(cycleIntent('best', 1)).toBe('exact');
    expect(cycleIntent('related', 1)).toBe('best'); // wraps
    expect(cycleIntent('best', -1)).toBe('related'); // wraps back
  });
});

describe('sortHits', () => {
  test('Recent sorts by mtime desc; other intents keep engine order', () => {
    const hits = [
      hit({ chunkId: 'a', mtime: 100, score: 0.9 }),
      hit({ chunkId: 'b', mtime: 300, score: 0.5 }),
      hit({ chunkId: 'c', mtime: 200, score: 0.7 }),
    ];
    expect(sortHits('best', hits).map((h) => h.chunkId)).toEqual(['a', 'b', 'c']);
    expect(sortHits('recent', hits).map((h) => h.chunkId)).toEqual(['b', 'c', 'a']);
  });

  test('never mutates the input array', () => {
    const hits = [hit({ chunkId: 'a', mtime: 1 }), hit({ chunkId: 'b', mtime: 2 })];
    const snapshot = hits.map((h) => h.chunkId);
    sortHits('recent', hits);
    expect(hits.map((h) => h.chunkId)).toEqual(snapshot);
  });
});

describe('parseSnippet', () => {
  test('splits FTS [..] markers into safe segments', () => {
    expect(parseSnippet('the [storage] engine')).toEqual([
      { text: 'the ', match: false },
      { text: 'storage', match: true },
      { text: ' engine', match: false },
    ]);
  });

  test('plain text with no markers is one non-match segment', () => {
    expect(parseSnippet('nothing here')).toEqual([{ text: 'nothing here', match: false }]);
  });

  test('an unbalanced bracket degrades to plain text (no injection)', () => {
    // A stray '[' never closes → its tail is plain text, never a match run.
    expect(parseSnippet('a [b')).toEqual([{ text: 'a ', match: false }, { text: 'b', match: false }]);
  });
});

describe('buildBreadcrumb', () => {
  test('assembles source › file › heading-path', () => {
    const crumbs = buildBreadcrumb(
      { filePath: 'notes/deep.md', name: 'deep.md', headingPath: ['Top', 'Sub'] },
      'my-vault',
    );
    expect(crumbs.map((c) => `${c.kind}:${c.label}`)).toEqual([
      'source:my-vault',
      'file:notes/deep.md',
      'heading:Top',
      'heading:Sub',
    ]);
  });

  test('omits the source crumb when there is no workspace name', () => {
    const crumbs = buildBreadcrumb({ filePath: 'a.md', name: 'a.md', headingPath: [] }, null);
    expect(crumbs.map((c) => c.kind)).toEqual(['file']);
  });
});

describe('filterRelated', () => {
  test('narrows a related list by name/path substring', () => {
    const related: RelatedHit[] = [
      { documentId: '1', path: '/ws/storage.md', filePath: 'storage.md', name: 'storage.md', score: 1, components: { graph: 1, concept: 0, vector: 0 } },
      { documentId: '2', path: '/ws/graph.md', filePath: 'graph.md', name: 'graph.md', score: 0.5, components: { graph: 0.5, concept: 0, vector: 0 } },
    ];
    expect(filterRelated(related, 'stor').map((r) => r.name)).toEqual(['storage.md']);
    expect(filterRelated(related, '')).toHaveLength(2);
  });
});
