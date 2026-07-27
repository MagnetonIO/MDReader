import { describe, expect, it } from 'vitest';

import { forwardPublicRequest } from './bounds';
import { acceptsPublicRequest } from './routes';

describe('public demo Worker route set', () => {
  it('accepts only the exact read surface', () => {
    for (const [method, path] of [
      ['POST', '/v1/search'],
      ['POST', '/v1/read'],
      ['POST', '/v1/related'],
      ['GET', '/v1/sources'],
      ['GET', '/v1/documents'],
    ]) {
      expect(acceptsPublicRequest(new Request(`https://demo.test${path}`, { method }))).toBe(true);
    }
    for (const [method, path] of [
      ['POST', '/v1/apply'],
      ['POST', '/v1/index'],
      ['POST', '/v1/validate'],
      ['POST', '/mcp'],
      ['GET', '/v1/index_status'],
      ['GET', '/v1/runtime'],
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

  it('pins compatible image inputs and keeps the unapproved runtime fail-closed', async () => {
    const { readFile } = await import('node:fs/promises');
    const dockerfile = await readFile(
      new URL('../../../Dockerfile.catdb-demo', import.meta.url),
      'utf8',
    );
    const server = await readFile(
      new URL(
        '../../mdreader/crates/mdreader-ingest/src/bin/catdb-demo-server.rs',
        import.meta.url,
      ),
      'utf8',
    );
    const cliManifest = await readFile(
      new URL('../../../catdb/crates/catdb-cli/Cargo.toml', import.meta.url),
      'utf8',
    );
    const ingestManifest = await readFile(
      new URL(
        '../../mdreader/crates/mdreader-ingest/Cargo.toml',
        import.meta.url,
      ),
      'utf8',
    );
    const jsWorkflow = await readFile(
      new URL('../../../.github/workflows/js.yml', import.meta.url),
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
    expect(dockerfile).toContain('--no-default-features');
    expect(cliManifest).toContain(
      'external-connectors = [\n    "dep:catdb-connector-dropbox"',
    );
    for (const connector of [
      'dropbox',
      'duckdb',
      'google-drive',
      'postgres',
      'snowflake',
    ]) {
      expect(cliManifest).toMatch(
        new RegExp(
          `catdb-connector-${connector} = \\{[^\\n]+optional = true`,
        ),
      );
    }
    expect(ingestManifest).toContain(
      'external-connectors = ["catdb-cli/external-connectors"]',
    );
    expect(jsWorkflow).toContain(
      'pnpm --filter @catdb/demo-runtime cf:dry-run',
    );
    expect(dockerfile).toContain('/image/metadata/workspace.sha256');
    expect(dockerfile).toContain('find /image -type f -exec chmod 0444');
    expect(dockerfile).toContain('USER 10001:10001');
    expect(dockerfile).not.toContain('--verify-read-only-image');
    expect(dockerfile.match(/sha256sum/g)).toHaveLength(1);
    expect(dockerfile).not.toMatch(/^FROM [^@\n]+$/m);
    expect(server).toContain('authoritative catalog revision verification is unavailable');
    expect(server).not.toContain('open_read_only');
  });

  it('bounds direct Container requests by elapsed time and response bytes', async () => {
    const oversized = new Uint8Array(2 * 1024 * 1024 + 1);
    const tooLarge = await forwardPublicRequest(
      new Request('https://demo.test/v1/sources'),
      async () => new Response(oversized),
      { timeoutMs: 1_000 },
    );
    expect(tooLarge.status).toBe(502);
    expect(await tooLarge.json()).toMatchObject({ error: 'response_too_large' });

    const timedOut = await forwardPublicRequest(
      new Request('https://demo.test/v1/sources'),
      () => new Promise(() => {}),
      { timeoutMs: 10 },
    );
    expect(timedOut.status).toBe(504);
    expect(await timedOut.json()).toMatchObject({ error: 'upstream_timeout' });
  });

  it('rejects a chunked request that exceeds the real byte limit before forwarding', async () => {
    let forwarded = false;
    const body = new ReadableStream({
      start(controller) {
        controller.enqueue(new Uint8Array(64 * 1024));
        controller.enqueue(new Uint8Array([1]));
        controller.close();
      },
    });
    const response = await forwardPublicRequest(
      new Request('https://demo.test/v1/search', {
        method: 'POST',
        body,
        duplex: 'half',
      } as RequestInit),
      async () => {
        forwarded = true;
        return Response.json({});
      },
    );
    expect(response.status).toBe(413);
    expect(forwarded).toBe(false);
  });

  it('cancels a stalled inbound body when the shared deadline expires', async () => {
    let cancelled = false;
    let forwarded = false;
    const body = new ReadableStream({
      start(controller) {
        controller.enqueue(new Uint8Array([1]));
      },
      cancel() {
        cancelled = true;
      },
    });
    const response = await forwardPublicRequest(
      new Request('https://demo.test/v1/search', {
        method: 'POST',
        body,
        duplex: 'half',
      } as RequestInit),
      async () => {
        forwarded = true;
        return Response.json({});
      },
      { timeoutMs: 10 },
    );
    expect(response.status).toBe(504);
    expect(cancelled).toBe(true);
    expect(forwarded).toBe(false);
  });
});
