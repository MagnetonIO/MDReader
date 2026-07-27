#!/usr/bin/env node

import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
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
import { pathToFileURL } from 'node:url';

import {
  verifyPackageBinarySet,
  type PackageBinary,
} from './binary-identity';
import type { ReleasePlatform } from '../../apps/web/lib/release-manifest';

const STABLE_VERSION = /^\d+\.\d+\.\d+$/;
const APPIMAGE_LAUNCHER =
  '#!/bin/sh\nexec "$APPDIR/usr/bin/mdreader-app" "$@"\n';
// Tauri CLI 2.11.4 installs this reviewed x86_64 AppRun implementation.
// It resolves the sole desktop entry's Exec command through AppDir/usr/bin.
const REVIEWED_APPRUN_SHA256 =
  'f30140a43a0a59e46db21bdefdf749b9e9f2c6946e92afabbacf98b8ae73fb4f';

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

export async function resolveReviewedPackageBinary(
  root: string,
  platform: ReleasePlatform,
  kind: string,
  expectedVersion: string,
): Promise<{
  body: Uint8Array;
  path: string;
  entrypoint: PackageBinary['entrypoint'];
}> {
  const inventory = await files(root);
  if (platform === 'macos') {
    const app = join(root, 'MDReader.app');
    const binaryPath = join(app, 'Contents', 'MacOS', 'mdreader-app');
    const plist = join(app, 'Contents', 'Info.plist');
    const matches = inventory.filter(
      (path) => basename(path) === 'mdreader-app',
    );
    if (matches.length !== 1 || matches[0] !== binaryPath) {
      throw new Error(
        'package does not bind its sole reviewed executable to MDReader.app/Contents/MacOS/mdreader-app',
      );
    }
    const plistValue = (key: string) =>
      execFileSync(
        'plutil',
        ['-extract', key, 'raw', '-o', '-', '--', plist],
        { encoding: 'utf8', timeout: 15_000, maxBuffer: 64 * 1024 },
      ).trim();
    if (
      plistValue('CFBundleIdentifier') !== 'dev.mdreader.app' ||
      plistValue('CFBundleExecutable') !== 'mdreader-app'
    ) {
      throw new Error('macOS bundle metadata does not name the reviewed MDReader entrypoint');
    }
    if (
      plistValue('CFBundleShortVersionString') !== expectedVersion ||
      plistValue('CFBundleVersion') !== expectedVersion
    ) {
      throw new Error(
        `macOS bundle version does not match release ${expectedVersion}`,
      );
    }
    return {
      body: await readFile(binaryPath),
      path: binaryPath,
      entrypoint: {
        contract: 'macos-bundle-executable',
        path: 'MDReader.app/Contents/MacOS/mdreader-app',
      },
    };
  }
  if (platform === 'windows') {
    const expectedName = 'mdreader-app.exe';
    const matches = inventory.filter((path) => basename(path) === expectedName);
    const allowed = new Set([
      join(root, expectedName),
      join(root, 'app-64bit', expectedName),
      join(root, '$PLUGINSDIR', 'app-64bit', expectedName),
    ]);
    if (matches.length !== 1 || !allowed.has(matches[0]!)) {
      throw new Error(
        `package contains ${matches.length} reviewed executables but does not bind one exact NSIS install payload to ${expectedName}`,
      );
    }
    const launchCandidates = inventory.filter(
      (path) =>
        path.toLowerCase().endsWith('.exe') &&
        (basename(path).toLowerCase().startsWith('mdreader') ||
          path.includes(`${sep}app-64bit${sep}`)),
    );
    if (launchCandidates.length !== 1 || launchCandidates[0] !== matches[0]) {
      throw new Error('NSIS package contains an extra unreviewed application launch target');
    }
    const contracts = inventory.filter(
      (path) => basename(path) === 'mdreader-package-contract',
    );
    if (contracts.length !== 1) {
      throw new Error(
        'NSIS package does not contain one compiled install and launch contract',
      );
    }
    const contract = (await readFile(contracts[0]!, 'utf8'))
      .replaceAll('\r\n', '\n')
      .trimEnd();
    const expectedContract =
      `schemaVersion=1;product=MDReader;identifier=dev.mdreader.app;` +
      `version=${expectedVersion};installTarget=mdreader-app.exe;` +
      'launchTarget=mdreader-app.exe';
    if (contract !== expectedContract) {
      throw new Error(
        'NSIS compiled install and launch contract does not match the reviewed release',
      );
    }
    return {
      body: await readFile(matches[0]!),
      path: matches[0]!,
      entrypoint: {
        contract: 'windows-nsis-install-target',
        path: expectedName,
      },
    };
  }
  const binaryPath = join(root, 'usr', 'bin', 'mdreader-app');
  const matches = inventory.filter((path) => basename(path) === 'mdreader-app');
  if (matches.length !== 1 || matches[0] !== binaryPath) {
    throw new Error(
      'package does not bind its sole reviewed executable to usr/bin/mdreader-app',
    );
  }
  if (kind === 'appimage' || kind === 'updater') {
    const appRun = join(root, 'AppRun');
    const desktop = inventory.filter((path) => path.endsWith('.desktop'));
    const appRunMetadata = await lstat(appRun);
    if (!appRunMetadata.isFile() || (appRunMetadata.mode & 0o111) === 0) {
      throw new Error('AppImage AppRun is not a regular executable launcher');
    }
    if (desktop.length !== 1) {
      throw new Error('AppImage must carry one desktop entry bound to AppRun');
    }
    const appRunBody = await readFile(appRun);
    const appRunSha256 = createHash('sha256').update(appRunBody).digest('hex');
    if (
      appRunBody.toString('utf8') !== APPIMAGE_LAUNCHER &&
      appRunSha256 !== REVIEWED_APPRUN_SHA256
    ) {
      throw new Error(
        'AppRun launcher does not dispatch exactly to usr/bin/mdreader-app',
      );
    }
    const desktopBody = await readFile(desktop[0]!, 'utf8');
    const desktopCommands = [...desktopBody.matchAll(/^Exec=([^\r\n]+)$/gm)];
    if (
      desktopCommands.length !== 1 ||
      !/^mdreader-app(?: +%[fFuU])?$/.test(desktopCommands[0]![1]!)
    ) {
      throw new Error('AppImage desktop entry does not dispatch to usr/bin/mdreader-app');
    }
    return {
      body: await readFile(binaryPath),
      path: binaryPath,
      entrypoint: {
        contract: 'linux-apprun-target',
        path: 'usr/bin/mdreader-app',
      },
    };
  }
  return {
    body: await readFile(binaryPath),
    path: binaryPath,
    entrypoint: {
      contract: 'linux-package-install-target',
      path: 'usr/bin/mdreader-app',
    },
  };
}

function authenticatePlatformSignature(
  platform: ReleasePlatform,
  path: string,
): { authenticated: boolean; identity: string | null } {
  if (platform === 'linux') return { authenticated: true, identity: null };
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
    const team = /^TeamIdentifier=([A-Z0-9]{10})$/m.exec(details)?.[1];
    if (!team) {
      throw new Error('final package macOS signature has no exact TeamIdentifier');
    }
    return { authenticated: true, identity: team };
  }
  const thumbprint = execFileSync(
    'pwsh',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      "& { param([string]$BinaryPath) $signature = Get-AuthenticodeSignature -LiteralPath $BinaryPath; if ($signature.Status -ne 'Valid') { throw \"invalid final package Authenticode status: $($signature.Status)\" }; if ($null -eq $signature.SignerCertificate) { throw 'final package has no Authenticode signer certificate' }; $signature.SignerCertificate.Thumbprint.ToUpperInvariant() }",
      path,
    ],
    { encoding: 'utf8', timeout: 30_000, maxBuffer: 64 * 1024 },
  ).trim();
  if (!/^[0-9A-F]{40}$/.test(thumbprint)) {
    throw new Error('final package Authenticode signer thumbprint is invalid');
  }
  return { authenticated: true, identity: thumbprint };
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
  const expectedVersion = option(args, '--version');
  if (!STABLE_VERSION.test(expectedVersion)) {
    throw new Error('expected package version must be an exact stable semantic version');
  }
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
  const expectedPublisherIdentity =
    platform === 'linux'
      ? null
      : option(args, '--expected-publisher-identity').toUpperCase();
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
    const binary = await resolveReviewedPackageBinary(
      root,
      platform,
      kind,
      expectedVersion,
    );
    const signature = authenticatePlatformSignature(platform, binary.path);
    packages.push({
      kind,
      binary: binary.body,
      artifact: await readFile(artifactPaths.get(kind)!),
      signatureAuthenticated: signature.authenticated,
      signatureIdentity: signature.identity,
      entrypoint: binary.entrypoint,
    });
  }
  const evidence = verifyPackageBinarySet(platform, packages, {
    target,
    sourceCommit,
    unsignedHandoffSha256,
    reviewedBinarySha256,
    reviewedBinary: reviewedBinaryBytes,
    expectedPublisherIdentity,
    expectedVersion,
  });
  await writeFile(output, `${JSON.stringify(evidence, null, 2)}\n`, {
    mode: 0o600,
  });
  process.stdout.write(
    `verified ${packages.length} final ${platform} packages contain reviewed MDReader executables\n`,
  );
}

if (
  process.argv[1] !== undefined &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  main().catch((error: unknown) => {
    process.stderr.write(
      `final package binary verification refused: ${
        error instanceof Error ? error.message : String(error)
      }\n`,
    );
    process.exitCode = 1;
  });
}
