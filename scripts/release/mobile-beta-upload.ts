import { createHash, createSign } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { constants, type Stats } from 'node:fs';
import {
  lstat,
  mkdtemp,
  open,
  rm,
  type FileHandle,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import {
  MOBILE_PRODUCTION_PACKAGE_ID,
  snapshotOpenedMobileBetaArtifact,
} from './mobile-beta-manifest';

const GOOGLE_TOKEN_ENDPOINT = 'https://oauth2.googleapis.com/token';
const GOOGLE_PUBLISHER_ENDPOINT =
  'https://androidpublisher.googleapis.com/androidpublisher/v3';
const GOOGLE_UPLOAD_ENDPOINT =
  'https://androidpublisher.googleapis.com/upload/androidpublisher/v3';
const GOOGLE_PLAY_SCOPE =
  'https://www.googleapis.com/auth/androidpublisher';
const APP_STORE_CONNECT_ENDPOINT =
  'https://api.appstoreconnect.apple.com/v1';
const MAX_ANDROID_BUNDLE_BYTES = 200 * 1024 * 1024;
const MAX_PROVIDER_RESPONSE_BYTES = 2 * 1024 * 1024;
const APP_STORE_BUILD_POLL_ATTEMPTS = 20;
const APP_STORE_GROUP_PAGE_LIMIT = 10;
const RESERVED_PLAY_TRACKS = new Set([
  'production',
  'beta',
  'alpha',
  'qa',
  'internal',
]);

export type ExternalToolRunner = (
  command: string,
  args: readonly string[],
) => string;

interface TestFlightUpload {
  artifactPath: string;
  expectedBytes: number;
  expectedSha256: string;
  expectedVersion: string;
  expectedBuildNumber: string;
  expectedBetaGroupId: string;
  apiKeyId: string;
  issuerId: string;
  privateKeyPath: string;
}

interface GooglePlayUpload {
  artifactPath: string;
  expectedBytes: number;
  expectedSha256: string;
  serviceAccountJson: string;
  expectedServiceAccountEmail: string;
  packageId: string;
  closedTrack: string;
  versionCode: string;
  releaseName: string;
}

interface GoogleServiceAccount {
  type: 'service_account';
  client_email: string;
  private_key: string;
  token_uri: typeof GOOGLE_TOKEN_ENDPOINT;
}

interface GooglePlayReceipt {
  editId: string;
  packageId: typeof MOBILE_PRODUCTION_PACKAGE_ID;
  track: string;
  versionCode: string;
  committed: true;
  providerVerified: true;
}

export class ProviderReadBackTimeout extends Error {
  constructor(provider: string) {
    super(`${provider} read-back timed out before the exact build appeared`);
    this.name = 'ProviderReadBackTimeout';
  }
}

function runExternal(command: string, args: readonly string[]): string {
  const result = spawnSync(command, [...args], {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
    maxBuffer: 16 * 1024 * 1024,
    timeout: 30 * 60 * 1000,
    killSignal: 'SIGKILL',
  });
  if (result.status !== 0) {
    throw new Error(`${command} failed`);
  }
  return String(result.stdout);
}

async function requireRegularFile(
  path: string,
  field: string,
  maximumBytes?: number,
): Promise<void> {
  const metadata = await lstat(path);
  if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.size <= 0) {
    throw new Error(`${field} must be a non-empty regular file`);
  }
  if (maximumBytes !== undefined && metadata.size > maximumBytes) {
    throw new Error(`${field} exceeds the ${maximumBytes}-byte limit`);
  }
}

interface OpenedArtifact {
  handle: FileHandle;
  identity: Stats;
  bytes: number;
  sha256: string;
}

function requireSameArtifactIdentity(
  before: Stats,
  after: Stats,
  maximumBytes?: number,
): void {
  if (
    !after.isFile() ||
    before.dev !== after.dev ||
    before.ino !== after.ino ||
    before.size !== after.size ||
    before.mtimeMs !== after.mtimeMs ||
    after.size <= 0 ||
    (maximumBytes !== undefined && after.size > maximumBytes)
  ) {
    throw new Error('mobile beta upload artifact changed or was replaced');
  }
}

async function hashOpenedArtifact(
  handle: FileHandle,
  identity: Stats,
  retainBody = false,
): Promise<{ bytes: number; sha256: string; body?: Buffer }> {
  const body = retainBody ? Buffer.allocUnsafe(identity.size) : undefined;
  const buffer = body ?? Buffer.allocUnsafe(1024 * 1024);
  const digest = createHash('sha256');
  let offset = 0;
  while (offset < identity.size) {
    const length = Math.min(buffer.byteLength, identity.size - offset);
    const targetOffset = body ? offset : 0;
    const { bytesRead } = await handle.read(
      buffer,
      targetOffset,
      length,
      offset,
    );
    if (bytesRead <= 0) {
      throw new Error('mobile beta upload artifact ended before its opened size');
    }
    digest.update(buffer.subarray(targetOffset, targetOffset + bytesRead));
    offset += bytesRead;
  }
  requireSameArtifactIdentity(identity, await handle.stat());
  return { bytes: offset, sha256: digest.digest('hex'), body };
}

async function openArtifact(
  path: string,
  expectedBytes: number,
  expectedSha256: string,
  maximumBytes?: number,
): Promise<OpenedArtifact> {
  if (
    !Number.isSafeInteger(expectedBytes) ||
    expectedBytes <= 0 ||
    !/^[0-9a-f]{64}$/.test(expectedSha256)
  ) {
    throw new Error('reviewed mobile beta artifact identity is invalid');
  }
  const pathIdentity = await lstat(path);
  if (!pathIdentity.isFile() || pathIdentity.isSymbolicLink()) {
    throw new Error('mobile beta upload artifact must be a regular file');
  }
  const handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const identity = await handle.stat();
    requireSameArtifactIdentity(pathIdentity, identity, maximumBytes);
    const hashed = await hashOpenedArtifact(handle, identity);
    if (
      hashed.bytes !== expectedBytes ||
      hashed.sha256 !== expectedSha256
    ) {
      throw new Error('mobile beta upload artifact differs from its manifest');
    }
    return { handle, identity, bytes: hashed.bytes, sha256: hashed.sha256 };
  } catch (error) {
    await handle.close();
    throw error;
  }
}

async function readOpenedAndroidBundle(
  opened: OpenedArtifact,
): Promise<Buffer> {
  if (opened.bytes > MAX_ANDROID_BUNDLE_BYTES) {
    throw new Error(
      `Google Play artifact exceeds the ${MAX_ANDROID_BUNDLE_BYTES}-byte limit`,
    );
  }
  const result = await hashOpenedArtifact(
    opened.handle,
    opened.identity,
    true,
  );
  if (
    result.bytes !== opened.bytes ||
    result.sha256 !== opened.sha256 ||
    !result.body
  ) {
    throw new Error('Google Play artifact changed after review');
  }
  return result.body;
}

async function requirePrivateSnapshot(
  path: string,
  bytes: number,
  sha256: string,
  maximumBytes?: number,
): Promise<void> {
  const snapshot = await openArtifact(path, bytes, sha256, maximumBytes);
  await snapshot.handle.close();
}

function parseJsonObject(body: string, field: string): Record<string, unknown> {
  const parsed: unknown = JSON.parse(body);
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error(`${field} is not a JSON object`);
  }
  return parsed as Record<string, unknown>;
}

/**
 * Runs Apple's own validation and upload tool with API-key authentication.
 * Only the key path and non-secret identifiers enter argv; key contents do not.
 */
export async function uploadTestFlight(
  input: TestFlightUpload,
  run: ExternalToolRunner = runExternal,
  fetcher: typeof fetch = fetch,
  nowMs = Date.now(),
  sleep: (milliseconds: number) => Promise<void> = (milliseconds) =>
    new Promise((resolve) => setTimeout(resolve, milliseconds)),
): Promise<Record<string, unknown>> {
  if (!input.artifactPath.toLowerCase().endsWith('.ipa')) {
    throw new Error('TestFlight upload requires an IPA');
  }
  if (!/^[A-Z0-9]{10}$/.test(input.apiKeyId)) {
    throw new Error('App Store Connect API key identifier is invalid');
  }
  if (
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
      input.issuerId,
    )
  ) {
    throw new Error('App Store Connect issuer identifier is invalid');
  }
  if (
    !/^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.test(
      input.expectedVersion,
    ) ||
    !/^[1-9][0-9]*$/.test(input.expectedBuildNumber)
  ) {
    throw new Error('TestFlight version or build number is invalid');
  }
  if (!/^[A-Za-z0-9-]{8,64}$/.test(input.expectedBetaGroupId)) {
    throw new Error('reviewed TestFlight beta-group identifier is invalid');
  }
  await requireRegularFile(input.privateKeyPath, 'App Store Connect private key');
  const keyMetadata = await lstat(input.privateKeyPath);
  if ((keyMetadata.mode & 0o077) !== 0) {
    throw new Error('App Store Connect private key permissions are too broad');
  }

  const privateKeyHandle = await open(
    input.privateKeyPath,
    constants.O_RDONLY | constants.O_NOFOLLOW,
  );
  let artifact: OpenedArtifact | undefined;
  let privateRoot: string | undefined;
  try {
    artifact = await openArtifact(
      input.artifactPath,
      input.expectedBytes,
      input.expectedSha256,
    );
    const openedKey = await privateKeyHandle.stat();
    requireSameArtifactIdentity(keyMetadata, openedKey, 64 * 1024);
    const privateKey = (await privateKeyHandle.readFile()).toString('utf8');
    requireSameArtifactIdentity(openedKey, await privateKeyHandle.stat(), 64 * 1024);
    if (
      !privateKey.includes('-----BEGIN PRIVATE KEY-----') ||
      !privateKey.includes('-----END PRIVATE KEY-----')
    ) {
      throw new Error('App Store Connect private key is malformed');
    }
    privateRoot = await mkdtemp(join(tmpdir(), 'mdreader-testflight-private-'));
    const privateArtifactPath = join(privateRoot, 'MDReader.ipa');
    const privateKeyPath = join(
      privateRoot,
      `AuthKey_${input.apiKeyId}.p8`,
    );
    const privateArtifact = await snapshotOpenedMobileBetaArtifact(
      artifact.handle,
      artifact.identity,
      privateArtifactPath,
    );
    const privateKeySnapshot = await snapshotOpenedMobileBetaArtifact(
      privateKeyHandle,
      openedKey,
      privateKeyPath,
    );
    if (
      privateArtifact.bytes !== input.expectedBytes ||
      privateArtifact.sha256 !== input.expectedSha256
    ) {
      throw new Error('private TestFlight snapshot differs from the manifest');
    }
    const auth = [
      '--api-key',
      input.apiKeyId,
      '--api-issuer',
      input.issuerId,
      '--p8-file-path',
      privateKeyPath,
      '--output-format',
      'json',
    ];
    await requirePrivateSnapshot(
      privateArtifactPath,
      privateArtifact.bytes,
      privateArtifact.sha256,
    );
    await requirePrivateSnapshot(
      privateKeyPath,
      privateKeySnapshot.bytes,
      privateKeySnapshot.sha256,
      64 * 1024,
    );
    run('xcrun', [
      'altool',
      '--validate-app',
      '-f',
      privateArtifactPath,
      '-t',
      'ios',
      ...auth,
    ]);
    await requirePrivateSnapshot(
      privateArtifactPath,
      privateArtifact.bytes,
      privateArtifact.sha256,
    );
    await requirePrivateSnapshot(
      privateKeyPath,
      privateKeySnapshot.bytes,
      privateKeySnapshot.sha256,
      64 * 1024,
    );
    const providerUpload = parseJsonObject(
      run('xcrun', [
        'altool',
        '--upload-app',
        '-f',
        privateArtifactPath,
        '-t',
        'ios',
        ...auth,
      ]),
      'App Store Connect upload receipt',
    );
    await requirePrivateSnapshot(
      privateArtifactPath,
      privateArtifact.bytes,
      privateArtifact.sha256,
    );
    await requirePrivateSnapshot(
      privateKeyPath,
      privateKeySnapshot.bytes,
      privateKeySnapshot.sha256,
      64 * 1024,
    );
    const appStoreConnect = await verifyTestFlightReadBack(
      input,
      privateKey,
      fetcher,
      nowMs,
      sleep,
    );
    await requirePrivateSnapshot(
      privateArtifactPath,
      privateArtifact.bytes,
      privateArtifact.sha256,
    );
    await requirePrivateSnapshot(
      privateKeyPath,
      privateKeySnapshot.bytes,
      privateKeySnapshot.sha256,
      64 * 1024,
    );
    return { providerUpload, appStoreConnect };
  } finally {
    await artifact?.handle.close();
    await privateKeyHandle.close();
    if (privateRoot) {
      await rm(privateRoot, { recursive: true, force: true });
    }
  }
}

function base64url(input: string | Buffer): string {
  return Buffer.from(input).toString('base64url');
}

function serviceAccount(
  encoded: string,
  expectedEmail: string,
): GoogleServiceAccount {
  const parsed = parseJsonObject(encoded, 'Google Play service account');
  if (
    parsed.type !== 'service_account' ||
    parsed.token_uri !== GOOGLE_TOKEN_ENDPOINT ||
    typeof parsed.client_email !== 'string' ||
    typeof parsed.private_key !== 'string' ||
    !parsed.private_key.includes('-----BEGIN PRIVATE KEY-----')
  ) {
    throw new Error('Google Play service account credential is malformed');
  }
  if (
    !/^[^@\s]+@[^@\s]+\.iam\.gserviceaccount\.com$/.test(expectedEmail) ||
    parsed.client_email !== expectedEmail
  ) {
    throw new Error('Google Play service account differs from reviewed authority');
  }
  return parsed as unknown as GoogleServiceAccount;
}

function assertion(account: GoogleServiceAccount, nowMs: number): string {
  const issuedAt = Math.floor(nowMs / 1000);
  const header = base64url(JSON.stringify({ alg: 'RS256', typ: 'JWT' }));
  const payload = base64url(
    JSON.stringify({
      iss: account.client_email,
      scope: GOOGLE_PLAY_SCOPE,
      aud: GOOGLE_TOKEN_ENDPOINT,
      iat: issuedAt,
      exp: issuedAt + 300,
    }),
  );
  const unsigned = `${header}.${payload}`;
  const signer = createSign('RSA-SHA256');
  signer.update(unsigned);
  signer.end();
  return `${unsigned}.${signer.sign(account.private_key).toString('base64url')}`;
}

function appStoreAssertion(
  privateKey: string,
  apiKeyId: string,
  issuerId: string,
  nowMs: number,
): string {
  const issuedAt = Math.floor(nowMs / 1000);
  const header = base64url(
    JSON.stringify({ alg: 'ES256', kid: apiKeyId, typ: 'JWT' }),
  );
  const payload = base64url(
    JSON.stringify({
      iss: issuerId,
      aud: 'appstoreconnect-v1',
      iat: issuedAt,
      exp: issuedAt + 900,
    }),
  );
  const unsigned = `${header}.${payload}`;
  const signer = createSign('SHA256');
  signer.update(unsigned);
  signer.end();
  return `${unsigned}.${signer
    .sign({ key: privateKey, dsaEncoding: 'ieee-p1363' })
    .toString('base64url')}`;
}

async function responseJson(
  response: Response,
  operation: string,
): Promise<Record<string, unknown>> {
  const declaredLength = response.headers.get('content-length');
  if (
    declaredLength !== null &&
    (!/^[0-9]+$/.test(declaredLength) ||
      Number(declaredLength) > MAX_PROVIDER_RESPONSE_BYTES)
  ) {
    throw new Error(`${operation} response exceeds the reviewed byte limit`);
  }
  const chunks: Uint8Array[] = [];
  let received = 0;
  if (response.body) {
    const reader = response.body.getReader();
    try {
      for (;;) {
        const next = await reader.read();
        if (next.done) break;
        received += next.value.byteLength;
        if (received > MAX_PROVIDER_RESPONSE_BYTES) {
          await reader.cancel();
          throw new Error(
            `${operation} response exceeds the reviewed byte limit`,
          );
        }
        chunks.push(next.value);
      }
    } finally {
      reader.releaseLock();
    }
  }
  const bytes = new Uint8Array(received);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  const body = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  let parsed: Record<string, unknown> = {};
  if (body.length > 0) {
    try {
      parsed = parseJsonObject(body, `${operation} response`);
    } catch {
      if (response.ok) throw new Error(`${operation} returned invalid JSON`);
    }
  }
  if (!response.ok) {
    throw new Error(`${operation} failed`);
  }
  return parsed;
}

function publisherHeaders(token: string): Record<string, string> {
  return {
    authorization: `Bearer ${token}`,
    'content-type': 'application/json',
  };
}

function objectArray(
  value: unknown,
  field: string,
): Array<Record<string, unknown>> {
  if (!Array.isArray(value)) throw new Error(`${field} is not an array`);
  return value.map((item) => {
    if (!item || typeof item !== 'object' || Array.isArray(item)) {
      throw new Error(`${field} contains a non-object`);
    }
    return item as Record<string, unknown>;
  });
}

async function verifyTestFlightReadBack(
  input: TestFlightUpload,
  privateKey: string,
  fetcher: typeof fetch,
  nowMs: number,
  sleep: (milliseconds: number) => Promise<void>,
): Promise<Record<string, unknown>> {
  const token = appStoreAssertion(
    privateKey,
    input.apiKeyId,
    input.issuerId,
    nowMs,
  );
  const headers = { authorization: `Bearer ${token}` };
  const appsUrl = new URL(`${APP_STORE_CONNECT_ENDPOINT}/apps`);
  appsUrl.searchParams.set('filter[bundleId]', MOBILE_PRODUCTION_PACKAGE_ID);
  appsUrl.searchParams.set('limit', '2');
  const apps = objectArray(
    (
      await responseJson(
        await fetcher(appsUrl, {
          method: 'GET',
          headers,
          signal: AbortSignal.timeout(120_000),
        }),
        'App Store Connect application read-back',
      )
    ).data,
    'App Store Connect applications',
  );
  if (apps.length !== 1) {
    throw new Error('App Store Connect did not return exactly one production app');
  }
  const app = apps[0]!;
  const appAttributes = app.attributes;
  if (
    app.type !== 'apps' ||
    typeof app.id !== 'string' ||
    !appAttributes ||
    typeof appAttributes !== 'object' ||
    Array.isArray(appAttributes) ||
    (appAttributes as Record<string, unknown>).bundleId !==
      MOBILE_PRODUCTION_PACKAGE_ID
  ) {
    throw new Error('App Store Connect returned a different application');
  }

  for (let attempt = 0; attempt < APP_STORE_BUILD_POLL_ATTEMPTS; attempt += 1) {
    const buildsUrl = new URL(`${APP_STORE_CONNECT_ENDPOINT}/builds`);
    buildsUrl.searchParams.set('filter[app]', app.id);
    buildsUrl.searchParams.set('filter[version]', input.expectedBuildNumber);
    buildsUrl.searchParams.set('include', 'preReleaseVersion');
    buildsUrl.searchParams.set('limit', '2');
    const buildResponse = await responseJson(
      await fetcher(buildsUrl, {
        method: 'GET',
        headers,
        signal: AbortSignal.timeout(120_000),
      }),
      'App Store Connect build read-back',
    );
    const builds = objectArray(
      buildResponse.data,
      'App Store Connect builds',
    );
    if (builds.length > 1) {
      throw new Error(
        'App Store Connect returned multiple builds for the exact identity',
      );
    }
    if (builds.length === 0) {
      if (attempt + 1 < APP_STORE_BUILD_POLL_ATTEMPTS) {
        await sleep(Math.min(1000 * 2 ** attempt, 15_000));
        continue;
      }
      throw new ProviderReadBackTimeout('App Store Connect');
    }
    const build = builds[0]!;
    const buildAttributes =
      build.attributes &&
      typeof build.attributes === 'object' &&
      !Array.isArray(build.attributes)
        ? (build.attributes as Record<string, unknown>)
        : undefined;
    const relationships =
      build.relationships &&
      typeof build.relationships === 'object' &&
      !Array.isArray(build.relationships)
        ? (build.relationships as Record<string, unknown>)
        : undefined;
    const prereleaseRelationship =
      relationships?.preReleaseVersion &&
      typeof relationships.preReleaseVersion === 'object' &&
      !Array.isArray(relationships.preReleaseVersion)
        ? (relationships.preReleaseVersion as Record<string, unknown>)
        : undefined;
    const prereleaseData =
      prereleaseRelationship?.data &&
      typeof prereleaseRelationship.data === 'object' &&
      !Array.isArray(prereleaseRelationship.data)
        ? (prereleaseRelationship.data as Record<string, unknown>)
        : undefined;
    const processingState = buildAttributes?.processingState;
    if (
      build.type !== 'builds' ||
      typeof build.id !== 'string' ||
      !/^[A-Za-z0-9._-]+$/.test(build.id) ||
      buildAttributes?.version !== input.expectedBuildNumber ||
      (processingState !== 'PROCESSING' && processingState !== 'VALID') ||
      prereleaseData?.type !== 'preReleaseVersions' ||
      typeof prereleaseData.id !== 'string'
    ) {
      throw new Error(
        'App Store Connect build identity or processing state does not match',
      );
    }
    const included = objectArray(
      buildResponse.included,
      'App Store Connect included resources',
    );
    const prerelease = included.filter(
      (resource) =>
        resource.type === 'preReleaseVersions' &&
        resource.id === prereleaseData.id,
    );
    const prereleaseAttributes =
      prerelease.length === 1 &&
      prerelease[0]!.attributes &&
      typeof prerelease[0]!.attributes === 'object' &&
      !Array.isArray(prerelease[0]!.attributes)
        ? (prerelease[0]!.attributes as Record<string, unknown>)
        : undefined;
    if (prereleaseAttributes?.version !== input.expectedVersion) {
      throw new Error('App Store Connect pre-release version does not match');
    }
    if (processingState === 'PROCESSING') {
      if (attempt + 1 < APP_STORE_BUILD_POLL_ATTEMPTS) {
        await sleep(Math.min(1000 * 2 ** attempt, 15_000));
        continue;
      }
      throw new ProviderReadBackTimeout('App Store Connect processing');
    }

    const groupRelationshipEndpoint = new URL(
      `${APP_STORE_CONNECT_ENDPOINT}/betaGroups/${encodeURIComponent(input.expectedBetaGroupId)}/relationships/builds`,
    );
    const readGroupBuilds = async (): Promise<string[]> => {
      const firstUrl = new URL(groupRelationshipEndpoint);
      firstUrl.searchParams.set('limit', '200');
      let nextUrl: URL | undefined = firstUrl;
      const visited = new Set<string>();
      const ids: string[] = [];
      for (
        let page = 0;
        nextUrl && page < APP_STORE_GROUP_PAGE_LIMIT;
        page += 1
      ) {
        const requestUrl: URL = nextUrl;
        const queryKeys = [...requestUrl.searchParams.keys()];
        const limitValues = requestUrl.searchParams.getAll('limit');
        const cursorValues = requestUrl.searchParams.getAll('cursor');
        if (
          requestUrl.origin !== new URL(APP_STORE_CONNECT_ENDPOINT).origin ||
          requestUrl.pathname !== groupRelationshipEndpoint.pathname ||
          requestUrl.username !== '' ||
          requestUrl.password !== '' ||
          requestUrl.hash !== '' ||
          visited.has(requestUrl.href) ||
          queryKeys.some((key) => key !== 'limit' && key !== 'cursor') ||
          limitValues.length !== 1 ||
          limitValues[0] !== '200' ||
          cursorValues.length > 1 ||
          (page === 0 && cursorValues.length !== 0) ||
          (page > 0 &&
            (cursorValues.length !== 1 ||
              !/^[A-Za-z0-9._~+/=-]{1,1024}$/.test(cursorValues[0]!)))
        ) {
          throw new Error(
            'App Store Connect beta-group pagination link is not trusted',
          );
        }
        visited.add(requestUrl.href);
        const relationship = await responseJson(
          await fetcher(requestUrl, {
            method: 'GET',
            headers,
            signal: AbortSignal.timeout(120_000),
          }),
          'App Store Connect beta-group build read-back',
        );
        ids.push(
          ...objectArray(
            relationship.data,
            'App Store Connect beta-group build relationships',
          ).map((resource) => {
            if (
              resource.type !== 'builds' ||
              typeof resource.id !== 'string' ||
              !/^[A-Za-z0-9._-]+$/.test(resource.id)
            ) {
              throw new Error(
                'App Store Connect beta group returned an invalid build identity',
              );
            }
            return resource.id;
          }),
        );
        const links =
          relationship.links &&
          typeof relationship.links === 'object' &&
          !Array.isArray(relationship.links)
            ? (relationship.links as Record<string, unknown>)
            : undefined;
        const next = links?.next;
        if (next === undefined || next === null) {
          nextUrl = undefined;
        } else if (typeof next === 'string') {
          nextUrl = new URL(next);
        } else {
          throw new Error(
            'App Store Connect beta-group pagination link is malformed',
          );
        }
      }
      if (nextUrl) {
        throw new Error(
          'App Store Connect beta-group read-back exceeded the page limit',
        );
      }
      return ids;
    };
    const beforeGroup = await readGroupBuilds();
    if (!beforeGroup.includes(build.id)) {
      await responseJson(
        await fetcher(groupRelationshipEndpoint, {
          method: 'POST',
          headers: {
            ...headers,
            'content-type': 'application/json',
          },
          body: JSON.stringify({
            data: [{ type: 'builds', id: build.id }],
          }),
          signal: AbortSignal.timeout(120_000),
        }),
        'App Store Connect beta-group build association',
      );
    }
    const afterGroup = await readGroupBuilds();
    if (afterGroup.filter((id) => id === build.id).length !== 1) {
      throw new Error(
        'App Store Connect beta-group read-back does not contain the exact build',
      );
    }
    return {
      appId: app.id,
      buildId: build.id,
      preReleaseVersionId: prereleaseData.id,
      processingState,
      betaGroupId: input.expectedBetaGroupId,
      verified: true,
    };
  }
  throw new ProviderReadBackTimeout('App Store Connect');
}

/**
 * Uploads one signed AAB into an existing custom closed-testing track and
 * commits the edit. Provider failures delete the uncommitted edit best-effort.
 */
export async function uploadGooglePlayClosedBeta(
  input: GooglePlayUpload,
  fetcher: typeof fetch = fetch,
  nowMs = Date.now(),
): Promise<GooglePlayReceipt> {
  if (input.packageId !== MOBILE_PRODUCTION_PACKAGE_ID) {
    throw new Error('Google Play package identifier is not the production app');
  }
  if (
    !/^[a-z][a-z0-9-]{2,63}$/.test(input.closedTrack) ||
    RESERVED_PLAY_TRACKS.has(input.closedTrack)
  ) {
    throw new Error('Google Play closed track is missing, reserved, or malformed');
  }
  if (!/^[1-9][0-9]*$/.test(input.versionCode)) {
    throw new Error('Google Play version code must be a positive decimal integer');
  }
  if (
    input.releaseName.length === 0 ||
    input.releaseName.length > 120 ||
    /[\u0000-\u001f\u007f]/.test(input.releaseName)
  ) {
    throw new Error('Google Play release name is invalid');
  }
  if (!input.artifactPath.toLowerCase().endsWith('.aab')) {
    throw new Error('Google Play upload requires an Android App Bundle');
  }
  const account = serviceAccount(
    input.serviceAccountJson,
    input.expectedServiceAccountEmail,
  );
  const artifact = await openArtifact(
    input.artifactPath,
    input.expectedBytes,
    input.expectedSha256,
    MAX_ANDROID_BUNDLE_BYTES,
  );
  try {
    const tokenResponse = await responseJson(
      await fetcher(GOOGLE_TOKEN_ENDPOINT, {
        method: 'POST',
        headers: { 'content-type': 'application/x-www-form-urlencoded' },
        body: new URLSearchParams({
          grant_type: 'urn:ietf:params:oauth:grant-type:jwt-bearer',
          assertion: assertion(account, nowMs),
        }),
        signal: AbortSignal.timeout(120_000),
      }),
      'Google OAuth token exchange',
    );
    const token = tokenResponse.access_token;
    if (typeof token !== 'string' || token.length < 5) {
      throw new Error('Google OAuth token exchange returned no access token');
    }

    const packagePath = encodeURIComponent(input.packageId);
    const editsUrl = `${GOOGLE_PUBLISHER_ENDPOINT}/applications/${packagePath}/edits`;
    const edit = await responseJson(
      await fetcher(editsUrl, {
        method: 'POST',
        headers: publisherHeaders(token),
        body: '{}',
        signal: AbortSignal.timeout(120_000),
      }),
      'Google Play edit creation',
    );
    const editId = edit.id;
    if (typeof editId !== 'string' || !/^[A-Za-z0-9._-]+$/.test(editId)) {
      throw new Error('Google Play edit creation returned an invalid edit identifier');
    }

    const editPath = `${editsUrl}/${encodeURIComponent(editId)}`;
    let commitConfirmed = false;
    try {
      const trackPath = `${editPath}/tracks/${encodeURIComponent(input.closedTrack)}`;
      const track = await responseJson(
        await fetcher(trackPath, {
          method: 'GET',
          headers: publisherHeaders(token),
          signal: AbortSignal.timeout(120_000),
        }),
        'Google Play closed-track lookup',
      );
      if (track.track !== input.closedTrack) {
        throw new Error('Google Play did not return the reviewed closed track');
      }

      const bundle = await readOpenedAndroidBundle(artifact);
      const uploadBody = bundle.buffer.slice(
        bundle.byteOffset,
        bundle.byteOffset + bundle.byteLength,
      ) as ArrayBuffer;
      const uploaded = await responseJson(
        await fetcher(
          `${GOOGLE_UPLOAD_ENDPOINT}/applications/${packagePath}/edits/${encodeURIComponent(editId)}/bundles?uploadType=media`,
          {
            method: 'POST',
            headers: {
              authorization: `Bearer ${token}`,
              'content-type': 'application/octet-stream',
              'content-length': String(bundle.byteLength),
            },
            body: uploadBody,
            signal: AbortSignal.timeout(120_000),
          },
        ),
        'Google Play App Bundle upload',
      );
      requireEqualVersionCode(uploaded.versionCode, input.versionCode);

      await responseJson(
        await fetcher(trackPath, {
          method: 'PUT',
          headers: publisherHeaders(token),
          body: JSON.stringify({
            track: input.closedTrack,
            releases: [
              {
                name: input.releaseName,
                versionCodes: [input.versionCode],
                status: 'completed',
              },
            ],
          }),
          signal: AbortSignal.timeout(120_000),
        }),
        'Google Play closed-track update',
      );
      await responseJson(
        await fetcher(`${editPath}:commit`, {
          method: 'POST',
          headers: publisherHeaders(token),
          body: '{}',
          signal: AbortSignal.timeout(120_000),
        }),
        'Google Play edit commit',
      );
      commitConfirmed = true;
    } catch (error) {
      if (!commitConfirmed) {
        try {
          await fetcher(editPath, {
            method: 'DELETE',
            headers: { authorization: `Bearer ${token}` },
            signal: AbortSignal.timeout(120_000),
          });
        } catch {
          // The original provider error remains authoritative.
        }
      }
      throw error;
    }

    const readBackEdit = await responseJson(
      await fetcher(editsUrl, {
        method: 'POST',
        headers: publisherHeaders(token),
        body: '{}',
        signal: AbortSignal.timeout(120_000),
      }),
      'Google Play read-back edit creation',
    );
    const readBackEditId = readBackEdit.id;
    if (
      typeof readBackEditId !== 'string' ||
      !/^[A-Za-z0-9._-]+$/.test(readBackEditId) ||
      readBackEditId === editId
    ) {
      throw new Error('Google Play returned an invalid fresh read-back edit');
    }
    const readBackPath = `${editsUrl}/${encodeURIComponent(readBackEditId)}`;
    try {
      const readBackTrack = await responseJson(
        await fetcher(
          `${readBackPath}/tracks/${encodeURIComponent(input.closedTrack)}`,
          {
            method: 'GET',
            headers: publisherHeaders(token),
            signal: AbortSignal.timeout(120_000),
          },
        ),
        'Google Play closed-track read-back',
      );
      const releases = objectArray(
        readBackTrack.releases,
        'Google Play closed-track read-back releases',
      );
      const versionCodes =
        releases.length === 1 && Array.isArray(releases[0]!.versionCodes)
          ? releases[0]!.versionCodes
          : undefined;
      if (
        readBackTrack.track !== input.closedTrack ||
        releases.length !== 1 ||
        releases[0]!.name !== input.releaseName ||
        releases[0]!.status !== 'completed' ||
        !versionCodes ||
        versionCodes.length !== 1 ||
        String(versionCodes[0]) !== input.versionCode
      ) {
        throw new Error(
          'Google Play closed-track read-back does not match the uploaded release',
        );
      }
    } finally {
      const cleanup = await fetcher(readBackPath, {
        method: 'DELETE',
        headers: { authorization: `Bearer ${token}` },
        signal: AbortSignal.timeout(120_000),
      });
      if (!cleanup.ok) {
        throw new Error('Google Play fresh read-back edit cleanup failed');
      }
    }
    return {
      editId,
      packageId: MOBILE_PRODUCTION_PACKAGE_ID,
      track: input.closedTrack,
      versionCode: input.versionCode,
      committed: true,
      providerVerified: true,
    };
  } finally {
    await artifact.handle.close();
  }
}

function requireEqualVersionCode(actual: unknown, expected: string): void {
  if (String(actual) !== expected) {
    throw new Error('Google Play uploaded version code does not match the package');
  }
}
