export interface ReleaseManifest {
  schemaVersion: 1;
  generatedAt: string;
  sourceCommit: string;
  updaterPublicKeySha256: string;
  releases: ReleaseManifestRelease[];
}

export interface ReleaseManifestRelease {
  channel: 'stable' | 'beta';
  current: boolean;
  version: string;
  artifacts: ReleaseManifestArtifact[];
}

export type ReleasePlatform = 'macos' | 'windows' | 'linux';
export type ReleaseArchitecture = 'universal' | 'x86_64';
export type ReleaseArtifactKind =
  | 'dmg'
  | 'nsis'
  | 'appimage'
  | 'deb'
  | 'rpm'
  | 'updater';

export interface ReleaseManifestArtifact {
  channel: 'stable' | 'beta';
  version: string;
  platform: ReleasePlatform;
  architecture: ReleaseArchitecture;
  kind: ReleaseArtifactKind;
  url: string;
  bytes: number;
  sha256: string;
  minimumOs: string;
  signature: string | null;
  sbomUrl: string;
  sbomSha256: string;
  publishedAt: string;
  sourceCommit: string;
  updaterPublicKeySha256: string;
  releaseConfigSha256: string;
}

const STABLE_SEMVER =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const SOURCE_SHA = /^[0-9a-f]{40}$/;
const SHA256 = /^[0-9a-f]{64}$/;
const DETACHED_SIGNATURE = /^[A-Za-z0-9+/]+={0,2}$/;
const CHANNELS = new Set(['stable', 'beta']);
const PLATFORMS = new Set(['macos', 'windows', 'linux']);
const ARCHITECTURES = new Set(['universal', 'x86_64']);
const KINDS = new Set(['dmg', 'nsis', 'appimage', 'deb', 'rpm', 'updater']);
const REQUIRED_MATRIX = [
  'macos:dmg',
  'macos:updater',
  'windows:nsis',
  'windows:updater',
  'linux:appimage',
  'linux:deb',
  'linux:rpm',
  'linux:updater',
] as const;
const RELEASE_ORIGIN = 'https://downloads.mdreader.dev';

function record(value: unknown, path: string): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error(`${path} must be an object`);
  }
  return value as Record<string, unknown>;
}

function exactKeys(
  value: Record<string, unknown>,
  expected: readonly string[],
  path: string,
): void {
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  const missing = wanted.filter((key) => !(key in value));
  if (missing.length > 0) {
    throw new Error(`${path}.${missing[0]} is required`);
  }
  if (
    actual.length !== wanted.length ||
    actual.some((key) => !wanted.includes(key))
  ) {
    throw new Error(`${path} contains unknown fields`);
  }
}

function string(value: unknown, path: string): string {
  if (typeof value !== 'string' || value.length === 0) {
    throw new Error(`${path} must be a non-empty string`);
  }
  return value;
}

function timestamp(value: unknown, path: string): string {
  const parsed = string(value, path);
  if (!parsed.endsWith('Z') || Number.isNaN(Date.parse(parsed))) {
    throw new Error(`${path} must be an ISO-8601 UTC timestamp`);
  }
  return parsed;
}

function semanticVersion(value: unknown, path: string): string {
  const parsed = string(value, path);
  if (!STABLE_SEMVER.test(parsed)) {
    throw new Error(`${path} must be a stable semantic version`);
  }
  return parsed;
}

function sourceSha(value: unknown, path: string): string {
  const parsed = string(value, path);
  if (!SOURCE_SHA.test(parsed)) throw new Error(`${path} must be an exact 40-character source commit`);
  return parsed;
}

function sha256(value: unknown, path: string): string {
  const parsed = string(value, path);
  if (!SHA256.test(parsed)) throw new Error(`${path} must be a lowercase SHA-256`);
  return parsed;
}

function signature(value: unknown, path: string, required: boolean): string | null {
  if (value === null && !required) return null;
  if (typeof value !== 'string' || value.length < 64 || value.length % 4 !== 0 || !DETACHED_SIGNATURE.test(value)) {
    throw new Error(`${path} must be a valid detached updater signature`);
  }
  return value;
}

function immutableUrl(
  value: unknown,
  path: string,
  version: string,
  expectedFilename: string,
): string {
  const parsed = string(value, path);
  let url: URL;
  try {
    url = new URL(parsed);
  } catch {
    throw new Error(`${path} must be a valid immutable HTTPS URL`);
  }
  if (
    url.protocol !== 'https:' ||
    url.origin !== RELEASE_ORIGIN ||
    url.username !== '' ||
    url.password !== '' ||
    url.search !== '' ||
    url.hash !== ''
  ) {
    throw new Error(
      `${path} must use ${RELEASE_ORIGIN} as an immutable HTTPS URL without credentials, query, or fragment`,
    );
  }
  const expectedPath = `/releases/v${version}/${expectedFilename}`;
  const canonical = `${RELEASE_ORIGIN}${expectedPath}`;
  if (parsed !== canonical) {
    throw new Error(
      `${path} must be the exact canonical filename URL ${canonical}`,
    );
  }
  if (url.pathname !== expectedPath) {
    throw new Error(
      `${path} must use canonical filename ${expectedFilename} at ${expectedPath}`,
    );
  }
  return canonical;
}

function canonicalFilename(
  platform: ReleasePlatform,
  kind: ReleaseArtifactKind,
  version: string,
): string {
  if (platform === 'macos') {
    return kind === 'dmg'
      ? `MDReader-${version}-universal.dmg`
      : `MDReader-${version}-universal.app.tar.gz`;
  }
  if (platform === 'windows') {
    return kind === 'nsis'
      ? `MDReader-${version}-x64-setup.exe`
      : `MDReader-${version}-x64-setup.nsis.zip`;
  }
  const suffix: Record<'appimage' | 'deb' | 'rpm' | 'updater', string> = {
    appimage: 'AppImage',
    deb: 'deb',
    rpm: 'rpm',
    updater: 'AppImage.tar.gz',
  };
  return `MDReader-${version}-x86_64.${suffix[kind as keyof typeof suffix]}`;
}

function enumValue<T extends string>(
  value: unknown,
  path: string,
  accepted: ReadonlySet<string>,
): T {
  const parsed = string(value, path);
  if (!accepted.has(parsed)) throw new Error(`${path} is unsupported`);
  return parsed as T;
}

function parseArtifact(
  value: unknown,
  release: Pick<ReleaseManifestRelease, 'channel' | 'version'>,
  manifestSource: string,
  manifestUpdaterPublicKeySha256: string,
  index: number,
): ReleaseManifestArtifact {
  const path = `releases[].artifacts[${index}]`;
  const input = record(value, path);
  exactKeys(
    input,
    [
      'channel',
      'version',
      'platform',
      'architecture',
      'kind',
      'url',
      'bytes',
      'sha256',
      'minimumOs',
      'signature',
      'sbomUrl',
      'sbomSha256',
      'publishedAt',
      'sourceCommit',
      'updaterPublicKeySha256',
      'releaseConfigSha256',
    ],
    path,
  );
  const channel = enumValue<'stable' | 'beta'>(input.channel, `${path}.channel`, CHANNELS);
  const version = semanticVersion(input.version, `${path}.version`);
  if (channel !== release.channel) throw new Error(`${path}.channel must match its release`);
  if (version !== release.version) throw new Error(`${path}.version must match its release`);

  const platform = enumValue<ReleasePlatform>(input.platform, `${path}.platform`, PLATFORMS);
  const architecture = enumValue<ReleaseArchitecture>(
    input.architecture,
    `${path}.architecture`,
    ARCHITECTURES,
  );
  if ((platform === 'macos') !== (architecture === 'universal')) {
    throw new Error(`${path}.architecture does not match ${platform}`);
  }
  const kind = enumValue<ReleaseArtifactKind>(input.kind, `${path}.kind`, KINDS);
  const allowed =
    kind === 'updater' ||
    (platform === 'macos' && kind === 'dmg') ||
    (platform === 'windows' && kind === 'nsis') ||
    (platform === 'linux' && ['appimage', 'deb', 'rpm'].includes(kind));
  if (!allowed) throw new Error(`${path}.kind does not match ${platform}`);

  const expectedFilename = canonicalFilename(platform, kind, version);
  const url = immutableUrl(input.url, `${path}.url`, version, expectedFilename);
  const bytes = input.bytes;
  if (!Number.isSafeInteger(bytes) || (bytes as number) <= 0) {
    throw new Error(`${path}.bytes must be a positive exact integer`);
  }
  const minimumOs = string(input.minimumOs, `${path}.minimumOs`);
  const expectedMinimumOs: Record<ReleasePlatform, string> = {
    macos: 'macOS 12.0',
    windows: 'Windows 10',
    linux: 'glibc 2.31',
  };
  if (minimumOs !== expectedMinimumOs[platform]) {
    throw new Error(
      `${path}.minimumOs must be exactly ${expectedMinimumOs[platform]}`,
    );
  }
  const sbomUrl = immutableUrl(
    input.sbomUrl,
    `${path}.sbomUrl`,
    version,
    `${expectedFilename}.spdx.json`,
  );
  const artifactSource = sourceSha(input.sourceCommit, `${path}.sourceCommit`);
  if (artifactSource !== manifestSource) {
    throw new Error(`${path} source commit does not match the manifest source commit`);
  }
  const artifactUpdaterPublicKeySha256 = sha256(
    input.updaterPublicKeySha256,
    `${path}.updaterPublicKeySha256`,
  );
  if (artifactUpdaterPublicKeySha256 !== manifestUpdaterPublicKeySha256) {
    throw new Error(
      `${path} updater key fingerprint does not match the manifest`,
    );
  }

  return {
    channel,
    version,
    platform,
    architecture,
    kind,
    url,
    bytes: bytes as number,
    sha256: sha256(input.sha256, `${path}.sha256`),
    minimumOs,
    signature: signature(input.signature, `${path}.signature`, kind === 'updater'),
    sbomUrl,
    sbomSha256: sha256(input.sbomSha256, `${path}.sbomSha256`),
    publishedAt: timestamp(input.publishedAt, `${path}.publishedAt`),
    sourceCommit: artifactSource,
    updaterPublicKeySha256: artifactUpdaterPublicKeySha256,
    releaseConfigSha256: sha256(
      input.releaseConfigSha256,
      `${path}.releaseConfigSha256`,
    ),
  };
}

function parseRelease(
  value: unknown,
  manifestSource: string,
  manifestUpdaterPublicKeySha256: string,
  index: number,
): ReleaseManifestRelease {
  const path = `releases[${index}]`;
  const input = record(value, path);
  exactKeys(input, ['channel', 'current', 'version', 'artifacts'], path);
  const channel = enumValue<'stable' | 'beta'>(input.channel, `${path}.channel`, CHANNELS);
  if (typeof input.current !== 'boolean') throw new Error(`${path}.current must be boolean`);
  const version = semanticVersion(input.version, `${path}.version`);
  if (!Array.isArray(input.artifacts)) throw new Error(`${path}.artifacts must be an array`);
  const artifacts = input.artifacts.map((artifact, artifactIndex) =>
    parseArtifact(
      artifact,
      { channel, version },
      manifestSource,
      manifestUpdaterPublicKeySha256,
      artifactIndex,
    ),
  );

  const entries = new Set<string>();
  for (const artifact of artifacts) {
    const key = `${artifact.platform}:${artifact.kind}`;
    if (entries.has(key)) throw new Error(`${path} contains duplicate ${key} artifacts`);
    entries.add(key);
  }
  for (const required of REQUIRED_MATRIX) {
    if (!entries.has(required)) throw new Error(`${path} is missing required artifact ${required}`);
  }

  return {
    channel,
    current: input.current,
    version,
    artifacts,
  };
}

/**
 * Parse untrusted JSON at the release boundary. A TypeScript assertion is not
 * enough: every URL and byte exposed by the download page comes through here.
 */
export function parseReleaseManifest(input: unknown): ReleaseManifest {
  const root = record(input, 'manifest');
  exactKeys(
    root,
    [
      'schemaVersion',
      'generatedAt',
      'sourceCommit',
      'updaterPublicKeySha256',
      'releases',
    ],
    'manifest',
  );
  if (root.schemaVersion !== 1) throw new Error('manifest.schemaVersion must be 1');
  const generatedAt = timestamp(root.generatedAt, 'manifest.generatedAt');
  const sourceCommit = sourceSha(root.sourceCommit, 'manifest.sourceCommit');
  const updaterPublicKeySha256 = sha256(
    root.updaterPublicKeySha256,
    'manifest.updaterPublicKeySha256',
  );
  if (!Array.isArray(root.releases) || root.releases.length === 0) {
    throw new Error('manifest.releases must be a non-empty array');
  }
  const releases = root.releases.map((release, index) =>
    parseRelease(release, sourceCommit, updaterPublicKeySha256, index),
  );
  const currentStable = releases.filter(
    (release) => release.channel === 'stable' && release.current,
  );
  if (currentStable.length !== 1) {
    throw new Error('manifest must contain exactly one current stable release');
  }
  if (releases.filter((release) => release.channel === 'beta' && release.current).length > 1) {
    throw new Error('manifest may contain at most one current beta release');
  }
  return {
    schemaVersion: 1,
    generatedAt,
    sourceCommit,
    updaterPublicKeySha256,
    releases,
  };
}
