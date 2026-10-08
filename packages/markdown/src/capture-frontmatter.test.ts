import { describe, expect, test } from 'vitest';
import { toHtml } from 'hast-util-to-html';
import { parseMarkdown } from './pipeline.js';

const metadata = JSON.stringify({ schema_version: 1, capture_is_not_approval: true, occurrences: [{ session_id: 'synthetic' }] });
const header = `<!-- mdreader-capture:v1 -->\n---\nmdreader_capture: ${metadata}\n---\n\n`;

describe('capture v1 frontmatter presentation', () => {
  test.each(['\n', '\r\n'])('renders the body and preserves raw source lines with %j endings', (ending) => {
    const source = `${header}# Visible plan\n\nExact body.\n`.replaceAll('\n', ending);
    const parsed = parseMarkdown(source);
    const html = toHtml(parsed.hast);
    expect(parsed.frontmatter).toMatchObject({ mdreader_capture: { schema_version: 1 } });
    expect(parsed.outline).toEqual([{ depth: 1, text: 'Visible plan', slug: 'user-content-visible-plan', lineStart: 6 }]);
    expect(html).toContain('data-line-start="6"');
    expect(html).toContain('data-line-start="8"');
    expect(html).toContain('Exact body.');
    expect(html).not.toContain('mdreader_capture');
    expect(parsed.stats.byteLength).toBe(new TextEncoder().encode(source).length);
  });

  test('does not hide ordinary or malformed Markdown using a similar comment', () => {
    const malformed = header.replace(metadata, 'not-json');
    expect(toHtml(parseMarkdown(`${malformed}# Body`).hast)).toContain('mdreader_capture');
    expect(toHtml(parseMarkdown('<!-- note -->\n---\nOrdinary title\n---\n').hast)).toContain('Ordinary title');
  });
});
