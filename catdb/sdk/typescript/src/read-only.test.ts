import { readFile } from 'node:fs/promises';

import { describe, expect, it, vi } from 'vitest';

import { createReadOnlyCatdbClient } from './read-only.js';

describe('createReadOnlyCatdbClient', () => {
  it('exposes and calls only the five public read routes', async () => {
    const calls: string[] = [];
    const fetch = vi.fn(async (input: string | URL | Request) => {
      calls.push(String(input));
      return Response.json({});
    }) as typeof globalThis.fetch;
    const client = createReadOnlyCatdbClient({
      baseUrl: 'https://catdb.test/',
      fetch,
    });

    await client.search({ q: 'schema' });
    await client.read({ reference: 'notes/schema.md' });
    await client.related({ reference: 'notes/schema.md' });
    await client.sources();
    await client.documents();

    expect(calls).toEqual([
      'https://catdb.test/v1/search',
      'https://catdb.test/v1/read',
      'https://catdb.test/v1/related',
      'https://catdb.test/v1/sources',
      'https://catdb.test/v1/documents',
    ]);
    expect(Object.keys(client).sort()).toEqual([
      'documents',
      'read',
      'related',
      'search',
      'sources',
    ]);
  });

  it('contains no mutable or non-public route literal', async () => {
    const source = await readFile(new URL('./read-only.ts', import.meta.url), 'utf8');
    expect(source).toContain('PublicSearchResponse');
    expect(source).not.toMatch(/\bSearchResponse\b/);
    for (const forbidden of [
      '/v1/apply',
      '/v1/build_context',
      '/v1/explain',
      '/v1/index',
      '/v1/index_status',
      '/v1/query',
      '/v1/runtime',
      '/v1/validate',
    ]) {
      expect(source).not.toContain(forbidden);
    }
  });
});
