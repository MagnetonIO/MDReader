#!/usr/bin/env node

import { execFileSync } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { parseReleaseManifest } from '../../apps/web/lib/release-manifest';
import {
  createReleaseConfig,
  readTauriIdentifier,
  readWorkspaceVersion,
} from './release-config';
import {
  publicationPlan,
  validateReleaseDirectory,
  type PublicationEntry,
} from './release-pipeline';
import {
  assertForwardStableVersion,
  createR2Client,
  putR2ReleaseEntry,
  readR2StablePointer,
} from './r2-publisher';
import {
  stagePlatformArtifacts,
  type SbomPackage,
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

function packagesFromValue(value: unknown, output: Map<string, SbomPackage>): void {
  if (Array.isArray(value)) {
    for (const item of value) packagesFromValue(item, output);
    return;
  }
  if (typeof value !== 'object' || value === null) return;
  const object = value as Record<string, unknown>;
  if (typeof object.name === 'string' && typeof object.version === 'string') {
    output.set(`${object.name}@${object.version}`, {
      name: object.name,
      version: object.version,
    });
  }
  for (const child of Object.values(object)) packagesFromValue(child, output);
}

function dependencyPackages(): SbomPackage[] {
  const output = new Map<string, SbomPackage>();
  const cargo = JSON.parse(
    execFileSync(
      'cargo',
      ['metadata', '--format-version', '1', '--locked'],
      { cwd: ROOT, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 },
    ),
  ) as { packages: Array<{ name: string; version: string }> };
  for (const pkg of cargo.packages) {
    output.set(`${pkg.name}@${pkg.version}`, { name: pkg.name, version: pkg.version });
  }
  const pnpm = JSON.parse(
    execFileSync(
      'pnpm',
      ['list', '--recursive', '--json', '--depth', 'Infinity'],
      { cwd: ROOT, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 },
    ),
  ) as unknown;
  packagesFromValue(pnpm, output);
  return [...output.values()];
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
    packages: dependencyPackages(),
  });
  process.stdout.write(
    `staged ${platform}: ${fragment.artifacts.length} verified artifact records\n`,
  );
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
  }
  const manifest = parseReleaseManifest({
    schemaVersion: 1,
    generatedAt,
    sourceCommit,
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
    const pointerEtags = new Map<string, string | null>();
    for (const entry of entries.filter((candidate) => !candidate.immutable)) {
      if (entry.contents === null) {
        throw new Error(`mutable pointer ${entry.key} has no contents`);
      }
      const next = JSON.parse(entry.contents) as { version?: unknown };
      if (typeof next.version !== 'string') {
        throw new Error(`mutable pointer ${entry.key} has no semantic version`);
      }
      const current = await readR2StablePointer(client, bucket, entry.key);
      assertForwardStableVersion(current?.version ?? null, next.version);
      pointerEtags.set(entry.key, current?.etag ?? null);
    }
    for (const entry of entries) {
      const body =
        entry.localPath === null
          ? entry.contents ?? ''
          : await readFile(entry.localPath);
      process.stdout.write(`publishing ${entry.kind} ${entry.key}\n`);
      await putR2ReleaseEntry(
        client,
        bucket,
        entry,
        body,
        entry.immutable ? undefined : pointerEtags.get(entry.key),
      );
    }
  } finally {
    client.destroy();
  }
}

async function main(): Promise<void> {
  const [command, ...args] = process.argv.slice(2);
  if (command === 'config') return writeConfig(args);
  if (command === 'stage') return stage(args);
  if (command === 'assemble') return assemble(args);
  if (command === 'validate') {
    const result = await plan(args);
    process.stdout.write(`verified publication plan: ${result.entries.length} objects\n`);
    return;
  }
  if (command === 'publish') return publish(args);
  throw new Error('usage: release-cli.ts <config|stage|assemble|validate|publish> [options]');
}

main().catch((error: unknown) => {
  process.stderr.write(`release pipeline refused: ${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 1;
});
