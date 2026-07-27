#!/usr/bin/env node

import { readFile, readdir, writeFile } from 'node:fs/promises';
import { basename, join, resolve } from 'node:path';

import {
  verifyPackageBinarySet,
  type PackageBinary,
} from './binary-identity';
import type { ReleasePlatform } from '../../apps/web/lib/release-manifest';

function option(args: string[], name: string): string {
  const index = args.indexOf(name);
  const value = index >= 0 ? args[index + 1] : undefined;
  if (!value || value.startsWith('--')) {
    throw new Error(`missing required option ${name}`);
  }
  return value;
}

async function files(root: string): Promise<string[]> {
  const result: string[] = [];
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const path = join(root, entry.name);
    if (entry.isDirectory()) result.push(...(await files(path)));
    else if (entry.isFile()) result.push(path);
  }
  return result;
}

async function reviewedBinary(
  root: string,
  platform: ReleasePlatform,
): Promise<Uint8Array> {
  const expectedName = platform === 'windows' ? 'mdreader-app.exe' : 'mdreader-app';
  const matches = (await files(root)).filter(
    (path) => basename(path) === expectedName,
  );
  if (matches.length !== 1) {
    throw new Error(
      `package extraction ${root} contains ${matches.length} ${expectedName} executables; expected exactly one`,
    );
  }
  const body = await readFile(matches[0]!);
  return body;
}

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const platform = option(args, '--platform') as ReleasePlatform;
  if (!['macos', 'windows', 'linux'].includes(platform)) {
    throw new Error('unsupported package verification platform');
  }
  const target = option(args, '--target');
  const sourceCommit = option(args, '--source-commit');
  const output = resolve(option(args, '--output'));
  const packages: PackageBinary[] = [];
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] !== '--package') continue;
    const value = args[index + 1];
    const separator = value?.indexOf('=');
    if (!value || separator === undefined || separator < 1) {
      throw new Error('package input must be kind=extracted-root');
    }
    const kind = value.slice(0, separator);
    const root = resolve(value.slice(separator + 1));
    packages.push({
      kind,
      binary: await reviewedBinary(root, platform),
    });
  }
  const evidence = verifyPackageBinarySet(platform, packages, {
    target,
    sourceCommit,
  });
  await writeFile(output, `${JSON.stringify(evidence, null, 2)}\n`, {
    mode: 0o600,
  });
  process.stdout.write(
    `verified ${packages.length} final ${platform} packages contain reviewed MDReader executables\n`,
  );
}

main().catch((error: unknown) => {
  process.stderr.write(
    `final package binary verification refused: ${
      error instanceof Error ? error.message : String(error)
    }\n`,
  );
  process.exitCode = 1;
});
