import {
  GetObjectCommand,
  PutObjectCommand,
  S3Client,
  type PutObjectCommandInput,
} from '@aws-sdk/client-s3';

import type { PublicationEntry } from './release-pipeline';

export interface R2PublisherCredentials {
  accountId: string;
  accessKeyId: string;
  secretAccessKey: string;
}

export interface StablePointerState {
  version: string;
  etag: string;
}

export function createR2Client(
  credentials: R2PublisherCredentials,
): S3Client {
  return new S3Client({
    region: 'auto',
    endpoint: `https://${credentials.accountId}.r2.cloudflarestorage.com`,
    credentials: {
      accessKeyId: credentials.accessKeyId,
      secretAccessKey: credentials.secretAccessKey,
    },
  });
}

/**
 * R2 supports conditional PutObject operations. Versioned release objects use
 * `If-None-Match: *`, so retries can never replace bytes already published at
 * an immutable URL. Only channel metadata is intentionally replaceable.
 */
export function createR2PutInput(
  bucket: string,
  entry: PublicationEntry,
  body: Uint8Array | string,
  expectedEtag?: string | null,
): PutObjectCommandInput {
  if (!entry.immutable && expectedEtag === undefined) {
    throw new Error(`mutable release pointer ${entry.key} requires an ETag condition`);
  }
  return {
    Bucket: bucket,
    Key: entry.key,
    Body: body,
    ContentType: entry.key.endsWith('.json')
      ? 'application/json'
      : entry.key.endsWith('.sig')
        ? 'text/plain; charset=utf-8'
        : 'application/octet-stream',
    CacheControl: entry.immutable
      ? 'public, max-age=31536000, immutable'
      : 'no-cache',
    ...(entry.immutable || expectedEtag === null
      ? { IfNoneMatch: '*' }
      : expectedEtag
        ? { IfMatch: expectedEtag }
        : {}),
  };
}

function parsedSemver(version: string): {
  core: [bigint, bigint, bigint];
  prerelease: string[] | null;
} {
  const match =
    /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/.exec(
      version,
    );
  if (!match) throw new Error(`invalid stable-channel semantic version ${version}`);
  return {
    core: [BigInt(match[1]!), BigInt(match[2]!), BigInt(match[3]!)],
    prerelease: match[4]?.split('.') ?? null,
  };
}

function compareIdentifier(left: string, right: string): number {
  const leftNumeric = /^\d+$/.test(left);
  const rightNumeric = /^\d+$/.test(right);
  if (leftNumeric && rightNumeric) {
    const a = BigInt(left);
    const b = BigInt(right);
    return a < b ? -1 : a > b ? 1 : 0;
  }
  if (leftNumeric !== rightNumeric) return leftNumeric ? -1 : 1;
  return left.localeCompare(right);
}

function compareSemver(left: string, right: string): number {
  const a = parsedSemver(left);
  const b = parsedSemver(right);
  for (let index = 0; index < 3; index += 1) {
    if (a.core[index] !== b.core[index]) {
      return a.core[index]! < b.core[index]! ? -1 : 1;
    }
  }
  if (a.prerelease === null || b.prerelease === null) {
    return a.prerelease === b.prerelease ? 0 : a.prerelease === null ? 1 : -1;
  }
  const count = Math.max(a.prerelease.length, b.prerelease.length);
  for (let index = 0; index < count; index += 1) {
    const leftPart = a.prerelease[index];
    const rightPart = b.prerelease[index];
    if (leftPart === undefined || rightPart === undefined) {
      return leftPart === rightPart ? 0 : leftPart === undefined ? -1 : 1;
    }
    const order = compareIdentifier(leftPart, rightPart);
    if (order !== 0) return order;
  }
  return 0;
}

export function assertForwardStableVersion(
  currentVersion: string | null,
  nextVersion: string,
): void {
  parsedSemver(nextVersion);
  if (currentVersion !== null && compareSemver(nextVersion, currentVersion) <= 0) {
    throw new Error(
      `stable release must move forward: current ${currentVersion}, requested ${nextVersion}`,
    );
  }
}

export async function readR2StablePointer(
  client: S3Client,
  bucket: string,
  key: string,
): Promise<StablePointerState | null> {
  try {
    const response = await client.send(
      new GetObjectCommand({ Bucket: bucket, Key: key }),
    );
    if (!response.ETag) {
      throw new Error(`stable pointer ${key} has no ETag`);
    }
    const body = await response.Body?.transformToString();
    if (!body) throw new Error(`stable pointer ${key} has no body`);
    const parsed = JSON.parse(body) as { version?: unknown };
    if (typeof parsed.version !== 'string') {
      throw new Error(`stable pointer ${key} has no semantic version`);
    }
    parsedSemver(parsed.version);
    return { version: parsed.version, etag: response.ETag };
  } catch (error) {
    const candidate = error as {
      name?: string;
      $metadata?: { httpStatusCode?: number };
    };
    if (
      candidate.name === 'NoSuchKey' ||
      candidate.name === 'NotFound' ||
      candidate.$metadata?.httpStatusCode === 404
    ) {
      return null;
    }
    throw error;
  }
}

export async function putR2ReleaseEntry(
  client: S3Client,
  bucket: string,
  entry: PublicationEntry,
  body: Uint8Array | string,
  expectedEtag?: string | null,
): Promise<void> {
  await client.send(
    new PutObjectCommand(createR2PutInput(bucket, entry, body, expectedEtag)),
  );
}
