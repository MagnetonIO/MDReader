/**
 * The two coordinate translations, asserted against real DOM and real paths.
 *
 * The selection reader is what makes the Inspector a *reading* gesture rather
 * than a form: highlight a passage, get its provenance. The path resolver is
 * what makes a located violation clickable. Both are pure, so both are proven
 * here rather than in an integration test that could pass for the wrong reason.
 */

import { afterEach, describe, expect, test } from 'vitest';

import { resolveSourcePath, spanFromRange, spanFromSelection } from './source-span.js';

function mount(html: string): HTMLElement {
  const host = document.createElement('div');
  host.className = 'reader-host';
  host.innerHTML = html;
  document.body.append(host);
  return host;
}

afterEach(() => {
  document.body.innerHTML = '';
});

const DOC = `
  <p data-line-start="3" data-line-end="5">First paragraph</p>
  <p data-line-start="7" data-line-end="9">Second paragraph</p>
  <p>Untracked block</p>
`;

describe('reading a selection as a line range', () => {
  test('a selection inside one block cites that block’s whole source range', () => {
    const host = mount(DOC);
    const block = host.querySelector('[data-line-start="3"]')!;
    const range = document.createRange();
    range.selectNodeContents(block);

    expect(spanFromRange(range, host)).toEqual({ startLine: 3, endLine: 5 });
  });

  test('a selection spanning two blocks cites the union of both', () => {
    // Both paragraphs contributed text a claim could have been derived from, so
    // both are cited — not just the block the selection started in.
    const host = mount(DOC);
    const range = document.createRange();
    range.setStart(host.querySelector('[data-line-start="3"]')!.firstChild!, 2);
    range.setEnd(host.querySelector('[data-line-start="7"]')!.firstChild!, 4);

    expect(spanFromRange(range, host)).toEqual({ startLine: 3, endLine: 9 });
  });

  test('a selection touching no located block yields nothing', () => {
    const host = mount(DOC);
    const range = document.createRange();
    range.selectNodeContents(host.querySelectorAll('p')[2]!);

    expect(spanFromRange(range, host)).toBeNull();
  });

  test('a collapsed caret is not an inspection request', () => {
    const host = mount(DOC);
    const selection = window.getSelection()!;
    selection.removeAllRanges();
    const range = document.createRange();
    range.setStart(host.querySelector('[data-line-start="3"]')!.firstChild!, 2);
    range.collapse(true);
    selection.addRange(range);

    expect(spanFromSelection(selection, host)).toBeNull();
  });

  test('a selection outside the reading column is not about this document', () => {
    const host = mount(DOC);
    const outside = document.createElement('p');
    outside.setAttribute('data-line-start', '99');
    outside.textContent = 'elsewhere in the chrome';
    document.body.append(outside);

    const selection = window.getSelection()!;
    selection.removeAllRanges();
    const range = document.createRange();
    range.selectNodeContents(outside);
    selection.addRange(range);

    expect(spanFromSelection(selection, host)).toBeNull();
  });

  test('a real selection inside the column reads back its lines', () => {
    const host = mount(DOC);
    const selection = window.getSelection()!;
    selection.removeAllRanges();
    const range = document.createRange();
    range.selectNodeContents(host.querySelector('[data-line-start="7"]')!);
    selection.addRange(range);

    expect(spanFromSelection(selection, host)).toEqual({ startLine: 7, endLine: 9 });
  });
});

describe('resolving a located violation to an openable document', () => {
  const files = [
    { path: '/ws/notes/adr-3.md' },
    { path: '/ws/notes/other-adr-3.md' },
    { path: '/ws/index.md' },
  ];

  test('prefers the workspace listing’s own spelling of the path', () => {
    expect(resolveSourcePath('notes/adr-3.md', '/ws', files)).toBe('/ws/notes/adr-3.md');
  });

  test('never matches a look-alike that is not on a segment boundary', () => {
    // `/ws/notes/other-adr-3.md` ends with the characters `adr-3.md`, so a naive
    // `endsWith` would find TWO candidates and either pick wrong or give up.
    // Matching on a segment boundary leaves exactly one.
    expect(resolveSourcePath('adr-3.md', '/ws', files)).toBe('/ws/notes/adr-3.md');
    expect(resolveSourcePath('adr-3.md', '/ws', files)).not.toBe('/ws/notes/other-adr-3.md');
  });

  test('falls back to joining the workspace root when the file is not listed yet', () => {
    expect(resolveSourcePath('notes/new.md', '/ws', files)).toBe('/ws/notes/new.md');
  });

  test('finds a unique suffix match when the root does not line up', () => {
    expect(resolveSourcePath('notes/adr-3.md', null, files)).toBe('/ws/notes/adr-3.md');
  });

  test('gives up honestly rather than inventing a path', () => {
    expect(resolveSourcePath('notes/missing.md', null, files)).toBeNull();
    expect(resolveSourcePath('', '/ws', files)).toBeNull();
  });

  test('tolerates windows separators and a trailing root slash', () => {
    expect(resolveSourcePath('notes\\adr-3.md', '/ws/', files)).toBe('/ws/notes/adr-3.md');
  });
});
