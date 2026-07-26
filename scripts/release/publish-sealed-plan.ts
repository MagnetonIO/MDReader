#!/usr/bin/env node

import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { publishReleaseEntries } from './publication-policy';
import {
  assertPublisherMemoryEnvelope,
  readBoundedFile,
  R2FetchStore,
} from './sealed-r2-store';
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
  assertPublisherMemoryEnvelope();

  const publisherContents = await readFile(fileURLToPath(import.meta.url));
  if (
    sealedPublicationPlanSha256(publisherContents) !== expectedPublisherSha256
  ) {
    throw new Error('sealed publisher does not match the approved SHA-256');
  }
  let contents = await readBoundedFile(planPath);
  if (sealedPublicationPlanSha256(contents) !== expectedSha256) {
    throw new Error('sealed publication plan does not match the approved SHA-256');
  }
  let parsed: unknown = JSON.parse(Buffer.from(contents).toString('utf8'));
  contents = new Uint8Array();
  const plan = parseSealedPublicationPlan(parsed);
  parsed = null;
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
    contents: null,
  }));
  const sealedEntries = new Map(
    plan.entries.map((entry) => [entry.key, entry]),
  );
  const store = new R2FetchStore({
    accountId: environment('CLOUDFLARE_ACCOUNT_ID'),
    accessKeyId: environment('MDREADER_R2_ACCESS_KEY_ID'),
    secretAccessKey: environment('MDREADER_R2_SECRET_ACCESS_KEY'),
  }, bucket);
  await publishReleaseEntries(
    entries,
    async (entry) => {
      const sealed = sealedEntries.get(entry.key);
      if (!sealed) {
        throw new Error(`sealed publication body is missing for ${entry.key}`);
      }
      const encoded = sealed.bodyBase64;
      sealed.bodyBase64 = '';
      return Buffer.from(encoded, 'base64');
    },
    {
      read: (entry, desiredBytes) => store.read(entry, desiredBytes),
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
