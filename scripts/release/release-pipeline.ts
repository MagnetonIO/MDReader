import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { existsSync } from 'node:fs';
import { readFile, stat } from 'node:fs/promises';
import { basename, dirname, join, resolve } from 'node:path';
import { promisify } from 'node:util';

import {
  parseReleaseManifest,
  type ReleaseManifest,
  type ReleaseManifestArtifact,
} from './release-manifest';
import { assertCompleteArtifactSbom } from './stage-release';
import { updaterPublicKeyFingerprint } from './release-config';

const VERIFIED_RELEASE = Symbol('verified-release');
const execFileAsync = promisify(execFile);

function repositoryRoot(): string {
  let candidate = resolve(process.cwd());
  for (;;) {
    if (existsSync(join(candidate, 'scripts', 'release', 'verifier', 'Cargo.toml'))) {
      return candidate;
    }
    const parent = dirname(candidate);
    if (parent === candidate) {
      throw new Error('cannot locate repository root for release verifier');
    }
    candidate = parent;
  }
}

export type PublicationKind =
  | 'artifact'
  | 'sbom'
  | 'signature'
  | 'manifest'
  | 'updater-manifest'
  | 'pointer';

export interface PublicationEntry {
  kind: PublicationKind;
  key: string;
  immutable: boolean;
  localPath: string | null;
  contents: string | null;
}

interface VerifiedRelease {
  readonly [VERIFIED_RELEASE]: true;
  readonly manifest: ReleaseManifest;
  readonly root: string;
}

function urlFilename(url: string): string {
  const decoded = decodeURIComponent(new URL(url).pathname.split('/').pop() ?? '');
  if (decoded === '' || basename(decoded) !== decoded) {
    throw new Error(`release URL does not name one local file: ${url}`);
  }
  return decoded;
}

function objectKey(url: string): string {
  return decodeURIComponent(new URL(url).pathname).replace(/^\/+/, '');
}

async function sha256File(path: string): Promise<string> {
  return createHash('sha256').update(await readFile(path)).digest('hex');
}

async function assertFile(
  path: string,
  label: string,
  bytes: number | null,
  checksum: string,
): Promise<void> {
  let details;
  try {
    details = await stat(path);
  } catch {
    throw new Error(`${label} is missing: ${path}`);
  }
  if (!details.isFile()) throw new Error(`${label} is not a regular file: ${path}`);
  if (bytes !== null && details.size !== bytes) {
    throw new Error(`${label} size mismatch: expected ${bytes}, found ${details.size}`);
  }
  const actual = await sha256File(path);
  if (actual !== checksum) {
    throw new Error(`${label} SHA-256 mismatch: expected ${checksum}, found ${actual}`);
  }
}

async function assertSbom(
  path: string,
  checksum: string,
  artifactFilename: string,
  artifactChecksum: string,
): Promise<void> {
  await assertFile(path, 'SBOM', null, checksum);
  let body: unknown;
  try {
    body = JSON.parse(await readFile(path, 'utf8'));
  } catch {
    throw new Error(`SBOM is not valid JSON: ${path}`);
  }
  if (typeof body !== 'object' || body === null) throw new Error(`SBOM has no document: ${path}`);
  const object = body as Record<string, unknown>;
  if (object.spdxVersion !== 'SPDX-2.3') {
    throw new Error(`SBOM must be SPDX 2.3: ${path}`);
  }
  assertCompleteArtifactSbom(body, artifactFilename, artifactChecksum);
}

export async function verifyUpdaterSignature(
  artifactPath: string,
  signature: string,
  publicKey: string,
): Promise<void> {
  try {
    await execFileAsync(
      'cargo',
      [
        'run',
        '--quiet',
        '--locked',
        '-p',
        'mdreader-release-verifier',
        '--',
        artifactPath,
        signature,
        publicKey,
      ],
      {
        cwd: repositoryRoot(),
        maxBuffer: 1024 * 1024,
      },
    );
  } catch (error) {
    const stderr =
      typeof error === 'object' &&
      error !== null &&
      'stderr' in error &&
      (typeof error.stderr === 'string' || Buffer.isBuffer(error.stderr))
        ? String(error.stderr).trim()
        : '';
    const detail = stderr || (error instanceof Error ? error.message : String(error));
    throw new Error(
      `updater signature verification failed: ${basename(artifactPath)}: ${detail}`,
    );
  }
}

function versionDirectory(root: string, artifact: ReleaseManifestArtifact): string {
  return join(root, 'releases', `v${artifact.version}`);
}

/**
 * Verify the complete local release set before minting the capability accepted
 * by `publicationPlan`. No upload command can be derived from unverified JSON.
 */
export async function validateReleaseDirectory(
  input: unknown,
  root: string,
  expectedSourceCommit: string,
  updaterPublicKey: string,
): Promise<VerifiedRelease> {
  const manifest = parseReleaseManifest(input);
  if (manifest.sourceCommit !== expectedSourceCommit) {
    throw new Error(
      `manifest source commit ${manifest.sourceCommit} does not match ${expectedSourceCommit}`,
    );
  }
  if (
    manifest.updaterPublicKeySha256 !==
    updaterPublicKeyFingerprint(updaterPublicKey)
  ) {
    throw new Error(
      'manifest embedded updater key fingerprint does not match the configured updater key',
    );
  }

  for (const release of manifest.releases) {
    for (const artifact of release.artifacts) {
      const directory = versionDirectory(root, artifact);
      const artifactPath = join(directory, urlFilename(artifact.url));
      const sbomPath = join(directory, urlFilename(artifact.sbomUrl));
      await assertFile(artifactPath, `${artifact.platform} ${artifact.kind} artifact`, artifact.bytes, artifact.sha256);
      await assertSbom(
        sbomPath,
        artifact.sbomSha256,
        urlFilename(artifact.url),
        artifact.sha256,
      );
      if (artifact.kind === 'updater') {
        const signaturePath = `${artifactPath}.sig`;
        let detached;
        try {
          detached = (await readFile(signaturePath, 'utf8')).trim();
        } catch {
          throw new Error(`updater signature is missing: ${signaturePath}`);
        }
        if (detached !== artifact.signature) {
          throw new Error(`updater signature does not match manifest: ${signaturePath}`);
        }
        await verifyUpdaterSignature(
          artifactPath,
          detached,
          updaterPublicKey,
        );
      }
    }
  }

  return { [VERIFIED_RELEASE]: true, manifest, root };
}

/**
 * Produce the only allowed publication order. Versioned artifacts, SBOMs, and
 * signatures come first; the immutable manifest follows; the stable manifest
 * pointer advances next; the updater pointer is the final commit point.
 */
export function publicationPlan(verified: VerifiedRelease): PublicationEntry[] {
  if (verified[VERIFIED_RELEASE] !== true) {
    throw new Error('publication requires a verified release directory');
  }
  const { manifest, root } = verified;
  const entries: PublicationEntry[] = [];
  const emitted = new Set<string>();
  const add = (entry: PublicationEntry) => {
    if (emitted.has(entry.key)) return;
    emitted.add(entry.key);
    entries.push(entry);
  };

  for (const release of manifest.releases) {
    for (const artifact of release.artifacts) {
      const directory = versionDirectory(root, artifact);
      add({
        kind: 'artifact',
        key: objectKey(artifact.url),
        immutable: true,
        localPath: join(directory, urlFilename(artifact.url)),
        contents: null,
      });
      add({
        kind: 'sbom',
        key: objectKey(artifact.sbomUrl),
        immutable: true,
        localPath: join(directory, urlFilename(artifact.sbomUrl)),
        contents: null,
      });
      if (artifact.kind === 'updater') {
        add({
          kind: 'signature',
          key: `${objectKey(artifact.url)}.sig`,
          immutable: true,
          localPath: `${join(directory, urlFilename(artifact.url))}.sig`,
          contents: null,
        });
      }
    }
  }

  const stable = manifest.releases.find(
    (release) => release.channel === 'stable' && release.current,
  )!;
  const manifestKey = `releases/v${stable.version}/manifest.json`;
  const manifestContents = `${JSON.stringify(manifest, null, 2)}\n`;
  const manifestChecksum = createHash('sha256').update(manifestContents).digest('hex');
  add({
    kind: 'manifest',
    key: manifestKey,
    immutable: true,
    localPath: null,
    contents: manifestContents,
  });
  const updaterEntries: Array<
    [string, { signature: string; url: string }]
  > = [];
  for (const artifact of stable.artifacts.filter(
    (entry) => entry.kind === 'updater',
  )) {
    if (artifact.signature === null) {
      throw new Error(`verified updater ${artifact.platform} has no signature`);
    }
    const value = {
      signature: artifact.signature,
      url: artifact.url,
    };
    if (artifact.platform === 'macos') {
      // Tauri selects updater entries by the running architecture. Both
      // architectures consume the same universal macOS archive.
      updaterEntries.push(
        ['darwin-aarch64', value],
        ['darwin-x86_64', value],
      );
    } else {
      updaterEntries.push([
        artifact.platform === 'windows'
          ? 'windows-x86_64'
          : 'linux-x86_64',
        value,
      ]);
    }
  }
  const updaterPlatforms = Object.fromEntries(updaterEntries);
  add({
    kind: 'pointer',
    key: 'channels/stable/manifest.json',
    immutable: false,
    localPath: null,
    contents: `${JSON.stringify(
      {
        schemaVersion: 1,
        version: stable.version,
        manifestUrl: `https://downloads.mdreader.dev/${manifestKey}`,
        manifestSha256: manifestChecksum,
        sourceCommit: manifest.sourceCommit,
      },
      null,
      2,
    )}\n`,
  });
  add({
    kind: 'updater-manifest',
    key: 'channels/stable/updater.json',
    immutable: false,
    localPath: null,
    contents: `${JSON.stringify(
      {
        version: stable.version,
        notes: '',
        pub_date: stable.artifacts[0]!.publishedAt,
        platforms: updaterPlatforms,
      },
      null,
      2,
    )}\n`,
  });
  return entries;
}
