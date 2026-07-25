/**
 * The honest-rendering rules, asserted directly.
 *
 * Every claim these surfaces make about the engine's state is composed by a
 * function in `lens-format`, so the rules are testable as rules rather than
 * inferred from a rendered panel. The ones that matter most are the absences:
 * an unassigned version, a boost that never fired, a semantic arm that never
 * ran. Each has a *specific* reading, and none of them is `0`.
 */

import { describe, expect, test } from 'vitest';

import type { InspectedFact } from '../../platform/semantics.js';
import {
  UNVERSIONED_REASON,
  armsSentence,
  boostLabel,
  checksumLabel,
  citeSpan,
  claimFields,
  claimReading,
  confidenceLabel,
  factSchemaLabel,
  factStanding,
  indexLabel,
  lawSentence,
  localName,
  pathChain,
  pathPhrase,
  plural,
  projectionSummary,
  registrationSummary,
  rankLabel,
  reReviewHashes,
  reReviewPrompt,
  scoreLabel,
  shortHash,
  summarizeValue,
  versionLabel,
  violationSentence,
} from './lens-format.js';

function fact(overrides: Partial<InspectedFact> = {}): InspectedFact {
  return {
    factId: 'f1',
    kind: 'object',
    schema: 'decision_overlay',
    name: 'decision_overlay.Decision',
    value: '{"title":"Adopt SQLite"}',
    sourceId: null,
    targetId: null,
    rule: 'D1',
    extractor: 'mdreader.decision@1',
    provenance: {
      file: 'notes/adr-3.md',
      startLine: 12,
      endLine: 18,
      contentHash: 'blake3:9f2c1a4e88b7d3c2',
      schemaVersion: '4',
      extractor: 'mdreader.decision@1',
      confidence: 0.82,
      approvedBy: null,
    },
    state: 'proposed',
    reviewer: null,
    inGraph: false,
    reviewedContentHash: null,
    sourceChangedSinceReview: false,
    ...overrides,
  };
}

describe('a version the engine has not assigned', () => {
  test('reads as "not yet versioned" — never v0, never blank', () => {
    expect(versionLabel(null)).toBe('not yet versioned');
    // The three ways this could go wrong, ruled out explicitly.
    expect(versionLabel(null)).not.toBe('v0');
    expect(versionLabel(null)).not.toBe('');
    expect(versionLabel(null)).not.toContain('0');
  });

  test('a real version reads as itself, including version 1', () => {
    expect(versionLabel(1)).toBe('v1');
    expect(versionLabel(4)).toBe('v4');
  });

  test('an absent checksum says so rather than showing an empty digest', () => {
    expect(checksumLabel(null)).toBe('no checksum yet');
    expect(checksumLabel('blake3:9f2c1a4e88b7d3c2ff01')).toBe('blake3:9f2c1a4e…ff01');
  });

  test('the reason a version can be missing is stated, not left to guesswork', () => {
    expect(UNVERSIONED_REASON).toMatch(/registration/i);
    expect(UNVERSIONED_REASON).toMatch(/index is built/i);
  });
});

describe('a claim standing', () => {
  test('proposed AND in the graph shows BOTH truths, with the reason', () => {
    // No overlay GC: an earlier approval committed the claim, then the source
    // changed and the approval was reset. Both readings are true at once.
    const standing = factStanding(fact({ state: 'proposed', inGraph: true }));
    expect(standing.state).toBe('proposed');
    expect(standing.inGraph).toBe(true);
    expect(standing.divergent).toBe(true);
    expect(standing.note).toMatch(/earlier approval/i);
    expect(standing.note).toMatch(/still in the graph/i);
  });

  test('approved and in the graph is not flagged — nothing to explain', () => {
    const standing = factStanding(fact({ state: 'approved', inGraph: true }));
    expect(standing.divergent).toBe(false);
    expect(standing.note).toBeNull();
  });

  test('proposed and NOT in the graph is the ordinary case, not a divergence', () => {
    const standing = factStanding(fact({ state: 'proposed', inGraph: false }));
    expect(standing.divergent).toBe(false);
    expect(standing.note).toBeNull();
  });

  test('approved but absent from the graph explains the waiting endpoint', () => {
    const standing = factStanding(fact({ state: 'approved', inGraph: false }));
    expect(standing.divergent).toBe(true);
    expect(standing.note).toMatch(/both of the objects/i);
  });

  test('an unrecognised state is surfaced as unrecognised, not coerced', () => {
    expect(factStanding(fact({ state: 'quarantined' })).state).toBe('unknown');
  });
});

describe('a review whose source has moved', () => {
  test('is an explicit invitation to review again', () => {
    const prompt = reReviewPrompt(fact({ state: 'rejected', sourceChangedSinceReview: true }));
    expect(prompt).toBe('You rejected this. The cited text has changed since — review it again?');
  });

  test('is silent when the source has not changed', () => {
    expect(reReviewPrompt(fact({ state: 'rejected', sourceChangedSinceReview: false }))).toBeNull();
  });

  test('shows BOTH digests, so "changed" is checkable rather than asserted', () => {
    const hashes = reReviewHashes(
      fact({ reviewedContentHash: 'blake3:0000111122223333aaaa' }),
      fact().provenance,
    );
    expect(hashes.reviewed).toBe('reviewed blake3:00001111…aaaa');
    expect(hashes.current).toBe('now blake3:9f2c1a4e…d3c2');
    // Two real digests: the reader compares them, and nothing needs explaining.
    expect(hashes.uncomparable).toBeNull();
  });

  test('a missing CURRENT hash says so — it is NOT a comparison against one hash', () => {
    const hashes = reReviewHashes(fact({ reviewedContentHash: 'blake3:0000111122223333aaaa' }), {
      file: 'notes/adr-3.md',
      contentHash: null,
    });
    // The reviewed digest still stands, and the other side is named as absent
    // rather than dropped — "these differ" is never implied by one hash alone.
    expect(hashes.reviewed).toBe('reviewed blake3:00001111…aaaa');
    expect(hashes.current).toBe('current hash unavailable');
    expect(hashes.uncomparable).toBe(
      'The two cannot be compared here — no digest was recorded for the cited text as it now stands.',
    );
  });

  test('a claim citing no source text says THAT, rather than blaming the digest', () => {
    const hashes = reReviewHashes(fact({ reviewedContentHash: 'blake3:0000111122223333aaaa' }), {
      file: null,
      contentHash: null,
    });
    expect(hashes.uncomparable).toBe(
      'The two cannot be compared here — this claim cites no source text, so there is nothing to take a digest of.',
    );
  });

  test('a missing REVIEWED hash is named too, not silently dropped from the pair', () => {
    const hashes = reReviewHashes(fact({ reviewedContentHash: null }), fact().provenance);
    expect(hashes.reviewed).toBe('reviewed hash not recorded');
    expect(hashes.current).toBe('now blake3:9f2c1a4e…d3c2');
    expect(hashes.uncomparable).toBe(
      'The two cannot be compared here — no digest was recorded when this was reviewed.',
    );
  });

  test('neither digest recorded reads as neither — not as a match', () => {
    const hashes = reReviewHashes(fact({ reviewedContentHash: null }), {
      file: 'notes/adr-3.md',
      contentHash: null,
    });
    expect(hashes.reviewed).toBe('reviewed hash not recorded');
    expect(hashes.current).toBe('current hash unavailable');
    expect(hashes.uncomparable).toBe('The two cannot be compared here — neither digest was recorded.');
  });
});

describe('citations', () => {
  test('cite a line range, a single line, a bare file, and nothing at all', () => {
    expect(citeSpan({ file: 'notes/adr-3.md', startLine: 12, endLine: 18 })).toBe('notes/adr-3.md:12–18');
    expect(citeSpan({ file: 'notes/adr-3.md', startLine: 12, endLine: 12 })).toBe('notes/adr-3.md:12');
    expect(citeSpan({ file: 'notes/adr-3.md', startLine: 12, endLine: null })).toBe('notes/adr-3.md:12');
    expect(citeSpan({ file: 'notes/adr-3.md', startLine: null, endLine: null })).toBe('notes/adr-3.md');
    expect(citeSpan({ file: null, startLine: 12, endLine: 18 })).toBe('no source recorded');
  });

  test('an unrecorded confidence is not a confidence of zero', () => {
    expect(confidenceLabel(null)).toBe('no confidence recorded');
    expect(confidenceLabel(0)).toBe('0% confident');
    expect(confidenceLabel(0.82)).toBe('82% confident');
  });

  test('a fact stamped with no schema version says unversioned', () => {
    expect(factSchemaLabel('decision_overlay', null)).toBe('decision_overlay (unversioned)');
    expect(factSchemaLabel('decision_overlay', '4')).toBe('decision_overlay@4');
  });

  test('a short hash keeps its algorithm prefix', () => {
    expect(shortHash('blake3:9f2c1a4e88b7d3c2ff01')).toMatch(/^blake3:/);
    expect(shortHash('blake3:abc')).toBe('blake3:abc');
  });
});

describe('a claim read as a sentence', () => {
  test('an object claim names its entity and its most name-like field', () => {
    const reading = claimReading(fact());
    expect(reading.kind).toBe('object');
    expect(reading.sentence).toBe('Decision — Adopt SQLite');
  });

  test('a morphism claim reads as a relation between two endpoints', () => {
    const reading = claimReading(
      fact({
        kind: 'morphism',
        name: 'architecture_overlay.exposes',
        value: null,
        sourceId: 'aaaaaaaa-1111-2222-3333-444444444444',
        targetId: 'bbbbbbbb-1111-2222-3333-444444444444',
      }),
    );
    expect(reading.kind).toBe('morphism');
    expect(reading.sentence).toBe('aaaaaaaa… exposes bbbbbbbb…');
  });

  test('an unparseable payload is shown verbatim rather than dropped', () => {
    expect(summarizeValue('not json')).toBe('not json');
    expect(summarizeValue(null)).toBeNull();
  });

  test('a payload with no name-like field lists its fields', () => {
    expect(claimFields('{"status":"accepted","date":"2026-01-02"}')).toEqual([
      { key: 'status', value: 'accepted' },
      { key: 'date', value: '2026-01-02' },
    ]);
  });
});

describe('laws stated for a human', () => {
  test('a two-arrow path reads as a sequence of steps', () => {
    expect(pathPhrase(['via', 'exposes'])).toBe('following via, then exposes');
    expect(pathPhrase(['provider'])).toBe('following provider');
    expect(pathPhrase([])).toBe('staying where you are');
  });

  test('an equation reads as a checkable sentence', () => {
    expect(lawSentence({ start: 'Dependency', lhs: ['via', 'exposes'], rhs: ['provider'] })).toBe(
      'Starting from any Dependency, following via, then exposes must land on the same object as following provider.',
    );
  });

  test('the symbolic chain is kept alongside the words', () => {
    expect(pathChain('Dependency', ['via', 'exposes'])).toBe('Dependency → via → exposes');
  });

  test('a violation says what actually happened, including a missing arrow', () => {
    expect(violationSentence({ lhsPath: ['via'], rhsPath: ['provider'], lhs: {}, rhs: {} })).toMatch(
      /different objects/,
    );
    expect(
      violationSentence({ lhsPath: ['via', 'exposes'], rhsPath: ['provider'], lhs: null, rhs: {} }),
    ).toMatch(/arrow on that side is missing/);
  });
});

describe('the ranking, read back', () => {
  test('an arm that did not match says so instead of showing a rank', () => {
    expect(rankLabel(null, 'semantic')).toBe('semantic did not match');
    expect(rankLabel(3, 'lexical')).toBe('lexical #3');
  });

  test('a boost that never fired is not a contribution of zero', () => {
    expect(boostLabel(null)).toBe('did not fire');
    expect(boostLabel(0)).toBe('0.0000');
  });

  test('an unrecorded score is not a score of zero', () => {
    expect(scoreLabel(null)).toBe('not recorded');
    expect(scoreLabel(0)).toBe('0.0000');
  });

  test('which arms produced the hit is stated plainly', () => {
    expect(armsSentence({ lexRank: 1, semRank: 2 })).toMatch(/Both arms/);
    expect(armsSentence({ lexRank: 1, semRank: null })).toMatch(/Only the lexical/);
    expect(armsSentence({ lexRank: null, semRank: 2 })).toMatch(/Only the semantic/);
    expect(armsSentence({ lexRank: null, semRank: null })).toMatch(/Neither arm/);
  });

  test('a semantic arm that never ran says so rather than naming a phantom index', () => {
    expect(indexLabel({ indexVersion: null, embeddingModel: 'bge-small' })).toBe(
      'the semantic arm did not run',
    );
    expect(indexLabel({ indexVersion: 'v7', embeddingModel: 'bge-small' })).toBe('v7 · bge-small');
    expect(indexLabel({ indexVersion: 'v7', embeddingModel: null })).toBe('v7');
  });
});

describe('projections', () => {
  test('a truncated projection says it is showing only the first N', () => {
    expect(projectionSummary({ objects: 200, morphisms: 41, truncated: true })).toBe(
      'first 200 objects, 41 relations',
    );
    expect(projectionSummary({ objects: 1, morphisms: 1, truncated: false })).toBe(
      '1 object, 1 relation',
    );
  });

  test('a qualified name reads by its schema-local tail', () => {
    expect(localName('decision_overlay.Decision')).toBe('Decision');
    expect(localName('Decision')).toBe('Decision');
  });
});

describe('counts and their nouns agree', () => {
  test('one of a thing is not "1 things"', () => {
    expect(plural(1, 'law')).toBe('1 law');
    expect(plural(2, 'law')).toBe('2 laws');
    expect(plural(0, 'law')).toBe('0 laws');
    expect(plural(1, 'entity', 'entities')).toBe('1 entity');
    expect(plural(3, 'entity', 'entities')).toBe('3 entities');
  });

  test('a schema’s registration reads as a sentence of counts', () => {
    expect(
      registrationSummary({
        entities: ['Decision'],
        attributeCount: 3,
        arrowCount: 1,
        equationCount: 1,
        approvalRequired: true,
      }),
    ).toBe('1 entity · 3 attributes · 1 arrow · 1 law · approval required');
  });

  test('a schema that needs no approval says so rather than staying silent', () => {
    expect(
      registrationSummary({
        entities: [],
        attributeCount: 0,
        arrowCount: 0,
        equationCount: 0,
        approvalRequired: false,
      }),
    ).toContain('no approval required');
  });
});
