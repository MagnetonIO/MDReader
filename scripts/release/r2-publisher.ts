import {
  GetObjectCommand,
  PutObjectCommand,
  S3Client,
  type PutObjectCommandInput,
} from '@aws-sdk/client-s3';

import type { PublicationEntry } from './release-pipeline';
import {
  assertForwardStableVersion,
  publishReleaseEntries,
  type ConditionalReleaseStore,
  type R2ObjectState,
} from './publication-policy';

export {
  assertForwardStableVersion,
  publishReleaseEntries,
  type ConditionalReleaseStore,
  type R2ObjectState,
} from './publication-policy';

export interface R2PublisherCredentials {
  accountId: string;
  accessKeyId: string;
  secretAccessKey: string;
}

export interface StablePointerState {
  version: string;
  etag: string;
  body: string;
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
    assertForwardStableVersion(null, parsed.version);
    return { version: parsed.version, etag: response.ETag, body };
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

export async function readR2Object(
  client: S3Client,
  bucket: string,
  key: string,
): Promise<R2ObjectState | null> {
  try {
    const response = await client.send(
      new GetObjectCommand({ Bucket: bucket, Key: key }),
    );
    if (!response.ETag) throw new Error(`release object ${key} has no ETag`);
    const body = await response.Body?.transformToByteArray();
    if (!body) throw new Error(`release object ${key} has no body`);
    return { etag: response.ETag, body };
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
