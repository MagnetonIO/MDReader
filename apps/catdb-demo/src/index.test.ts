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

  it('pins compatible image inputs and bakes an actual read-only workspace digest', async () => {
    const { readFile } = await import('node:fs/promises');
    const dockerfile = await readFile(
      new URL('../../../Dockerfile.catdb-demo', import.meta.url),
      'utf8',
    );
    expect(dockerfile).toMatch(
      /^FROM rust:1\.94\.0-bookworm@sha256:[0-9a-f]{64} AS builder$/m,
    );
    expect(dockerfile).toMatch(
      /^FROM debian:bookworm-slim@sha256:[0-9a-f]{64}$/m,
    );
    expect(dockerfile).toContain('cargo fetch --locked');
    expect(dockerfile).toContain('cargo build --locked --offline');
    expect(dockerfile).toContain('/image/metadata/workspace.sha256');
    expect(dockerfile).toContain('find /image -type f -exec chmod 0444');
    expect(dockerfile).toContain('USER 10001:10001');
    expect(dockerfile).not.toMatch(/^FROM [^@\n]+$/m);
  });
});
