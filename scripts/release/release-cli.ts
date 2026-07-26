#!/usr/bin/env node

import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { parseReleaseManifest } from '../../apps/web/lib/release-manifest';
import {
  createReleaseConfig,
  readTauriIdentifier,
  readWorkspaceVersion,
  updaterPublicKeyFingerprint,
} from './release-config';
import {
  publicationPlan,
  validateReleaseDirectory,
  type PublicationEntry,
} from './release-pipeline';
import {
  attestReleaseBuildConfig,
  sealReleaseBundle,
  verifyReleaseBuildAttestation,
} from './release-build-attestation';
import {
  createR2Client,
  publishReleaseEntries,
  putR2ReleaseEntry,
  readR2Object,
} from './r2-publisher';
import { assertGlibcCompatibility } from './platform-compat';
import {
  stagePlatformArtifacts,
} from './stage-release';

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, '..', '..');

function option(args: string[], name: string, required = true): string | undefined {
  const index = args.indexOf(name);
  const value = index >= 0 ? args[index + 1] : undefined;
  if (required && (!value || value.startsWith('--'))) {
    throw new Error(`missing required option ${name}`);
  }
  return value;
}

function requireEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`missing required release secret ${name}`);
  return value;
}

async function writeConfig(args: string[]): Promise<void> {
  const version = option(args, '--version')!;
  const output = resolve(option(args, '--output')!);
  const config = createReleaseConfig({
    version,
    workspaceVersion: readWorkspaceVersion(join(ROOT, 'Cargo.toml')),
    bundleIdentifier: readTauriIdentifier(
      join(ROOT, 'apps', 'mdreader', 'src-tauri', 'tauri.conf.json'),
    ),
    updaterPublicKey: requireEnvironment('MDREADER_UPDATER_PUBLIC_KEY'),
    updaterEndpoint:
      option(args, '--updater-endpoint', false) ??
      'https://downloads.mdreader.dev/channels/stable/updater.json',
    macosSigningIdentity: option(args, '--macos-signing-identity', false),
    windowsCertificateThumbprint: option(args, '--windows-thumbprint', false),
    windowsInstallerHooks: option(args, '--windows-thumbprint', false)
      ? join(ROOT, 'scripts', 'release', 'windows-minimum.nsh')
      : undefined,
  });
  await mkdir(dirname(output), { recursive: true });
  await writeFile(output, `${JSON.stringify(config, null, 2)}\n`, { mode: 0o600 });
  process.stdout.write(`release config written: ${output}\n`);
}

async function stage(args: string[]): Promise<void> {
  const platform = option(args, '--platform')!;
  if (!['macos', 'windows', 'linux'].includes(platform)) {
    throw new Error(`unsupported release platform ${platform}`);
  }
  const version = option(args, '--version')!;
  const workspaceVersion = readWorkspaceVersion(join(ROOT, 'Cargo.toml'));
  if (version !== workspaceVersion) {
    throw new Error(`release version ${version} does not match workspace version ${workspaceVersion}`);
  }
  const fragment = await stagePlatformArtifacts({
    platform: platform as 'macos' | 'windows' | 'linux',
    version,
    sourceCommit: option(args, '--source-commit')!,
    publishedAt: option(args, '--published-at')!,
    bundleRoot: resolve(option(args, '--bundle-root')!),
    outputRoot: resolve(option(args, '--output-root')!),
    releaseConfigPath: resolve(option(args, '--release-config')!),
    buildAttestationPath: resolve(option(args, '--build-attestation')!),
    bundleSealPath: resolve(option(args, '--bundle-seal')!),
    buildTarget: option(args, '--build-target')!,
    buildBundles: option(args, '--build-bundles')!,
  });
  process.stdout.write(
    `staged ${platform}: ${fragment.artifacts.length} verified artifact records\n`,
  );
}

async function sealBuild(args: string[]): Promise<void> {
  await sealReleaseBundle(
    resolve(option(args, '--bundle-root')!),
    resolve(option(args, '--config')!),
    resolve(option(args, '--attestation')!),
    resolve(option(args, '--seal')!),
    {
      version: option(args, '--version')!,
      target: option(args, '--target')!,
      bundles: option(args, '--bundles')!,
    },
  );
  process.stdout.write('release bundle sealed\n');
}

async function attestBuild(args: string[]): Promise<void> {
  const attestation = await attestReleaseBuildConfig(
    resolve(option(args, '--config')!),
    resolve(option(args, '--attestation')!),
    option(args, '--target')!,
    option(args, '--bundles')!,
  );
  process.stdout.write(
    `release build config attested: ${attestation.releaseConfigSha256}\n`,
  );
}

async function verifyBuild(args: string[]): Promise<void> {
  await verifyReleaseBuildAttestation(
    resolve(option(args, '--config')!),
    resolve(option(args, '--attestation')!),
    {
      version: option(args, '--version')!,
      target: option(args, '--target')!,
      bundles: option(args, '--bundles')!,
    },
  );
  process.stdout.write('release build attestation verified\n');
}

async function assemble(args: string[]): Promise<void> {
  const root = resolve(option(args, '--root')!);
  const version = option(args, '--version')!;
  const sourceCommit = option(args, '--source-commit')!;
  const generatedAt = option(args, '--published-at')!;
  const fragments = await Promise.all(
    ['macos', 'windows', 'linux'].map(async (platform) => {
      const path = join(root, `fragment-${platform}.json`);
      try {
        return JSON.parse(await readFile(path, 'utf8')) as {
          channel: 'stable';
          current: true;
          version: string;
          sourceCommit: string;
          updaterPublicKeySha256: string;
          releaseConfigSha256: string;
          artifacts: unknown[];
        };
      } catch {
        throw new Error(`missing or invalid platform release fragment: ${path}`);
      }
    }),
  );
  for (const fragment of fragments) {
    if (fragment.version !== version || fragment.sourceCommit !== sourceCommit) {
      throw new Error('platform fragment version/source commit mismatch');
    }
    if (
      !/^[0-9a-f]{64}$/.test(fragment.releaseConfigSha256) ||
      fragment.artifacts.some((artifact) => {
        if (
          typeof artifact !== 'object' ||
          artifact === null ||
          Array.isArray(artifact)
        ) {
          return true;
        }
        const record = artifact as Record<string, unknown>;
        return (
          record.releaseConfigSha256 !== fragment.releaseConfigSha256 ||
          record.updaterPublicKeySha256 !== fragment.updaterPublicKeySha256
        );
      })
    ) {
      throw new Error(
        'platform fragment artifacts do not match the attested release build config',
      );
    }
  }
  const updaterPublicKeySha256 = updaterPublicKeyFingerprint(
    requireEnvironment('MDREADER_UPDATER_PUBLIC_KEY'),
  );
  if (
    fragments.some(
      (fragment) =>
        fragment.updaterPublicKeySha256 !== updaterPublicKeySha256,
    )
  ) {
    throw new Error(
      'platform fragment updater key fingerprint does not match the configured updater key',
    );
  }
  const manifest = parseReleaseManifest({
    schemaVersion: 1,
    generatedAt,
    sourceCommit,
    updaterPublicKeySha256,
    releases: [
      {
        channel: 'stable',
        current: true,
        version,
        artifacts: fragments.flatMap((fragment) => fragment.artifacts),
      },
    ],
  });
  const output =
    resolve(option(args, '--output', false) ?? join(root, 'releases', `v${version}`, 'manifest.json'));
  await mkdir(dirname(output), { recursive: true });
  await writeFile(output, `${JSON.stringify(manifest, null, 2)}\n`);
  process.stdout.write(`immutable release manifest assembled: ${output}\n`);
}

async function verifyLinuxAbi(args: string[]): Promise<void> {
  const reportPath = resolve(option(args, '--report')!);
  const maximum = option(args, '--max-glibc')!;
  const required = assertGlibcCompatibility(
    await readFile(reportPath, 'utf8'),
    maximum,
  );
  process.stdout.write(`verified Linux payload requires at most GLIBC ${required}\n`);
}

async function plan(args: string[]): Promise<{
  entries: PublicationEntry[];
  root: string;
}> {
  const manifestPath = resolve(option(args, '--manifest')!);
  const root = resolve(option(args, '--root')!);
  const sourceCommit = option(args, '--source-commit')!;
  const updaterPublicKey = requireEnvironment('MDREADER_UPDATER_PUBLIC_KEY');
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8')) as unknown;
  const verified = await validateReleaseDirectory(
    manifest,
    root,
    sourceCommit,
    updaterPublicKey,
  );
  return { entries: publicationPlan(verified), root };
}

async function publish(args: string[]): Promise<void> {
  const { entries } = await plan(args);
  const live = args.includes('--publish');
  if (!live && !args.includes('--dry-run')) {
    throw new Error('choose exactly one of --dry-run or --publish');
  }
  if (live && args.includes('--dry-run')) {
    throw new Error('choose exactly one of --dry-run or --publish');
  }
  if (!live) {
    for (const entry of entries) process.stdout.write(`DRY RUN ${entry.kind} ${entry.key}\n`);
    return;
  }

  const accountId = requireEnvironment('CLOUDFLARE_ACCOUNT_ID');
  const accessKeyId = requireEnvironment('MDREADER_R2_ACCESS_KEY_ID');
  const secretAccessKey = requireEnvironment('MDREADER_R2_SECRET_ACCESS_KEY');
  const bucket = option(args, '--bucket')!;
  if (!/^[a-z0-9][a-z0-9-]{1,62}$/.test(bucket)) throw new Error('invalid R2 bucket name');
  const client = createR2Client({ accountId, accessKeyId, secretAccessKey });
  try {
    await publishReleaseEntries(
      entries,
      async (entry) =>
        entry.localPath === null
          ? Buffer.from(entry.contents ?? '', 'utf8')
          : await readFile(entry.localPath),
      {
        read: (key) => readR2Object(client, bucket, key),
        put: async (entry, body, expectedEtag) => {
          process.stdout.write(`publishing ${entry.kind} ${entry.key}\n`);
          await putR2ReleaseEntry(
            client,
            bucket,
            entry,
            body,
            expectedEtag,
          );
        },
      },
    );
  } finally {
    client.destroy();
  }
}

async function main(): Promise<void> {
  const [command, ...args] = process.argv.slice(2);
  if (command === 'config') return writeConfig(args);
  if (command === 'attest-build') return attestBuild(args);
  if (command === 'seal-build') return sealBuild(args);
  if (command === 'verify-build-attestation') return verifyBuild(args);
  if (command === 'stage') return stage(args);
  if (command === 'assemble') return assemble(args);
  if (command === 'verify-linux-abi') return verifyLinuxAbi(args);
  if (command === 'validate') {
    const result = await plan(args);
    process.stdout.write(`verified publication plan: ${result.entries.length} objects\n`);
    return;
  }
  if (command === 'publish') return publish(args);
  throw new Error('usage: release-cli.ts <config|stage|assemble|verify-linux-abi|validate|publish> [options]');
}

main().catch((error: unknown) => {
  process.stderr.write(`release pipeline refused: ${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 1;
});
