import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { readFileSync } from 'node:fs';
import {
  chmod,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rename,
  rm,
  symlink,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, relative, resolve, sep } from 'node:path';
import { promisify } from 'node:util';

import { afterEach, describe, expect, test } from 'vitest';
import { parse } from 'yaml';

const execFileAsync = promisify(execFile);
const repositoryRoot = resolve(process.cwd(), '..', '..');
const roots: string[] = [];
const SOURCE_COMMIT = '0123456789abcdef0123456789abcdef01234567';
const UPDATER_KEY_SHA256 = 'a'.repeat(64);
const CLI_INTEGRITY = 'sha512-' + Buffer.alloc(64, 1).toString('base64');
const NATIVE_INTEGRITY =
  'sha512-' + Buffer.alloc(64, 2).toString('base64');
const WINDOWS_SIGNER_SHA256 =
  'b0615fb72dfa2b22c0ce42733836297c012b93504c17a6f01f2ba874d4179e30';
const WINDOWS_NSIS_HOOK_SHA256 =
  'd47e8e70f688281a29eb8d23b18e245b59fec995ff3f196307e3fa5b17b475a3';
const SAFE_PACKAGE_EXTRACTOR_SHA256 =
  'cd47d9353b4905e50b15446b4acdeb6f672716bb52f64590923733921a72a9c8';
const PUBLIC_OAUTH_AUTHORITY = {
  schemaVersion: 1,
  googleDriveClientIdSha256: sha256(
    '123456789-example.apps.googleusercontent.com',
  ),
  dropboxClientIdSha256: sha256('dropbox-public-app-key'),
  dropboxRedirectUriSha256: sha256(
    'http://127.0.0.1:53682/oauth/dropbox',
  ),
};
const MACOS_ORT_AUTHORITY = JSON.parse(
  readFileSync(
    join(
      repositoryRoot,
      'scripts',
      'release',
      'macos-ort-runtime.json',
    ),
    'utf8',
  ),
) as {
  schemaVersion: number;
  version: string;
  source: Record<string, unknown>;
  toolchain: Record<string, unknown>;
  build: {
    architectures: string[];
    deploymentTarget: string;
  };
  dylibName: string;
  linkerAlias: string;
  linkerAliasTarget: string;
  installName: string;
  allowedDependencies: string[];
};
let nodeAttestationPromise:
  | Promise<{ path: string; mode: number; bytes: number; sha256: string }>
  | undefined;

interface FileEntry {
  path: string;
  mode: number;
  bytes: number;
  sha256: string;
}

type ContentEntry = Omit<FileEntry, 'mode'>;

interface ReleaseFixture {
  platform: 'macos' | 'windows';
  target: string;
  bundles: string;
  binary: string;
  nativePackage: string;
  executable: Buffer;
}

function sha256(body: Uint8Array | string): string {
  return createHash('sha256').update(body).digest('hex');
}

function releaseIdentity(): Buffer {
  return Buffer.from(
    [
      'MDREADER_RELEASE_IDENTITY_V1',
      'productName=MDReader',
      'bundleIdentifier=dev.mdreader.app',
      `sourceCommit=${SOURCE_COMMIT}`,
      '',
    ].join('\0'),
  );
}

function windowsExecutable(): Buffer {
  const header = 0x80;
  const coff = header + 4;
  const optional = coff + 20;
  const section = optional + 0xf0;
  const executable = Buffer.alloc(512);
  executable.write('MZ', 0, 'ascii');
  executable.writeUInt32LE(header, 0x3c);
  executable.write('PE\0\0', header, 'binary');
  executable.writeUInt16LE(0x8664, coff);
  executable.writeUInt16LE(1, coff + 2);
  executable.writeUInt16LE(0xf0, coff + 16);
  executable.writeUInt16LE(0x0002, coff + 18);
  executable.writeUInt16LE(0x20b, optional);
  executable.writeUInt32LE(0x20000020, section + 36);
  return Buffer.concat([executable, releaseIdentity()]);
}

function thinMachO(cpuType: number): Buffer {
  const identity = releaseIdentity();
  const executable = Buffer.alloc(40 + identity.byteLength);
  executable.writeUInt32LE(0xfeedfacf, 0);
  executable.writeUInt32LE(cpuType, 4);
  executable.writeUInt32LE(cpuType === 0x0100000c ? 0 : 3, 8);
  executable.writeUInt32LE(2, 12);
  executable.writeUInt32LE(1, 16);
  executable.writeUInt32LE(8, 20);
  executable.writeUInt32LE(0x1b, 32);
  executable.writeUInt32LE(8, 36);
  identity.copy(executable, 40);
  return executable;
}

function universalMachO(architectures: Array<'arm64' | 'x86_64'>): Buffer {
  const slices = architectures.map((architecture) => {
    const cpuType = architecture === 'arm64' ? 0x0100000c : 0x01000007;
    return { cpuType, body: thinMachO(cpuType) };
  });
  const headerBytes = 8 + slices.length * 20;
  const executable = Buffer.alloc(
    headerBytes + slices.reduce((total, slice) => total + slice.body.byteLength, 0),
  );
  executable.writeUInt32BE(0xcafebabe, 0);
  executable.writeUInt32BE(slices.length, 4);
  let offset = headerBytes;
  slices.forEach((slice, index) => {
    const entry = 8 + index * 20;
    executable.writeUInt32BE(slice.cpuType, entry);
    executable.writeUInt32BE(slice.cpuType === 0x0100000c ? 0 : 3, entry + 4);
    executable.writeUInt32BE(offset, entry + 8);
    executable.writeUInt32BE(slice.body.byteLength, entry + 12);
    executable.writeUInt32BE(0, entry + 16);
    slice.body.copy(executable, offset);
    offset += slice.body.byteLength;
  });
  return executable;
}

function thinDylibMachO(cpuType: number): Buffer {
  const installName = Buffer.from(
    '@rpath/libonnxruntime.1.24.2.dylib\0',
    'utf8',
  );
  const commandSize = Math.ceil((24 + installName.byteLength) / 8) * 8;
  const dylib = Buffer.alloc(32 + commandSize);
  dylib.writeUInt32LE(0xfeedfacf, 0);
  dylib.writeUInt32LE(cpuType, 4);
  dylib.writeUInt32LE(cpuType === 0x0100000c ? 0 : 3, 8);
  dylib.writeUInt32LE(6, 12);
  dylib.writeUInt32LE(1, 16);
  dylib.writeUInt32LE(commandSize, 20);
  dylib.writeUInt32LE(0x0d, 32);
  dylib.writeUInt32LE(commandSize, 36);
  dylib.writeUInt32LE(24, 40);
  dylib.writeUInt32LE(0, 44);
  dylib.writeUInt32LE(0x00010000, 48);
  dylib.writeUInt32LE(0x00010000, 52);
  installName.copy(dylib, 56);
  return dylib;
}

function universalDylib(): Buffer {
  const slices = [
    { cpuType: 0x0100000c, body: thinDylibMachO(0x0100000c) },
    { cpuType: 0x01000007, body: thinDylibMachO(0x01000007) },
  ];
  const headerBytes = 8 + slices.length * 20;
  const dylib = Buffer.alloc(
    headerBytes +
      slices.reduce((total, slice) => total + slice.body.byteLength, 0),
  );
  dylib.writeUInt32BE(0xcafebabe, 0);
  dylib.writeUInt32BE(slices.length, 4);
  let offset = headerBytes;
  slices.forEach((slice, index) => {
    const entry = 8 + index * 20;
    dylib.writeUInt32BE(slice.cpuType, entry);
    dylib.writeUInt32BE(slice.cpuType === 0x0100000c ? 0 : 3, entry + 4);
    dylib.writeUInt32BE(offset, entry + 8);
    dylib.writeUInt32BE(slice.body.byteLength, entry + 12);
    dylib.writeUInt32BE(0, entry + 16);
    slice.body.copy(dylib, offset);
    offset += slice.body.byteLength;
  });
  return dylib;
}

function windowsRelease(): ReleaseFixture {
  return {
    platform: 'windows',
    target: 'x86_64-pc-windows-msvc',
    bundles: 'nsis',
    binary: 'mdreader-app.exe',
    nativePackage: '@tauri-apps/cli-win32-x64-msvc',
    executable: windowsExecutable(),
  };
}

function macosRelease(
  architectures: Array<'arm64' | 'x86_64'> = ['arm64', 'x86_64'],
): ReleaseFixture {
  return {
    platform: 'macos',
    target: 'universal-apple-darwin',
    bundles: 'dmg',
    binary: 'mdreader-app',
    nativePackage: '@tauri-apps/cli-darwin-x64',
    executable: universalMachO(architectures),
  };
}

function nodeAttestation() {
  nodeAttestationPromise ??= Promise.all([
    readFile(process.execPath),
    lstat(process.execPath),
  ]).then(([body, metadata]) => ({
    path: process.execPath,
    mode: metadata.mode & 0o777,
    bytes: body.byteLength,
    sha256: sha256(body),
  }));
  return nodeAttestationPromise;
}

async function inventory(
  root: string,
  directory = root,
): Promise<FileEntry[]> {
  const output: FileEntry[] = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      output.push(...(await inventory(root, path)));
      continue;
    }
    if (!entry.isFile()) {
      throw new Error(`fixture entry is not regular: ${path}`);
    }
    const body = await readFile(path);
    const metadata = await lstat(path);
    output.push({
      path: relative(root, path).split(sep).join('/'),
      mode: metadata.mode & 0o777,
      bytes: body.byteLength,
      sha256: sha256(body),
    });
  }
  return output.sort((left, right) => left.path.localeCompare(right.path));
}

async function contentInventory(root: string): Promise<ContentEntry[]> {
  return (await inventory(root)).map(({ path, bytes, sha256 }) => ({
    path,
    bytes,
    sha256,
  }));
}

async function writeRegular(
  path: string,
  body: string | Uint8Array,
  mode = 0o644,
) {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, body);
  await chmod(path, mode);
}

function embeddedBoundary(): string {
  const workflow = parse(
    readFileSync(
      join(repositoryRoot, '.github', 'workflows', 'release.yml'),
      'utf8',
    ),
  ) as {
    jobs?: {
      'bundle-sign'?: {
        steps?: Array<{ id?: string; run?: string }>;
      };
    };
  };
  const body = workflow.jobs?.['bundle-sign']?.steps?.find(
    (step) => step.id === 'verify-credential-boundaries',
  )?.run;
  expect(body).toBeTypeOf('string');
  return body!;
}

// Kept independent of production inventory code so the boundary is tested
// against hand-derived manifests rather than against itself.
async function fixture(release = windowsRelease()) {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-release-handoff-'));
  roots.push(root);
  const unsignedDownload = join(root, 'unsigned-download');
  const unsignedTemp = join(unsignedDownload, 'unsigned-temp');
  const unsignedPayload = join(unsignedTemp, 'payload');
  const binary = join(
    unsignedPayload,
    'target',
    release.target,
    'release',
    release.binary,
  );
  await writeRegular(binary, release.executable, 0o755);
  for (const icon of [
    '32x32.png',
    '128x128.png',
    '128x128@2x.png',
    'icon.icns',
    'icon.ico',
  ]) {
    await writeRegular(join(unsignedPayload, 'icons', icon), `icon:${icon}`);
  }
  await writeRegular(
    join(
      unsignedPayload,
      'scripts',
      'release',
      'public-oauth-authority.json',
    ),
    `${JSON.stringify(PUBLIC_OAUTH_AUTHORITY, null, 2)}\n`,
  );
  let macosOrt: null | {
    authoritySha256: string;
    manifestSha256: string;
    dylibSha256: string;
  } = null;
  if (release.platform === 'macos') {
    const dylib = universalDylib();
    const authority = MACOS_ORT_AUTHORITY;
    const authorityBytes = `${JSON.stringify(authority, null, 2)}\n`;
    const runtime = {
      schemaVersion: 2,
      authoritySha256: sha256(authorityBytes),
      version: authority.version,
      source: authority.source,
      toolchain: authority.toolchain,
      build: authority.build,
      dylib: {
        path: authority.dylibName,
        bytes: dylib.byteLength,
        sha256: sha256(dylib),
        architectures: authority.build.architectures,
        installName: authority.installName,
        slices: authority.build.architectures.map((architecture) => ({
          architecture,
          bytes: 1,
          sha256: '1'.repeat(64),
        })),
        allowedDependencies: authority.allowedDependencies,
        deploymentTarget: authority.build.deploymentTarget,
      },
      linkerAlias: {
        path: authority.linkerAlias,
        target: authority.linkerAliasTarget,
      },
    };
    const runtimeBytes = `${JSON.stringify(runtime, null, 2)}\n`;
    await writeRegular(
      join(
        unsignedPayload,
        'scripts',
        'release',
        'macos-ort-runtime.json',
      ),
      authorityBytes,
    );
    await writeRegular(
      join(
        unsignedPayload,
        'scripts',
        'release',
        'macos-ort-runtime.sha256',
      ),
      `${sha256(authorityBytes)}\n`,
    );
    await writeRegular(
      join(unsignedPayload, 'runtime', 'macos', 'runtime.json'),
      runtimeBytes,
    );
    await writeRegular(
      join(
        unsignedPayload,
        'runtime',
        'macos',
        'libonnxruntime.1.24.2.dylib',
      ),
      dylib,
    );
    macosOrt = {
      authoritySha256: sha256(authorityBytes),
      manifestSha256: sha256(runtimeBytes),
      dylibSha256: sha256(dylib),
    };
  }
  const handoff = {
    schemaVersion: 3,
    version: '1.0.0',
    sourceCommit: SOURCE_COMMIT,
    platform: release.platform,
    target: release.target,
    bundles: release.bundles,
    updaterPublicKeySha256: UPDATER_KEY_SHA256,
    googleDriveClientIdSha256:
      PUBLIC_OAUTH_AUTHORITY.googleDriveClientIdSha256,
    dropboxClientIdSha256:
      PUBLIC_OAUTH_AUTHORITY.dropboxClientIdSha256,
    dropboxRedirectUriSha256:
      PUBLIC_OAUTH_AUTHORITY.dropboxRedirectUriSha256,
    macosOrt,
    files: await contentInventory(unsignedPayload),
  };
  const handoffBytes = `${JSON.stringify(handoff, null, 2)}\n`;
  await writeFile(join(unsignedTemp, 'handoff.json'), handoffBytes);
  const handoffSha256 = sha256(handoffBytes);
  const unsignedRoot = join(
    unsignedDownload,
    `mdreader-unsigned-${release.platform}-${handoffSha256}`,
  );
  await rename(unsignedTemp, unsignedRoot);
  for (const entry of handoff.files) {
    await chmod(join(unsignedRoot, 'payload', entry.path), 0o644);
  }

  const supportDownload = join(root, 'support-download');
  const supportRoot = join(
    supportDownload,
    `mdreader-release-support-${'b'.repeat(64)}`,
  );
  const supportPayload = join(supportRoot, 'payload');
  await mkdir(supportPayload, { recursive: true });
  await writeFile(
    join(supportPayload, 'sign-windows-from-store.ps1'),
    await readFile(
      join(
        repositoryRoot,
        'scripts',
        'release',
        'sign-windows-from-store.ps1',
      ),
    ),
  );
  await writeFile(
    join(supportPayload, 'windows-minimum.nsh'),
    await readFile(
      join(repositoryRoot, 'scripts', 'release', 'windows-minimum.nsh'),
    ),
  );
  await writeFile(
    join(supportPayload, 'safe-package-extract.mjs'),
    await readFile(
      join(repositoryRoot, 'scripts', 'release', 'safe-package-extract.mjs'),
    ),
  );
  const support = {
    schemaVersion: 1,
    sourceCommit: SOURCE_COMMIT,
    files: await contentInventory(supportPayload),
  };
  const supportBytes = `${JSON.stringify(support, null, 2)}\n`;
  await writeFile(join(supportRoot, 'support.json'), supportBytes);
  const supportSha256 = sha256(supportBytes);
  const finalSupportRoot = join(
    supportDownload,
    `mdreader-release-support-${supportSha256}`,
  );
  await rename(supportRoot, finalSupportRoot);
  for (const entry of support.files) {
    await chmod(join(finalSupportRoot, 'payload', entry.path), 0o644);
  }

  const runtimeRoot = join(root, 'runtime');
  await writeRegular(
    join(runtimeRoot, 'node_modules', '@tauri-apps', 'cli', 'package.json'),
    JSON.stringify({ name: '@tauri-apps/cli', version: '2.11.4' }),
  );
  for (const file of ['tauri.js', 'main.js', 'index.js']) {
    await writeRegular(
      join(runtimeRoot, 'node_modules', '@tauri-apps', 'cli', file),
      `cli:${file}`,
    );
  }
  await writeRegular(
    join(
      runtimeRoot,
      'node_modules',
      '@tauri-apps',
      release.nativePackage.slice('@tauri-apps/'.length),
      'package.json',
    ),
    JSON.stringify({ name: release.nativePackage, version: '2.11.4' }),
  );
  await writeRegular(
    join(
      runtimeRoot,
      'node_modules',
      '@tauri-apps',
      release.nativePackage.slice('@tauri-apps/'.length),
      `cli.${release.nativePackage.slice('@tauri-apps/cli-'.length)}.node`,
    ),
    'native-binding',
  );
  const runtime = {
    schemaVersion: 1,
    platform: release.platform,
    cliVersion: '2.11.4',
    cliPackage: '@tauri-apps/cli',
    cliIntegrity: CLI_INTEGRITY,
    nativePackage: release.nativePackage,
    nativeIntegrity: NATIVE_INTEGRITY,
    node: await nodeAttestation(),
    files: await inventory(runtimeRoot),
  };
  const runtimeBytes = `${JSON.stringify(runtime, null, 2)}\n`;
  await writeFile(join(runtimeRoot, 'runtime.json'), runtimeBytes);
  const runtimeSha256 = sha256(runtimeBytes);

  return {
    root,
    unsignedDownload,
    unsignedRoot,
    supportDownload,
    supportRoot: finalSupportRoot,
    supportSha256,
    runtimeRoot,
    runtimeSha256,
    bundleRoot: join(root, 'bundle-workspace'),
    release,
    macosOrt,
  };
}

async function runBoundary(input: Awaited<ReturnType<typeof fixture>>) {
  const script = join(input.root, 'verify-boundaries.sh');
  await writeFile(script, embeddedBoundary());
  return execFileAsync('bash', [script], {
    env: {
      ...process.env,
      UNSIGNED_DOWNLOAD_ROOT: input.unsignedDownload,
      SUPPORT_DOWNLOAD_ROOT: input.supportDownload,
      TAURI_RUNTIME_ROOT: input.runtimeRoot,
      BUNDLE_WORKSPACE_ROOT: input.bundleRoot,
      EXPECTED_SUPPORT_SHA256: input.supportSha256,
      EXPECTED_WINDOWS_SIGNER_SHA256: WINDOWS_SIGNER_SHA256,
      EXPECTED_WINDOWS_NSIS_HOOK_SHA256: WINDOWS_NSIS_HOOK_SHA256,
      EXPECTED_SAFE_PACKAGE_EXTRACTOR_SHA256:
        SAFE_PACKAGE_EXTRACTOR_SHA256,
      EXPECTED_RELEASE_VERSION: '1.0.0',
      EXPECTED_SOURCE_COMMIT: SOURCE_COMMIT,
      EXPECTED_RELEASE_PLATFORM: input.release.platform,
      EXPECTED_RELEASE_TARGET: input.release.target,
      EXPECTED_RELEASE_BUNDLES: input.release.bundles,
      EXPECTED_RELEASE_BINARY: input.release.binary,
      EXPECTED_UPDATER_PUBLIC_KEY_SHA256: UPDATER_KEY_SHA256,
      EXPECTED_GOOGLE_DRIVE_CLIENT_ID_SHA256:
        PUBLIC_OAUTH_AUTHORITY.googleDriveClientIdSha256,
      EXPECTED_DROPBOX_CLIENT_ID_SHA256:
        PUBLIC_OAUTH_AUTHORITY.dropboxClientIdSha256,
      EXPECTED_DROPBOX_REDIRECT_URI_SHA256:
        PUBLIC_OAUTH_AUTHORITY.dropboxRedirectUriSha256,
      EXPECTED_MACOS_ORT_AUTHORITY_SHA256:
        input.macosOrt?.authoritySha256 ?? 'unused-on-non-macos',
      EXPECTED_TAURI_CLI_INTEGRITY: CLI_INTEGRITY,
      EXPECTED_TAURI_NATIVE_PACKAGE: input.release.nativePackage,
      EXPECTED_TAURI_NATIVE_INTEGRITY: NATIVE_INTEGRITY,
      EXPECTED_RUNTIME_MANIFEST_SHA256: input.runtimeSha256,
      GITHUB_OUTPUT: join(input.root, 'github-output.txt'),
    },
  });
}

afterEach(async () => {
  await Promise.all(
    roots.splice(0).map((root) => rm(root, { recursive: true, force: true })),
  );
});

describe('closed unsigned handoff and isolated Tauri runtime', () => {
  test('accepts only the exact hash-addressed unsigned binary/resources, reviewed support, and inventoried runtime', async () => {
    const input = await fixture();
    await expect(runBoundary(input)).resolves.toMatchObject({ stderr: '' });
    const output = await readFile(
      join(input.root, 'github-output.txt'),
      'utf8',
    );
    expect(output).toContain(
      `windows_signer_path=${join(
        input.bundleRoot,
        'scripts',
        'release',
        'sign-windows-from-store.ps1',
      )}`,
    );
    expect(
      (await lstat(
        join(
          input.bundleRoot,
          'target',
          input.release.target,
          'release',
          input.release.binary,
        ),
      )).mode & 0o777,
    ).toBe(0o755);
  });

  test('rejects a self-sealed public OAuth authority that differs from the handoff identity', async () => {
    const input = await fixture();
    const oldRoot = input.unsignedRoot;
    const payload = join(oldRoot, 'payload');
    await writeFile(
      join(
        payload,
        'scripts',
        'release',
        'public-oauth-authority.json',
      ),
      `${JSON.stringify(
        {
          ...PUBLIC_OAUTH_AUTHORITY,
          dropboxClientIdSha256: 'f'.repeat(64),
        },
        null,
        2,
      )}\n`,
    );
    const manifestPath = join(oldRoot, 'handoff.json');
    const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
    manifest.files = await contentInventory(payload);
    const body = `${JSON.stringify(manifest, null, 2)}\n`;
    await writeFile(manifestPath, body);
    const nextRoot = join(
      dirname(oldRoot),
      `mdreader-unsigned-${input.release.platform}-${sha256(body)}`,
    );
    await rename(oldRoot, nextRoot);
    input.unsignedRoot = nextRoot;

    await expect(runBoundary(input)).rejects.toThrow(
      /public OAuth authority.*handoff identity/i,
    );
  });

  test.each([
    'payload/scripts/release/bundle-direct-release.sh',
    'payload/node_modules/@tauri-apps/cli/tauri.js',
  ])('rejects build-side executable content even when its hash is added to the handoff manifest: %s', async (path) => {
    const input = await fixture();
    const oldRoot = input.unsignedRoot;
    await writeRegular(join(oldRoot, path), 'credential-stealing replacement', 0o755);
    const manifestPath = join(oldRoot, 'handoff.json');
    const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
    manifest.files = await contentInventory(join(oldRoot, 'payload'));
    const body = `${JSON.stringify(manifest, null, 2)}\n`;
    await writeFile(manifestPath, body);
    const nextRoot = join(
      dirname(oldRoot),
      `mdreader-unsigned-${input.release.platform}-${sha256(body)}`,
    );
    await rename(oldRoot, nextRoot);
    input.unsignedRoot = nextRoot;

    await expect(runBoundary(input)).rejects.toThrow(
      /closed unsigned handoff|unexpected handoff path/i,
    );
  });

  test.each([
    [
      'sign-windows-from-store.ps1',
      /approved Windows signer.*SHA-256/i,
    ],
    ['windows-minimum.nsh', /approved Windows NSIS hook.*SHA-256/i],
    [
      'safe-package-extract.mjs',
      /approved Windows package extractor.*SHA-256/i,
    ],
  ])(
    'rejects self-sealed unapproved support bytes before credentials: %s',
    async (name, error) => {
      const input = await fixture();
      const oldRoot = input.supportRoot;
      const payload = join(oldRoot, 'payload');
      await writeFile(
        join(payload, name),
        'arbitrary checkout-controlled replacement',
      );
      const manifestPath = join(oldRoot, 'support.json');
      const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
      manifest.files = await contentInventory(payload);
      const body = `${JSON.stringify(manifest, null, 2)}\n`;
      await writeFile(manifestPath, body);
      const nextSha256 = sha256(body);
      const nextRoot = join(
        dirname(oldRoot),
        `mdreader-release-support-${nextSha256}`,
      );
      await rename(oldRoot, nextRoot);
      input.supportRoot = nextRoot;
      input.supportSha256 = nextSha256;

      await expect(runBoundary(input)).rejects.toThrow(error);
    },
  );

  test('rejects a handoff whose binary bytes no longer match its manifest', async () => {
    const input = await fixture();
    await writeFile(
      join(
        input.unsignedRoot,
        'payload',
        'target',
        input.release.target,
        'release',
        input.release.binary,
      ),
      'replaced-binary',
    );

    await expect(runBoundary(input)).rejects.toThrow(
      /unsigned handoff.*hash|inventory mismatch/i,
    );
  });

  test('rejects a self-sealed shell script substituted for the reviewed executable', async () => {
    const input = await fixture();
    const oldRoot = input.unsignedRoot;
    await writeRegular(
      join(
        oldRoot,
        'payload',
        'target',
        input.release.target,
        'release',
        input.release.binary,
      ),
      `#!/usr/bin/env bash\n# ${releaseIdentity().toString('binary')}\necho placeholder\n`,
      0o755,
    );
    const manifestPath = join(oldRoot, 'handoff.json');
    const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
    manifest.files = await contentInventory(join(oldRoot, 'payload'));
    const body = `${JSON.stringify(manifest, null, 2)}\n`;
    await writeFile(manifestPath, body);
    const nextRoot = join(
      dirname(oldRoot),
      `mdreader-unsigned-${input.release.platform}-${sha256(body)}`,
    );
    await rename(oldRoot, nextRoot);
    input.unsignedRoot = nextRoot;

    await expect(runBoundary(input)).rejects.toThrow(
      /executable format|PE|Mach-O|ELF/i,
    );
  });

  test('rejects a self-sealed executable with the wrong source identity', async () => {
    const input = await fixture();
    const oldRoot = input.unsignedRoot;
    const executable = Buffer.from(input.release.executable);
    const sourceOffset = executable.indexOf(Buffer.from(SOURCE_COMMIT));
    expect(sourceOffset).toBeGreaterThan(-1);
    executable.write('f'.repeat(40), sourceOffset, 'ascii');
    await writeRegular(
      join(
        oldRoot,
        'payload',
        'target',
        input.release.target,
        'release',
        input.release.binary,
      ),
      executable,
      0o755,
    );
    const manifestPath = join(oldRoot, 'handoff.json');
    const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
    manifest.files = await contentInventory(join(oldRoot, 'payload'));
    const body = `${JSON.stringify(manifest, null, 2)}\n`;
    await writeFile(manifestPath, body);
    const nextRoot = join(
      dirname(oldRoot),
      `mdreader-unsigned-${input.release.platform}-${sha256(body)}`,
    );
    await rename(oldRoot, nextRoot);
    input.unsignedRoot = nextRoot;

    await expect(runBoundary(input)).rejects.toThrow(
      /wrong MDReader product, bundle, or source identity/i,
    );
  });

  test('rejects a universal macOS handoff with its arm64 slice removed', async () => {
    const input = await fixture(macosRelease());
    const oldRoot = input.unsignedRoot;
    await writeRegular(
      join(
        oldRoot,
        'payload',
        'target',
        input.release.target,
        'release',
        input.release.binary,
      ),
      universalMachO(['x86_64']),
      0o755,
    );
    const manifestPath = join(oldRoot, 'handoff.json');
    const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
    manifest.files = await contentInventory(join(oldRoot, 'payload'));
    const body = `${JSON.stringify(manifest, null, 2)}\n`;
    await writeFile(manifestPath, body);
    const nextRoot = join(
      dirname(oldRoot),
      `mdreader-unsigned-${input.release.platform}-${sha256(body)}`,
    );
    await rename(oldRoot, nextRoot);
    input.unsignedRoot = nextRoot;

    await expect(runBoundary(input)).rejects.toThrow(
      /universal Mach-O.*arm64.*x86_64|Mach-O slices/i,
    );
  });

  test('rejects a symlink substituted for an inventoried resource', async () => {
    const input = await fixture();
    const icon = join(
      input.unsignedRoot,
      'payload',
      'icons',
      '32x32.png',
    );
    await rm(icon);
    await symlink('128x128.png', icon);

    await expect(runBoundary(input)).rejects.toThrow(
      /symbolic link|regular file|unsupported handoff entry/i,
    );
  });

  test('rejects a changed package-local Tauri runtime byte', async () => {
    const input = await fixture();
    await writeFile(
      join(
        input.runtimeRoot,
        'node_modules',
        '@tauri-apps',
        'cli',
        'tauri.js',
      ),
      'changed runtime',
    );

    await expect(runBoundary(input)).rejects.toThrow(
      /Tauri runtime.*inventory|runtime.*hash/i,
    );
  });
});
