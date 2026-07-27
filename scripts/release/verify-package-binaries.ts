#!/usr/bin/env node

import { execFileSync, spawnSync } from 'node:child_process';
import { lstat, readFile, readdir, realpath, writeFile } from 'node:fs/promises';
import {
  basename,
  dirname,
  isAbsolute,
  join,
  relative,
  resolve,
  sep,
} from 'node:path';

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

async function files(root: string, directory = root): Promise<string[]> {
  const result: string[] = [];
  const realRoot = await realpath(root);
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    const metadata = await lstat(path);
    if (metadata.isSymbolicLink()) {
      throw new Error(`package extraction contains a symbolic link: ${path}`);
    }
    if (metadata.isDirectory()) {
      result.push(...(await files(root, path)));
    } else if (metadata.isFile()) {
      const real = await realpath(path);
      const within = relative(realRoot, real);
      if (within === '..' || within.startsWith(`..${sep}`) || isAbsolute(within)) {
        throw new Error(`package extraction path escapes its root: ${path}`);
      }
      result.push(path);
    } else {
      throw new Error(`package extraction contains a special file: ${path}`);
    }
  }
  return result;
}

async function reviewedBinary(
  root: string,
  platform: ReleasePlatform,
): Promise<{ body: Uint8Array; path: string }> {
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
  return { body, path: matches[0]! };
}

function authenticatePlatformSignature(
  platform: ReleasePlatform,
  path: string,
): boolean {
  if (platform === 'linux') return true;
  if (platform === 'macos') {
    let appPath = dirname(path);
    while (!basename(appPath).endsWith('.app')) {
      const parent = dirname(appPath);
      if (parent === appPath) {
        throw new Error('final macOS package binary is not inside an app bundle');
      }
      appPath = parent;
    }
    execFileSync(
      'codesign',
      ['--verify', '--deep', '--strict', '--verbose=2', appPath],
      {
        stdio: 'pipe',
      },
    );
    const detailsResult = spawnSync(
      'codesign',
      ['--display', '--verbose=4', path],
      { encoding: 'utf8' },
    );
    const details = `${detailsResult.stdout ?? ''}${detailsResult.stderr ?? ''}`;
    if (
      detailsResult.status !== 0 ||
      !/^Authority=Developer ID Application:/m.test(details) ||
      !/^TeamIdentifier=[A-Z0-9]{10}$/m.test(details)
    ) {
      throw new Error(
        'final package macOS signature is not an authenticated Developer ID Application identity',
      );
    }
    execFileSync(
      'spctl',
      ['--assess', '--type', 'execute', '--verbose=2', appPath],
      { stdio: 'pipe' },
    );
    return true;
  }
  execFileSync(
    'pwsh',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      "& { param([string]$BinaryPath) $signature = Get-AuthenticodeSignature -LiteralPath $BinaryPath; if ($signature.Status -ne 'Valid') { throw \"invalid final package Authenticode status: $($signature.Status)\" } }",
      path,
    ],
    { stdio: 'pipe' },
  );
  return true;
}

function keyedArgs(args: string[], optionName: string): Map<string, string> {
  const output = new Map<string, string>();
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] !== optionName) continue;
    const value = args[index + 1];
    const separator = value?.indexOf('=');
    if (!value || separator === undefined || separator < 1) {
      throw new Error(`${optionName} input must be kind=path`);
    }
    const kind = value.slice(0, separator);
    if (output.has(kind)) {
      throw new Error(`${optionName} duplicates package kind ${kind}`);
    }
    output.set(kind, resolve(value.slice(separator + 1)));
  }
  return output;
}

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  const platform = option(args, '--platform') as ReleasePlatform;
  if (!['macos', 'windows', 'linux'].includes(platform)) {
    throw new Error('unsupported package verification platform');
  }
  const target = option(args, '--target');
  const sourceCommit = option(args, '--source-commit');
  const credentialedManifestPath = resolve(
    option(args, '--credentialed-manifest'),
  );
  const manifest = JSON.parse(
    await readFile(credentialedManifestPath, 'utf8'),
  ) as Record<string, unknown>;
  if (
    manifest.schemaVersion !== 2 ||
    manifest.platform !== platform ||
    manifest.target !== target ||
    manifest.sourceCommit !== sourceCommit ||
    typeof manifest.unsignedHandoffSha256 !== 'string' ||
    typeof manifest.reviewedBinarySha256 !== 'string' ||
    typeof manifest.reviewedBinaryPath !== 'string'
  ) {
    throw new Error(
      'credentialed manifest does not carry the exact reviewed executable and source identity',
    );
  }
  const unsignedHandoffSha256 = manifest.unsignedHandoffSha256;
  const reviewedBinarySha256 = manifest.reviewedBinarySha256;
  const reviewedBinaryPath = resolve(
    dirname(credentialedManifestPath),
    manifest.reviewedBinaryPath,
  );
  const manifestRoot = resolve(dirname(credentialedManifestPath));
  const reviewedWithin = relative(manifestRoot, reviewedBinaryPath);
  if (
    reviewedWithin === '..' ||
    reviewedWithin.startsWith(`..${sep}`) ||
    isAbsolute(reviewedWithin)
  ) {
    throw new Error('reviewed binary path escapes the credentialed output');
  }
  const reviewedBinaryBytes = await readFile(reviewedBinaryPath);
  const output = resolve(option(args, '--output'));
  const packageRoots = keyedArgs(args, '--package');
  const artifactPaths = keyedArgs(args, '--artifact');
  if (
    packageRoots.size !== artifactPaths.size ||
    [...packageRoots.keys()].some((kind) => !artifactPaths.has(kind))
  ) {
    throw new Error('every extracted package must have one exact final artifact');
  }
  const packages: PackageBinary[] = [];
  for (const [kind, root] of packageRoots) {
    const binary = await reviewedBinary(root, platform);
    packages.push({
      kind,
      binary: binary.body,
      artifact: await readFile(artifactPaths.get(kind)!),
      signatureAuthenticated: authenticatePlatformSignature(
        platform,
        binary.path,
      ),
    });
  }
  const evidence = verifyPackageBinarySet(platform, packages, {
    target,
    sourceCommit,
    unsignedHandoffSha256,
    reviewedBinarySha256,
    reviewedBinary: reviewedBinaryBytes,
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
