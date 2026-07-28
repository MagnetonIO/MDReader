import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import {
  lstat,
  mkdtemp,
  readFile,
  readdir,
  realpath,
  rm,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join, posix, relative, resolve, sep } from 'node:path';

import {
  MOBILE_PRODUCTION_PACKAGE_ID,
  type MobileBetaArtifactEvidence,
} from './mobile-beta-manifest';

const MAX_ZIP_ENTRIES = 50_000;
const MAX_ZIP_ENTRY_COMPRESSED_BYTES = 512 * 1024 * 1024;
const MAX_ZIP_ENTRY_UNCOMPRESSED_BYTES = 768 * 1024 * 1024;
const MAX_ZIP_COMPRESSED_BYTES = 768 * 1024 * 1024;
const MAX_ZIP_UNCOMPRESSED_BYTES = 1536 * 1024 * 1024;
const MAX_ZIP_SYMLINK_TARGET_BYTES = 4096;
const DEFAULT_INSPECTION_TOOL_TIMEOUT_MS = 5 * 60 * 1000;

interface ZipEntry {
  path: string;
  kind: 'file' | 'directory' | 'symlink';
  uncompressedBytes: number;
}

export type MobileBetaToolRunner = (
  command: string,
  args: readonly string[],
) => string;

interface IosExpected {
  sourceCommit: string;
  version: string;
  buildNumber: string;
  teamId: string;
  certificateSha256: string;
  provisioningProfileSha256: string;
}

interface AndroidExpected {
  sourceCommit: string;
  version: string;
  buildNumber: string;
  certificateSha256: string;
}

interface MobileBetaInspectionOptions {
  toolTimeoutMs?: number;
  artifactDescriptor?: number;
}

function isTimeout(error: Error | undefined): boolean {
  return (error as NodeJS.ErrnoException | undefined)?.code === 'ETIMEDOUT';
}

export function createMobileBetaInspectionToolRunner(
  timeoutMs: number,
  artifactDescriptor?: number,
): MobileBetaToolRunner {
  if (!Number.isSafeInteger(timeoutMs) || timeoutMs <= 0) {
    throw new Error('mobile package inspection timeout is invalid');
  }
  if (
    artifactDescriptor !== undefined &&
    (!Number.isSafeInteger(artifactDescriptor) ||
      artifactDescriptor <= 2 ||
      artifactDescriptor > 1024)
  ) {
    throw new Error('mobile package inspection descriptor is invalid');
  }
  return (command, args) =>
    runTool(command, args, timeoutMs, artifactDescriptor);
}

function runTool(
  command: string,
  args: readonly string[],
  timeoutMs: number,
  artifactDescriptor?: number,
): string {
  const stdio: Array<'ignore' | 'pipe' | number> = [
    'ignore',
    'pipe',
    'pipe',
  ];
  if (artifactDescriptor !== undefined) {
    while (stdio.length <= artifactDescriptor) stdio.push('ignore');
    stdio[artifactDescriptor] = artifactDescriptor;
  }
  const result = spawnSync(command, [...args], {
    encoding: 'utf8',
    stdio,
    maxBuffer: 16 * 1024 * 1024,
    timeout: timeoutMs,
    killSignal: 'SIGKILL',
  });
  if (isTimeout(result.error)) {
    throw new Error(`${basename(command)} timed out during mobile package inspection`);
  }
  if (result.status !== 0) {
    const detail = `${result.stderr ?? ''}`.trim();
    throw new Error(
      `${basename(command)} failed${detail ? `: ${detail}` : ''}`,
    );
  }
  return `${result.stdout ?? ''}${result.stderr ?? ''}`;
}

function requireEqual(actual: string, expected: string, field: string): void {
  if (actual !== expected) {
    throw new Error(
      `${field} ${JSON.stringify(actual)} does not match ${JSON.stringify(expected)}`,
    );
  }
}

function normalizeFingerprint(value: string): string {
  const normalized = value.replaceAll(':', '').trim().toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(normalized)) {
    throw new Error('signing certificate SHA-256 is missing or malformed');
  }
  return normalized;
}

function canonicalObject(value: Record<string, unknown>): string {
  return JSON.stringify(
    Object.fromEntries(
      Object.entries(value).sort(([left], [right]) =>
        left.localeCompare(right),
      ),
    ),
  );
}

function requireExactEntitlements(
  actual: Record<string, unknown>,
  expected: Record<string, unknown>,
  label: string,
): string {
  const unexpected = Object.keys(actual).filter(
    (key) => !Object.hasOwn(expected, key),
  );
  if (unexpected.length > 0) {
    throw new Error(
      `unexpected ${label} entitlement(s): ${unexpected.sort().join(', ')}`,
    );
  }
  const actualCanonical = canonicalObject(actual);
  const expectedCanonical = canonicalObject(expected);
  if (actualCanonical !== expectedCanonical) {
    throw new Error(`${label} entitlements do not match the reviewed allowlist`);
  }
  return createHash('sha256').update(actualCanonical).digest('hex');
}

function expectedReleaseIdentity(sourceCommit: string): Buffer {
  if (!/^[0-9a-f]{40}$/.test(sourceCommit)) {
    throw new Error('expected source commit must be a lowercase 40-character Git SHA');
  }
  return Buffer.from(
    [
      'MDREADER_RELEASE_IDENTITY_V1\0',
      'productName=MDReader\0',
      `bundleIdentifier=${MOBILE_PRODUCTION_PACKAGE_ID}\0`,
      `sourceCommit=${sourceCommit}\0`,
    ].join(''),
  );
}

function verifyReleaseIdentity(body: Buffer, sourceCommit: string): void {
  if (!body.includes(expectedReleaseIdentity(sourceCommit))) {
    throw new Error('native package does not carry the expected release identity');
  }
  if (
    body.includes(Buffer.from('bundleIdentifier=dev.mdreader.app.e2e\0')) ||
    body.includes(Buffer.from('profile=e2e-build'))
  ) {
    throw new Error('native package carries the E2E release identity');
  }
}

function zipEntries(
  artifactPath: string,
  run: MobileBetaToolRunner,
): ZipEntry[] {
  const entries = run('unzip', ['-Z1', artifactPath])
    .split(/\r?\n/)
    .filter(Boolean);
  if (entries.length === 0) {
    throw new Error('mobile package ZIP is empty');
  }
  const seen = new Set<string>();
  const portable = new Map<string, string>();
  for (const entry of entries) {
    const canonical = entry.endsWith('/') ? entry.slice(0, -1) : entry;
    const parts = canonical.split('/');
    if (
      canonical.length === 0 ||
      entry.startsWith('/') ||
      entry.includes('\\') ||
      parts.some(
        (part) =>
          part.length === 0 ||
          part === '..' ||
          part === '.' ||
          /[\u0000-\u001f\u007f]/u.test(part),
      ) ||
      entry.includes('\0')
    ) {
      throw new Error(`unsafe ZIP entry: ${JSON.stringify(entry)}`);
    }
    if (seen.has(entry)) {
      throw new Error(`duplicate ZIP entry: ${JSON.stringify(entry)}`);
    }
    seen.add(entry);
    const collisionKey = canonical.normalize('NFD').toLocaleLowerCase('en-US');
    const collided = portable.get(collisionKey);
    if (collided !== undefined && collided !== canonical) {
      throw new Error(
        `case or Unicode-normalization ZIP path collision: ${JSON.stringify(collided)} and ${JSON.stringify(canonical)}`,
      );
    }
    portable.set(collisionKey, canonical);
  }
  if (entries.length > MAX_ZIP_ENTRIES) {
    throw new Error(`ZIP entry count exceeds the ${MAX_ZIP_ENTRIES}-entry limit`);
  }
  return validateZipSizes(artifactPath, entries, run);
}

function validateZipSizes(
  artifactPath: string,
  entries: readonly string[],
  run: MobileBetaToolRunner,
): ZipEntry[] {
  const records = run('zipinfo', ['-l', artifactPath])
    .split(/\r?\n/)
    .flatMap((line) => {
      const match = line.match(
        /^(\S+)\s+\S+\s+\S+\s+(\d+)\s+\S+\s+(\d+)\s+\S+\s+\S+\s+\S+\s+(.+)$/,
      );
      if (!match) return [];
      return [
        {
          mode: match[1]!,
          uncompressed: Number(match[2]),
          compressed: Number(match[3]),
          path: match[4]!,
        },
      ];
    });
  if (records.length !== entries.length) {
    throw new Error('ZIP size inventory does not match the central-directory entry list');
  }
  let compressed = 0;
  let uncompressed = 0;
  const inventory: ZipEntry[] = [];
  for (let index = 0; index < records.length; index += 1) {
    const record = records[index]!;
    if (record.path !== entries[index]) {
      throw new Error('ZIP size inventory path differs from the entry list');
    }
    if (
      !Number.isSafeInteger(record.compressed) ||
      !Number.isSafeInteger(record.uncompressed) ||
      record.compressed < 0 ||
      record.uncompressed < 0
    ) {
      throw new Error(`ZIP entry has invalid sizes: ${JSON.stringify(record.path)}`);
    }
    if (record.compressed > MAX_ZIP_ENTRY_COMPRESSED_BYTES) {
      throw new Error(
        `ZIP compressed entry exceeds the ${MAX_ZIP_ENTRY_COMPRESSED_BYTES}-byte limit`,
      );
    }
    if (record.uncompressed > MAX_ZIP_ENTRY_UNCOMPRESSED_BYTES) {
      throw new Error(
        `ZIP uncompressed entry exceeds the ${MAX_ZIP_ENTRY_UNCOMPRESSED_BYTES}-byte limit`,
      );
    }
    compressed += record.compressed;
    uncompressed += record.uncompressed;
    if (compressed > MAX_ZIP_COMPRESSED_BYTES) {
      throw new Error(
        `ZIP compressed total exceeds the ${MAX_ZIP_COMPRESSED_BYTES}-byte limit`,
      );
    }
    if (uncompressed > MAX_ZIP_UNCOMPRESSED_BYTES) {
      throw new Error(
        `ZIP uncompressed total exceeds the ${MAX_ZIP_UNCOMPRESSED_BYTES}-byte limit`,
      );
    }
    const kind =
      record.mode.startsWith('d')
        ? 'directory'
        : record.mode.startsWith('l')
          ? 'symlink'
          : record.mode.startsWith('-')
            ? 'file'
            : undefined;
    if (!kind) {
      throw new Error(
        `ZIP entry has unsupported file type: ${JSON.stringify(record.path)}`,
      );
    }
    if ((kind === 'directory') !== record.path.endsWith('/')) {
      throw new Error(
        `ZIP entry type conflicts with its path: ${JSON.stringify(record.path)}`,
      );
    }
    inventory.push({
      path: record.path,
      kind,
      uncompressedBytes: record.uncompressed,
    });
  }
  return inventory;
}

function validateIosArchiveLinks(
  artifactPath: string,
  entries: readonly ZipEntry[],
  appPrefix: string,
  run: MobileBetaToolRunner,
): void {
  const appRoot = appPrefix.endsWith('/')
    ? appPrefix.slice(0, -1)
    : appPrefix;
  const symlinkEntries = entries.filter((entry) => entry.kind === 'symlink');
  const symlinks = new Set(symlinkEntries.map((entry) => entry.path));
  for (const entry of entries) {
    const parts = entry.path.replace(/\/$/, '').split('/');
    for (let length = 1; length < parts.length; length += 1) {
      const ancestor = parts.slice(0, length).join('/');
      if (symlinks.has(ancestor)) {
        throw new Error(
          `archive symlink ${JSON.stringify(ancestor)} has child entry ${JSON.stringify(entry.path)}`,
        );
      }
    }
  }
  for (const entry of symlinkEntries) {
    const link = entry.path;
    if (!link.startsWith(`${appRoot}/`)) {
      throw new Error(`archive symlink is outside the application root: ${link}`);
    }
    if (
      entry.uncompressedBytes <= 0 ||
      entry.uncompressedBytes > MAX_ZIP_SYMLINK_TARGET_BYTES
    ) {
      throw new Error(`archive symlink target is outside reviewed bounds: ${link}`);
    }
    const target = run('unzip', ['-p', artifactPath, link]);
    if (
      Buffer.byteLength(target, 'utf8') !== entry.uncompressedBytes ||
      target.includes('\0') ||
      target.includes('\\') ||
      posix.isAbsolute(target)
    ) {
      throw new Error(`archive symlink has an unsafe target: ${link}`);
    }
    const resolved = posix.normalize(posix.join(posix.dirname(link), target));
    if (
      resolved !== appRoot &&
      !resolved.startsWith(`${appRoot}/`)
    ) {
      throw new Error(`archive symlink target escapes the application root: ${link}`);
    }
    if (symlinks.has(resolved)) {
      throw new Error(`archive symlink chains are not accepted: ${link}`);
    }
  }
}

function readZipEntry(
  artifactPath: string,
  entry: string,
  timeoutMs: number,
  artifactDescriptor?: number,
): Buffer {
  const stdio: Array<'ignore' | 'pipe' | number> = [
    'ignore',
    'pipe',
    'pipe',
  ];
  if (artifactDescriptor !== undefined) {
    while (stdio.length <= artifactDescriptor) stdio.push('ignore');
    stdio[artifactDescriptor] = artifactDescriptor;
  }
  const result = spawnSync('unzip', ['-p', artifactPath, entry], {
    encoding: 'buffer',
    stdio,
    maxBuffer: 256 * 1024 * 1024,
    timeout: timeoutMs,
    killSignal: 'SIGKILL',
  });
  if (isTimeout(result.error)) {
    throw new Error('unzip timed out while reading a native package entry');
  }
  if (result.status !== 0 || !Buffer.isBuffer(result.stdout)) {
    throw new Error(`cannot read ZIP entry ${JSON.stringify(entry)}`);
  }
  return result.stdout;
}

async function verifyExtractedTree(root: string, path = root): Promise<void> {
  const resolvedRoot = await realpath(root);
  for (const entry of await readdir(path, { withFileTypes: true })) {
    const child = join(path, entry.name);
    const metadata = await lstat(child);
    if (
      !metadata.isDirectory() &&
      !metadata.isFile() &&
      !metadata.isSymbolicLink()
    ) {
      throw new Error(`unsupported extracted package entry: ${child}`);
    }
    const resolved = await realpath(child);
    const scope = relative(resolvedRoot, resolved);
    if (
      scope === '..' ||
      scope.startsWith(`..${sep}`) ||
      resolve(resolvedRoot, scope) !== resolved
    ) {
      throw new Error(`extracted package entry escapes its staging directory: ${child}`);
    }
    if (metadata.isDirectory()) {
      await verifyExtractedTree(root, child);
    }
  }
}

function extractXml(output: string, field: string): string {
  const start = output.indexOf('<?xml');
  const plist = output.indexOf('<plist');
  const offset = start >= 0 ? start : plist;
  if (offset < 0) {
    throw new Error(`${field} did not contain a property list`);
  }
  return output.slice(offset);
}

async function plistJson(
  path: string,
  run: MobileBetaToolRunner,
): Promise<Record<string, unknown>> {
  const parsed: unknown = JSON.parse(
    run('plutil', ['-convert', 'json', '-o', '-', path]),
  );
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error(`property list is not a dictionary: ${path}`);
  }
  return parsed as Record<string, unknown>;
}

function stringField(
  value: Record<string, unknown>,
  field: string,
  source: string,
): string {
  const result = value[field];
  if (typeof result !== 'string' || result.length === 0) {
    throw new Error(`${source} is missing ${field}`);
  }
  return result;
}

/**
 * Inspects a signed device IPA. Values are derived from the package and its
 * signature rather than copied from workflow inputs.
 */
export async function inspectIosBetaArtifact(
  artifactPath: string,
  expected: IosExpected,
  runner?: MobileBetaToolRunner,
  options: MobileBetaInspectionOptions = {},
): Promise<MobileBetaArtifactEvidence> {
  const toolTimeoutMs =
    options.toolTimeoutMs ?? DEFAULT_INSPECTION_TOOL_TIMEOUT_MS;
  const toolArtifactPath =
    options.artifactDescriptor === undefined
      ? artifactPath
      : `/dev/fd/${options.artifactDescriptor}`;
  const run =
    runner ??
    createMobileBetaInspectionToolRunner(
      toolTimeoutMs,
      options.artifactDescriptor,
    );
  if (!artifactPath.toLowerCase().endsWith('.ipa')) {
    throw new Error('iOS beta artifact must be an IPA');
  }
  const entries = zipEntries(toolArtifactPath, run);
  const infoEntries = entries.filter((entry) =>
    /^Payload\/[^/]+\.app\/Info\.plist$/.test(entry.path),
  );
  if (infoEntries.length !== 1) {
    throw new Error(`expected exactly one iOS application; found ${infoEntries.length}`);
  }
  const appPrefix = infoEntries[0]!.path.slice(0, -'Info.plist'.length);
  validateIosArchiveLinks(toolArtifactPath, entries, appPrefix, run);

  const staging = await mkdtemp(join(tmpdir(), 'mdreader-ios-beta-'));
  try {
    run('unzip', ['-q', toolArtifactPath, '-d', staging]);
    await verifyExtractedTree(staging);
    const appPath = join(staging, appPrefix);
    const info = await plistJson(join(appPath, 'Info.plist'), run);
    const packageId = stringField(info, 'CFBundleIdentifier', 'Info.plist');
    const executableName = stringField(info, 'CFBundleExecutable', 'Info.plist');
    const version = stringField(
      info,
      'CFBundleShortVersionString',
      'Info.plist',
    );
    const buildNumber = stringField(info, 'CFBundleVersion', 'Info.plist');
    requireEqual(packageId, MOBILE_PRODUCTION_PACKAGE_ID, 'iOS bundle identifier');
    requireEqual(version, expected.version, 'iOS version');
    requireEqual(buildNumber, expected.buildNumber, 'iOS build number');

    const executable = join(appPath, executableName);
    const executableBytes = await readFile(executable);
    verifyReleaseIdentity(executableBytes, expected.sourceCommit);
    const architectures = run('lipo', ['-archs', executable])
      .trim()
      .split(/\s+/)
      .filter(Boolean);
    if (architectures.length !== 1 || architectures[0] !== 'arm64') {
      throw new Error(`iOS beta architecture must be exactly arm64; found ${architectures.join(', ')}`);
    }

    run('codesign', ['--verify', '--deep', '--strict', appPath]);
    const signature = run('codesign', ['-dv', '--verbose=4', appPath]);
    if (!signature.includes('Authority=Apple Distribution:')) {
      throw new Error('iOS package is not signed by an Apple Distribution identity');
    }
    const teamId = signature.match(/^TeamIdentifier=(.+)$/m)?.[1]?.trim();
    if (!teamId) throw new Error('iOS code signature has no team identifier');
    requireEqual(teamId, expected.teamId, 'iOS signer team identifier');

    const certificatePrefix = join(staging, 'signer-cert-');
    run('codesign', [`--extract-certificates=${certificatePrefix}`, appPath]);
    const certificateSha256 = createHash('sha256')
      .update(await readFile(`${certificatePrefix}0`))
      .digest('hex');
    requireEqual(
      certificateSha256,
      normalizeFingerprint(expected.certificateSha256),
      'iOS signing certificate SHA-256',
    );

    const profileXml = run('security', [
      'cms',
      '-D',
      '-i',
      join(appPath, 'embedded.mobileprovision'),
    ]);
    const provisioningProfileSha256 = createHash('sha256')
      .update(await readFile(join(appPath, 'embedded.mobileprovision')))
      .digest('hex');
    requireEqual(
      provisioningProfileSha256,
      normalizeFingerprint(expected.provisioningProfileSha256),
      'embedded provisioning profile SHA-256',
    );
    const profilePath = join(staging, 'profile.plist');
    await writeFile(profilePath, extractXml(profileXml, 'embedded provisioning profile'));
    const profile = await plistJson(profilePath, run);
    const provisioningProfileUuid =
      typeof profile.UUID === 'string' &&
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
        profile.UUID,
      )
        ? profile.UUID.toLowerCase()
        : undefined;
    if (!provisioningProfileUuid) {
      throw new Error('embedded provisioning profile UUID is malformed');
    }
    const profileTeams = profile.TeamIdentifier;
    if (
      !Array.isArray(profileTeams) ||
      profileTeams.length !== 1 ||
      profileTeams[0] !== teamId
    ) {
      throw new Error('embedded provisioning profile does not match the signing team');
    }
    const profileEntitlements = profile.Entitlements;
    if (
      !profileEntitlements ||
      typeof profileEntitlements !== 'object' ||
      Array.isArray(profileEntitlements)
    ) {
      throw new Error('embedded provisioning profile has no entitlements');
    }
    const profileValues = profileEntitlements as Record<string, unknown>;
    requireEqual(
      String(profileValues['application-identifier'] ?? ''),
      `${teamId}.${MOBILE_PRODUCTION_PACKAGE_ID}`,
      'provisioning application identifier',
    );
    if (profileValues['get-task-allow'] !== false) {
      throw new Error('iOS beta provisioning profile permits debugging');
    }
    if ('ProvisionedDevices' in profile) {
      throw new Error(
        'iOS beta package uses a development or ad hoc profile, not an App Store profile',
      );
    }
    if (profile.ProvisionsAllDevices === true) {
      throw new Error(
        'iOS beta package uses an enterprise profile, not an App Store profile',
      );
    }
    if (profileValues['beta-reports-active'] !== true) {
      throw new Error(
        'iOS App Store profile does not authorize TestFlight beta reporting',
      );
    }
    const expiration = profile.ExpirationDate;
    if (
      typeof expiration !== 'string' ||
      !Number.isFinite(Date.parse(expiration)) ||
      Date.parse(expiration) <= Date.now()
    ) {
      throw new Error('iOS App Store provisioning profile is expired or malformed');
    }
    const profileEntitlementsSha256 = requireExactEntitlements(
      profileValues,
      {
        'application-identifier': `${teamId}.${MOBILE_PRODUCTION_PACKAGE_ID}`,
        'beta-reports-active': true,
        'com.apple.developer.team-identifier': teamId,
        'get-task-allow': false,
        'keychain-access-groups': [`${teamId}.*`],
      },
      'provisioning profile',
    );

    const entitlementsXml = run('codesign', [
      '-d',
      '--entitlements',
      ':-',
      appPath,
    ]);
    const entitlementsPath = join(staging, 'entitlements.plist');
    await writeFile(
      entitlementsPath,
      extractXml(entitlementsXml, 'signed iOS entitlements'),
    );
    const entitlements = await plistJson(entitlementsPath, run);
    requireEqual(
      String(entitlements['application-identifier'] ?? ''),
      `${teamId}.${MOBILE_PRODUCTION_PACKAGE_ID}`,
      'signed iOS application identifier',
    );
    requireEqual(
      String(entitlements['com.apple.developer.team-identifier'] ?? ''),
      teamId,
      'signed iOS team identifier',
    );
    if (entitlements['get-task-allow'] !== false) {
      throw new Error('signed iOS beta package permits debugging');
    }
    const signedEntitlementsSha256 = requireExactEntitlements(
      entitlements,
      {
        'application-identifier': `${teamId}.${MOBILE_PRODUCTION_PACKAGE_ID}`,
        'com.apple.developer.team-identifier': teamId,
        'get-task-allow': false,
      },
      'signed iOS',
    );

    return {
      platform: 'ios',
      channel: 'testflight',
      artifactPath,
      sourceCommit: expected.sourceCommit,
      version,
      buildNumber,
      packageId,
      architectures,
      signingIdentity: {
        kind: 'apple-distribution',
        teamId,
        certificateSha256,
      },
      iosAuthority: {
        provisioningProfileUuid,
        provisioningProfileSha256,
        profileEntitlementsSha256,
        signedEntitlementsSha256,
      },
    };
  } finally {
    await rm(staging, { recursive: true, force: true });
  }
}

function bundletoolValue(
  jarPath: string,
  artifactPath: string,
  xpath: string,
  run: MobileBetaToolRunner,
): string {
  return run('java', [
    '-jar',
    jarPath,
    'dump',
    'manifest',
    `--bundle=${artifactPath}`,
    `--xpath=${xpath}`,
  ]).trim();
}

function verifyAndroidJarSignatureCoverage(
  entries: readonly ZipEntry[],
  output: string,
): void {
  if (!/\bjar verified\.\s*$/m.test(output)) {
    throw new Error('Android App Bundle JAR signature was not verified');
  }
  const lines = output.split(/\r?\n/);
  for (const entry of entries) {
    const isJarSignatureInfrastructure =
      entry.path === 'META-INF/MANIFEST.MF' ||
      /^META-INF\/[^/]+\.(SF|RSA|DSA|EC)$/.test(entry.path);
    if (entry.kind !== 'file' || isJarSignatureInfrastructure) continue;
    const inventoryLine = lines.find((line) =>
      line.endsWith(` ${entry.path}`),
    );
    if (!inventoryLine || !/^s[mk ]*\s+\d+\s/.test(inventoryLine.trimStart())) {
      throw new Error(
        `Android App Bundle entry lacks signature coverage: ${JSON.stringify(entry.path)}`,
      );
    }
  }
}

/**
 * Inspects a signed Android App Bundle using the reviewed bundletool binary,
 * JDK signature tools, and the package's real native libraries.
 */
export async function inspectAndroidBetaArtifact(
  artifactPath: string,
  bundletoolJarPath: string,
  expected: AndroidExpected,
  runner?: MobileBetaToolRunner,
  options: MobileBetaInspectionOptions = {},
): Promise<MobileBetaArtifactEvidence> {
  const toolTimeoutMs =
    options.toolTimeoutMs ?? DEFAULT_INSPECTION_TOOL_TIMEOUT_MS;
  const toolArtifactPath =
    options.artifactDescriptor === undefined
      ? artifactPath
      : `/dev/fd/${options.artifactDescriptor}`;
  const run =
    runner ??
    createMobileBetaInspectionToolRunner(
      toolTimeoutMs,
      options.artifactDescriptor,
    );
  if (!artifactPath.toLowerCase().endsWith('.aab')) {
    throw new Error('Android beta artifact must be an AAB');
  }
  const entries = zipEntries(toolArtifactPath, run);
  if (entries.some((entry) => entry.kind === 'symlink')) {
    throw new Error('Android App Bundle may not contain archive symlinks');
  }
  const signatureFiles = entries.filter((entry) =>
    /^META-INF\/[^/]+\.(RSA|DSA|EC)$/.test(entry.path),
  );
  const signatureMetadata = entries.filter((entry) =>
    /^META-INF\/[^/]+\.SF$/.test(entry.path),
  );
  if (signatureFiles.length === 0 || signatureMetadata.length === 0) {
    throw new Error('Android App Bundle has no JAR signing block');
  }

  run('java', ['-jar', bundletoolJarPath, 'validate', `--bundle=${toolArtifactPath}`]);
  verifyAndroidJarSignatureCoverage(
    entries,
    run('jarsigner', ['-verify', '-verbose', '-certs', toolArtifactPath]),
  );

  const packageId = bundletoolValue(
    bundletoolJarPath,
    toolArtifactPath,
    '/manifest/@package',
    run,
  );
  const version = bundletoolValue(
    bundletoolJarPath,
    toolArtifactPath,
    '/manifest/@android:versionName',
    run,
  );
  const buildNumber = bundletoolValue(
    bundletoolJarPath,
    toolArtifactPath,
    '/manifest/@android:versionCode',
    run,
  );
  requireEqual(packageId, MOBILE_PRODUCTION_PACKAGE_ID, 'Android application ID');
  requireEqual(version, expected.version, 'Android version');
  requireEqual(buildNumber, expected.buildNumber, 'Android version code');

  const nativeLibraries = entries.filter((entry) =>
    /^base\/lib\/[^/]+\/[^/]+\.so$/.test(entry.path),
  );
  const architectures = [
    ...new Set(
      nativeLibraries
        .map((entry) => entry.path.split('/')[2]!)
        .filter(Boolean),
    ),
  ].sort();
  if (architectures.length !== 1 || architectures[0] !== 'arm64-v8a') {
    throw new Error(
      `Android beta architecture must be exactly arm64-v8a; found ${architectures.join(', ')}`,
    );
  }
  if (
    !nativeLibraries.some((entry) => {
      const body = readZipEntry(
        toolArtifactPath,
        entry.path,
        toolTimeoutMs,
        options.artifactDescriptor,
      );
      try {
        verifyReleaseIdentity(body, expected.sourceCommit);
        return true;
      } catch {
        return false;
      }
    })
  ) {
    throw new Error('Android native libraries do not carry the expected release identity');
  }

  const certificateOutput = run('keytool', [
    '-printcert',
    '-jarfile',
    toolArtifactPath,
  ]);
  const certificate = certificateOutput.match(
    /SHA256:\s*([0-9A-Fa-f:]{64,95})/,
  )?.[1];
  if (!certificate) {
    throw new Error('Android package signer certificate SHA-256 is missing');
  }
  const certificateSha256 = normalizeFingerprint(certificate);
  requireEqual(
    certificateSha256,
    normalizeFingerprint(expected.certificateSha256),
    'Android upload certificate SHA-256',
  );

  return {
    platform: 'android',
    channel: 'google-play-closed',
    artifactPath,
    sourceCommit: expected.sourceCommit,
    version,
    buildNumber,
    packageId,
    architectures,
    signingIdentity: {
      kind: 'google-play-upload',
      certificateSha256,
    },
  };
}
