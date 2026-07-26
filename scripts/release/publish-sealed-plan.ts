#!/usr/bin/env node

import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  publishReleaseEntries,
  type ConditionalReleaseStore,
  type R2ObjectState,
} from './publication-policy';
import type { PublicationEntry } from './release-pipeline';
import {
  createSignedR2Request,
  type R2Credentials,
} from './r2-sigv4';
import {
  parseSealedPublicationPlan,
  sealedPublicationPlanSha256,
} from './sealed-publication';

function option(args: string[], name: string): string {
  const index = args.indexOf(name);
  const value = index >= 0 ? args[index + 1] : undefined;
  if (!value || value.startsWith('--')) {
    throw new Error(`missing required option ${name}`);
  }
  return value;
}

function environment(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`missing required publication credential ${name}`);
  return value;
}

class R2FetchStore implements ConditionalReleaseStore {
  constructor(
    private readonly credentials: R2Credentials,
    private readonly bucket: string,
  ) {}

  async read(key: string): Promise<R2ObjectState | null> {
    const request = createSignedR2Request(
      this.credentials,
      this.bucket,
      'GET',
      key,
      new Uint8Array(),
      {},
    );
    const response = await fetch(request.url, request.init);
    if (response.status === 404) return null;
    if (!response.ok) {
      throw new Error(`R2 read failed for ${key}: HTTP ${response.status}`);
    }
    const etag = response.headers.get('etag');
    if (!etag) throw new Error(`R2 object ${key} has no ETag`);
    return {
      etag,
      body: new Uint8Array(await response.arrayBuffer()),
    };
  }

  async put(
    entry: PublicationEntry,
    body: Uint8Array,
    expectedEtag?: string | null,
  ): Promise<void> {
    if (!entry.immutable && expectedEtag === undefined) {
      throw new Error(
        `mutable release pointer ${entry.key} requires an ETag condition`,
      );
    }
    const headers: Record<string, string> = {
      'content-type': entry.key.endsWith('.json')
        ? 'application/json'
        : entry.key.endsWith('.sig')
          ? 'text/plain; charset=utf-8'
          : 'application/octet-stream',
      'cache-control': entry.immutable
        ? 'public, max-age=31536000, immutable'
        : 'no-cache',
      ...(entry.immutable || expectedEtag === null
        ? { 'if-none-match': '*' }
        : expectedEtag
          ? { 'if-match': expectedEtag }
          : {}),
    };
    const request = createSignedR2Request(
      this.credentials,
      this.bucket,
      'PUT',
      entry.key,
      body,
      headers,
    );
    const response = await fetch(request.url, request.init);
    if (!response.ok) {
      throw new Error(`R2 write failed for ${entry.key}: HTTP ${response.status}`);
    }
  }
}

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const planPath = resolve(option(args, '--plan'));
  const expectedSha256 = option(args, '--plan-sha256');
  const expectedPublisherSha256 = option(args, '--publisher-sha256');
  const expectedSourceCommit = option(args, '--source-commit');
  const expectedVersion = option(args, '--version');
  const bucket = option(args, '--bucket');
  if (!/^[0-9a-f]{64}$/.test(expectedSha256)) {
    throw new Error('expected sealed plan SHA-256 is invalid');
  }
  if (!/^[0-9a-f]{64}$/.test(expectedPublisherSha256)) {
    throw new Error('expected sealed publisher SHA-256 is invalid');
  }
  if (!/^[a-z0-9][a-z0-9-]{1,62}$/.test(bucket)) {
    throw new Error('invalid R2 bucket name');
  }

  const publisherContents = await readFile(fileURLToPath(import.meta.url));
  if (
    sealedPublicationPlanSha256(publisherContents) !== expectedPublisherSha256
  ) {
    throw new Error('sealed publisher does not match the approved SHA-256');
  }
  const contents = await readFile(planPath);
  if (sealedPublicationPlanSha256(contents) !== expectedSha256) {
    throw new Error('sealed publication plan does not match the approved SHA-256');
  }
  const plan = parseSealedPublicationPlan(
    JSON.parse(contents.toString('utf8')) as unknown,
  );
  if (
    plan.sourceCommit !== expectedSourceCommit ||
    plan.version !== expectedVersion
  ) {
    throw new Error('sealed publication plan source/version mismatch');
  }

  const entries = plan.entries.map((entry) => ({
    kind: entry.kind,
    key: entry.key,
    immutable: entry.immutable,
    localPath: null,
    contents: Buffer.from(entry.bodyBase64, 'base64').toString('base64'),
  }));
  const bodies = new Map(
    plan.entries.map((entry) => [
      entry.key,
      Buffer.from(entry.bodyBase64, 'base64'),
    ]),
  );
  const store = new R2FetchStore({
    accountId: environment('CLOUDFLARE_ACCOUNT_ID'),
    accessKeyId: environment('MDREADER_R2_ACCESS_KEY_ID'),
    secretAccessKey: environment('MDREADER_R2_SECRET_ACCESS_KEY'),
  }, bucket);
  await publishReleaseEntries(
    entries,
    async (entry) => bodies.get(entry.key)!,
    {
      read: (key) => store.read(key),
      put: async (entry, body, expectedEtag) => {
        process.stdout.write(`publishing ${entry.kind} ${entry.key}\n`);
        await store.put(entry, body, expectedEtag);
      },
    },
  );
}

main().catch((error: unknown) => {
  process.stderr.write(
    `sealed publication refused: ${
      error instanceof Error ? error.message : String(error)
    }\n`,
  );
  process.exitCode = 1;
});
