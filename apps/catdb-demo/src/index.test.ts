import { describe, expect, it } from 'vitest';

import { acceptsPublicRequest } from './routes';

describe('public demo Worker route set', () => {
  it('accepts only the exact read surface', () => {
    for (const [method, path] of [
      ['POST', '/v1/search'],
      ['POST', '/v1/read'],
      ['POST', '/v1/related'],
      ['GET', '/v1/sources'],
      ['GET', '/v1/documents'],
      ['GET', '/v1/runtime'],
    ]) {
      expect(acceptsPublicRequest(new Request(`https://demo.test${path}`, { method }))).toBe(true);
    }
    for (const [method, path] of [
      ['POST', '/v1/apply'],
      ['POST', '/v1/index'],
      ['POST', '/v1/validate'],
      ['POST', '/mcp'],
      ['GET', '/v1/index_status'],
      ['GET', '/v1/search?upstream=https://example.com'],
    ]) {
      expect(acceptsPublicRequest(new Request(`https://demo.test${path}`, { method }))).toBe(false);
    }
  });

  it('contains no mutable origin, credential, or desktop import', async () => {
    const source = await import('node:fs/promises').then((fs) =>
      fs.readFile(new URL('./index.ts', import.meta.url), 'utf8'),
    );
    for (const forbidden of [
      'CATDB_SERVER_URL',
      'authorization',
      '@tauri-apps',
      '/v1/apply',
      '/v1/index"',
    ]) {
      expect(source).not.toContain(forbidden);
    }
  });
});
