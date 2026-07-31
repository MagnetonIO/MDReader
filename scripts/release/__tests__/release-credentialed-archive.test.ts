import { createHash } from 'node:crypto';
import { execFile, spawnSync } from 'node:child_process';
import {
  chmod,
  link,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  readlink,
  readdir,
  rename,
  rm,
  symlink,
  truncate,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, relative, resolve, sep } from 'node:path';
import { promisify } from 'node:util';

import { afterEach, describe, expect, test } from 'vitest';

import {
  createReleaseConfig,
  updaterPublicKeyFingerprint,
} from '../release-config';
import { reviewedExecutable } from './release-binary-fixtures';

const execFileAsync = promisify(execFile);
const repositoryRoot = resolve(process.cwd(), '..', '..');
const roots: string[] = [];
const SOURCE_COMMIT = '0123456789abcdef0123456789abcdef01234567';
const UPDATER_KEY = 'A'.repeat(88);
const TAR_COMMANDS = [
  'tar',
  ...(spawnSync('gtar', ['--version']).status === 0 ? ['gtar'] : []),
];

function sha256(body: Uint8Array): string {
  return createHash('sha256').update(body).digest('hex');
}

type Platform = 'macos' | 'linux';

const PLATFORMS = {
  macos: {
    target: 'universal-apple-darwin',
    bundles: 'dmg',
    bundlePath: 'bundle/macos/MDReader.app/Contents/MacOS/mdreader-app',
  },
  linux: {
    target: 'x86_64-unknown-linux-gnu',
    bundles: 'appimage,deb,rpm',
    bundlePath: 'bundle/appimage/MDReader.AppImage',
  },
} as const;

async function fixture(
  options: {
    platform?: Platform;
    additionalFiles?: Array<{ path: string; body: string; mode?: number }>;
    hardlinks?: Array<{ path: string; target: string }>;
    symlinks?: Array<{ path: string; target: string }>;
    tarCommand?: string;
  } = {},
) {
  const platform = options.platform ?? 'linux';
  const spec = PLATFORMS[platform];
  const root = await mkdtemp(join(tmpdir(), 'mdreader-credentialed-archive-'));
  roots.push(root);
  const content = join(root, 'content');
  const artifact = join(root, 'artifact');
  const download = join(root, 'download');
  const output = join(root, 'mdreader-credentialed-output');
  const configPath = join(content, 'release-config.json');
  const bundlePath = join(content, spec.bundlePath);
  const reviewedBinaryRelativePath = 'reviewed-binary/mdreader-app';
  const reviewedBinaryPath = join(content, reviewedBinaryRelativePath);
  await mkdir(dirname(bundlePath), { recursive: true });
  await mkdir(dirname(reviewedBinaryPath), { recursive: true });
  await writeFile(
    configPath,
    `${JSON.stringify(
      createReleaseConfig({
        version: '1.0.0',
        workspaceVersion: '1.0.0',
        bundleIdentifier: 'dev.mdreader.app',
        updaterPublicKey: UPDATER_KEY,
        updaterEndpoint:
          'https://downloads.mdreader.dev/channels/stable/updater.json',
        macosSigningIdentity:
          platform === 'macos'
            ? 'Developer ID Application: Release Archive Test'
            : undefined,
        macosFramework:
          platform === 'macos'
            ? '/reviewed/runtime/libonnxruntime.1.24.2.dylib'
            : undefined,
      }),
      null,
      2,
    )}\n`,
  );
  await writeFile(bundlePath, 'credentialed artifact');
  await writeFile(
    reviewedBinaryPath,
    reviewedExecutable(platform, SOURCE_COMMIT),
  );
  await chmod(bundlePath, 0o755);
  for (const file of options.additionalFiles ?? []) {
    const path = join(content, file.path);
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, file.body);
    await chmod(path, file.mode ?? 0o644);
  }
  for (const link of options.symlinks ?? []) {
    const path = join(content, link.path);
    await mkdir(dirname(path), { recursive: true });
    await symlink(link.target, path);
  }
  for (const hardlink of options.hardlinks ?? []) {
    const path = join(content, hardlink.path);
    await mkdir(dirname(path), { recursive: true });
    await link(join(content, hardlink.target), path);
  }
  const files: Array<{
    path: string;
    kind: 'file' | 'symlink';
    mode: number;
    bytes: number;
    sha256: string;
  }> = [];
  async function walk(directory: string): Promise<void> {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) {
        await walk(path);
        continue;
      }
      const metadata = await lstat(path);
      const body = entry.isSymbolicLink()
        ? Buffer.from(await readlink(path), 'utf8')
        : await readFile(path);
      files.push({
        path: relative(content, path).split(sep).join('/'),
        kind: entry.isSymbolicLink() ? 'symlink' : 'file',
        mode: metadata.mode & 0o777,
        bytes: body.byteLength,
        sha256: sha256(body),
      });
    }
  }
  await walk(content);
  files.sort((left, right) => left.path.localeCompare(right.path));
  await writeFile(
    join(content, 'credentialed-output.json'),
    `${JSON.stringify(
      {
        schemaVersion: 2,
        version: '1.0.0',
        sourceCommit: SOURCE_COMMIT,
        platform,
        target: spec.target,
        bundles: spec.bundles,
        unsignedHandoffSha256: 'a'.repeat(64),
        reviewedBinaryPath: reviewedBinaryRelativePath,
        reviewedBinarySha256: files.find(
          (entry) => entry.path === reviewedBinaryRelativePath,
        )!.sha256,
        updaterPublicKeySha256: updaterPublicKeyFingerprint(UPDATER_KEY),
        releaseConfigSha256: files.find(
          (entry) => entry.path === 'release-config.json',
        )!.sha256,
        files,
      },
      null,
      2,
    )}\n`,
  );
  await mkdir(artifact);
  const archive = join(artifact, 'credentialed-output.tar');
  await execFileAsync(options.tarCommand ?? 'tar', [
    '--format=ustar',
    '-cf',
    archive,
    '-C',
    content,
    'credentialed-output.json',
    'release-config.json',
    'reviewed-binary',
    'bundle',
  ]);
  const archiveSha256 = sha256(await readFile(archive));
  await mkdir(download);
  const artifactRoot = join(
    download,
    `mdreader-bundled-${platform}-${archiveSha256}`,
  );
  await rename(artifact, artifactRoot);
  return {
    root,
    content,
    download,
    output,
    platform,
    target: spec.target,
    bundles: spec.bundles,
    bundlePath: spec.bundlePath,
    artifactRoot,
    archive: join(artifactRoot, 'credentialed-output.tar'),
  };
}

async function reseal(input: Awaited<ReturnType<typeof fixture>>) {
  const nextSha256 = sha256(await readFile(input.archive));
  const nextArtifactRoot = join(
    dirname(input.artifactRoot),
    `mdreader-bundled-${input.platform}-${nextSha256}`,
  );
  await rename(input.artifactRoot, nextArtifactRoot);
  input.artifactRoot = nextArtifactRoot;
  input.archive = join(nextArtifactRoot, 'credentialed-output.tar');
}

async function rebuildArchive(
  input: Awaited<ReturnType<typeof fixture>>,
): Promise<void> {
  await execFileAsync('tar', [
    '--format=ustar',
    '-cf',
    input.archive,
    '-C',
    input.content,
    'credentialed-output.json',
    'release-config.json',
    'reviewed-binary',
    'bundle',
  ]);
  await reseal(input);
}

async function rewriteCredentialedManifest(
  input: Awaited<ReturnType<typeof fixture>>,
  rewrite:
    | string
    | ((manifest: Record<string, unknown>) => Record<string, unknown>),
): Promise<void> {
  const manifestPath = join(input.content, 'credentialed-output.json');
  if (typeof rewrite === 'string') {
    await writeFile(manifestPath, rewrite);
  } else {
    const manifest = JSON.parse(
      await readFile(manifestPath, 'utf8'),
    ) as Record<string, unknown>;
    await writeFile(
      manifestPath,
      `${JSON.stringify(rewrite(manifest), null, 2)}\n`,
    );
  }
  await rebuildArchive(input);
}

function writeTarString(
  header: Buffer,
  offset: number,
  length: number,
  value: string,
) {
  const encoded = Buffer.from(value, 'utf8');
  if (encoded.byteLength > length) {
    throw new Error(`test tar field is too long: ${value}`);
  }
  encoded.copy(header, offset);
}

function writeTarOctal(
  header: Buffer,
  offset: number,
  length: number,
  value: number,
) {
  writeTarString(
    header,
    offset,
    length,
    `${value.toString(8).padStart(length - 1, '0')}\0`,
  );
}

function rawTarEntry({
  name,
  type = '0',
  body = Buffer.alloc(0),
  linkname = '',
  mode = type === '5' ? 0o755 : 0o644,
}: {
  name: string;
  type?: string;
  body?: Buffer;
  linkname?: string;
  mode?: number;
}): Buffer {
  const header = Buffer.alloc(512);
  writeTarString(header, 0, 100, name);
  writeTarOctal(header, 100, 8, mode);
  writeTarOctal(header, 108, 8, 0);
  writeTarOctal(header, 116, 8, 0);
  writeTarOctal(header, 124, 12, body.byteLength);
  writeTarOctal(header, 136, 12, 0);
  header.fill(0x20, 148, 156);
  writeTarString(header, 156, 1, type);
  writeTarString(header, 157, 100, linkname);
  writeTarString(header, 257, 6, 'ustar');
  writeTarString(header, 263, 2, '00');
  const checksum = header.reduce((total, byte) => total + byte, 0);
  writeTarString(
    header,
    148,
    8,
    `${checksum.toString(8).padStart(6, '0')}\0 `,
  );
  const padding = Buffer.alloc((512 - (body.byteLength % 512)) % 512);
  return Buffer.concat([header, body, padding]);
}

function tarPayloadLength(archive: Buffer): number {
  let offset = 0;
  while (offset + 512 <= archive.byteLength) {
    const header = archive.subarray(offset, offset + 512);
    if (header.every((byte) => byte === 0)) return offset;
    const encodedSize = header
      .subarray(124, 136)
      .toString('ascii')
      .replace(/\0.*$/, '')
      .trim();
    const size = Number.parseInt(encodedSize || '0', 8);
    offset += 512 + Math.ceil(size / 512) * 512;
  }
  throw new Error('test archive has no end marker');
}

async function appendRawEntry(
  input: Awaited<ReturnType<typeof fixture>>,
  entry: Parameters<typeof rawTarEntry>[0],
) {
  const archive = await readFile(input.archive);
  await writeFile(
    input.archive,
    Buffer.concat([
      archive.subarray(0, tarPayloadLength(archive)),
      rawTarEntry(entry),
      Buffer.alloc(1024),
    ]),
  );
  await reseal(input);
}

async function markOutput(input: Awaited<ReturnType<typeof fixture>>) {
  await mkdir(input.output, { recursive: true });
  const marker = join(input.output, 'must-survive-rejected-archive');
  await writeFile(marker, 'untouched');
  return marker;
}

async function verify(
  input: Awaited<ReturnType<typeof fixture>>,
  runnerTemp = input.root,
) {
  return execFileAsync(
    'bash',
    [
      'scripts/release/verify-credentialed-output.sh',
      input.platform,
      '1.0.0',
      SOURCE_COMMIT,
      input.target,
      input.bundles,
      input.download,
      input.output,
      updaterPublicKeyFingerprint(UPDATER_KEY),
    ],
    {
      cwd: repositoryRoot,
      env: { ...process.env, RUNNER_TEMP: runnerTemp },
    },
  );
}

afterEach(async () => {
  await Promise.all(
    roots.splice(0).map((root) => rm(root, { recursive: true, force: true })),
  );
});

describe('hash-addressed credentialed archive handoff', () => {
  test.each(TAR_COMMANDS)(
    'restores the exact %s ustar bundle before content verification',
    async (tarCommand) => {
      const input = await fixture({ tarCommand });

      await expect(verify(input)).resolves.toMatchObject({
        stdout: expect.stringContaining('credentialed release output verified'),
        stderr: '',
      });
      expect(
        (await lstat(
          join(input.output, 'bundle', 'appimage', 'MDReader.AppImage'),
        )).mode & 0o777,
      ).toBe(0o755);
    },
  );

  test('preserves a legitimate relative macOS bundle symlink and executable mode', async () => {
    const frameworkRoot =
      'bundle/macos/MDReader.app/Contents/Frameworks/Example.framework';
    const input = await fixture({
      platform: 'macos',
      additionalFiles: [
        {
          path: `${frameworkRoot}/Versions/A/Example`,
          body: 'signed framework',
          mode: 0o755,
        },
        {
          path: `${frameworkRoot}/Versions/A/Resources/Info.plist`,
          body: 'framework resources',
        },
      ],
      symlinks: [
        {
          path: `${frameworkRoot}/Versions/Current`,
          target: 'A',
        },
        {
          path: `${frameworkRoot}/Resources`,
          target: 'Versions/Current/Resources',
        },
        {
          path: `${frameworkRoot}/Example`,
          target: 'Versions/Current/Example',
        },
      ],
    });

    await expect(verify(input)).resolves.toMatchObject({
      stdout: expect.stringContaining('credentialed release output verified'),
      stderr: '',
    });
    const current = join(input.output, frameworkRoot, 'Versions', 'Current');
    expect((await lstat(current)).isSymbolicLink()).toBe(true);
    expect(await readlink(current)).toBe('A');
    expect(await readlink(join(input.output, frameworkRoot, 'Resources'))).toBe(
      'Versions/Current/Resources',
    );
    expect(await readlink(join(input.output, frameworkRoot, 'Example'))).toBe(
      'Versions/Current/Example',
    );
    expect(
      (await lstat(join(input.output, frameworkRoot, 'Versions', 'A', 'Example')))
        .mode & 0o777,
    ).toBe(0o755);
  });

  test('refuses to publish outside the trusted runner temp directory', async () => {
    const input = await fixture();
    const trustedRunnerTemp = join(input.root, 'trusted-runner-temp');
    await mkdir(trustedRunnerTemp);

    await expect(verify(input, trustedRunnerTemp)).rejects.toThrow(
      /output root must be the dedicated path in RUNNER_TEMP/i,
    );
    await expect(lstat(input.output)).rejects.toMatchObject({ code: 'ENOENT' });
  });

  test.each([
    ['/outside-the-closed-output', /absolute symbolic-link target/i],
    ['../../outside-the-closed-output', /escapes the closed bundle root/i],
  ])('rejects an unsafe credentialed archive symlink target: %s', async (target, error) => {
    const input = await fixture({
      symlinks: [{ path: 'bundle/escape-link', target }],
    });
    const marker = await markOutput(input);

    await expect(verify(input)).rejects.toThrow(error);
    await expect(readFile(marker, 'utf8')).resolves.toBe('untouched');
  });

  test('rejects duplicate archive members before extraction', async () => {
    const input = await fixture();
    await execFileAsync('tar', [
      '-rf',
      input.archive,
      '-C',
      input.content,
      'release-config.json',
    ]);
    await reseal(input);

    await expect(verify(input)).rejects.toThrow(
      /duplicate (?:canonical )?archive entry/i,
    );
  });

  test.each(TAR_COMMANDS)(
    'rejects a nested-dot extraction alias produced by %s before materialization',
    async (tarCommand) => {
      const input = await fixture({
        additionalFiles: [
          { path: 'bundle/alias-target', body: 'authorized body' },
        ],
        tarCommand,
      });
      await execFileAsync(tarCommand, [
        '-rf',
        input.archive,
        '-C',
        input.content,
        'bundle/./alias-target',
      ]);
      await reseal(input);
      const marker = await markOutput(input);

      await expect(verify(input)).rejects.toThrow(
        /dot path segment|duplicate canonical archive entry/i,
      );
      await expect(readFile(marker, 'utf8')).resolves.toBe('untouched');
    },
  );

  test('rejects a real in-bundle hard link before it is collapsed to an ordinary file', async () => {
    const input = await fixture({
      additionalFiles: [{ path: 'bundle/base', body: 'shared bytes' }],
      hardlinks: [{ path: 'bundle/hard', target: 'bundle/base' }],
    });
    const marker = await markOutput(input);

    await expect(verify(input)).rejects.toThrow(/hard link/i);
    await expect(readFile(marker, 'utf8')).resolves.toBe('untouched');
  });

  test.each([
    ['bundle/CaseAlias', '0', /Windows case-folding collision/i],
    ['bundle/con.txt', '0', /Windows reserved name/i],
    ['bundle/trailing.', '0', /trailing dot or space/i],
    ['bundle/trailing ', '0', /trailing dot or space/i],
    ['bundle/colon:name', '0', /Windows-forbidden character/i],
    ['bundle/../release-config.json', '0', /parent path segment/i],
    ['bundle//empty-segment', '0', /empty path segment/i],
    ['bundle\\separator-alias', '0', /backslash path-separator alias/i],
    ['/bundle/absolute', '0', /absolute member path/i],
  ])(
    'rejects the cross-platform archive namespace %s before materialization',
    async (name, type, error) => {
      const input = await fixture({
        additionalFiles:
          name === 'bundle/CaseAlias'
            ? [{ path: 'bundle/casealias', body: 'first spelling' }]
            : [],
      });
      await appendRawEntry(input, {
        name,
        type,
        body: Buffer.from('unauthorized'),
      });
      const marker = await markOutput(input);

      await expect(verify(input)).rejects.toThrow(error);
      await expect(readFile(marker, 'utf8')).resolves.toBe('untouched');
    },
  );

  test('rejects case-folding collisions between implicit archive ancestors before materialization', async () => {
    const input = await fixture();
    await appendRawEntry(input, {
      name: 'bundle/Foo/first',
      body: Buffer.from('first'),
    });
    await appendRawEntry(input, {
      name: 'bundle/foo/second',
      body: Buffer.from('second'),
    });
    const marker = await markOutput(input);

    await expect(verify(input)).rejects.toThrow(
      /Windows case-folding collision.*bundle\/Foo.*bundle\/foo/i,
    );
    await expect(readFile(marker, 'utf8')).resolves.toBe('untouched');
  });

  test.each([
    'bundle/σ/first',
    'bundle/ς/second',
    'bundle/Σ/third',
    'bundle/é/composed',
    'bundle/e\u0301/decomposed',
  ])(
    'rejects the non-ASCII portable filename alias %s during namespace preflight',
    async (name) => {
      const input = await fixture();
      await appendRawEntry(input, {
        name,
        body: Buffer.from('must not reach materialization'),
      });
      const marker = await markOutput(input);

      await expect(verify(input)).rejects.toThrow(
        /non-ASCII archive path segment/i,
      );
      await expect(readFile(marker, 'utf8')).resolves.toBe('untouched');
      await expect(readdir(input.output)).resolves.toEqual([
        'must-survive-rejected-archive',
      ]);
    },
  );

  test.each(['σ', 'ς', 'Σ', 'é', 'e\u0301'])(
    'rejects the non-ASCII symbolic-link target segment %s during namespace preflight',
    async (target) => {
      const input = await fixture({
        symlinks: [{ path: 'bundle/alias-link', target }],
      });
      const marker = await markOutput(input);

      await expect(verify(input)).rejects.toThrow(
        /non-ASCII symbolic-link target segment/i,
      );
      await expect(readFile(marker, 'utf8')).resolves.toBe('untouched');
      await expect(readdir(input.output)).resolves.toEqual([
        'must-survive-rejected-archive',
      ]);
    },
  );

  test.each([
    ['archive member', 'bundle/portable~1', undefined],
    ['symbolic-link target', 'bundle/alias-link', 'portable~1'],
  ])(
    'rejects the Windows short-name alias character within the %s during namespace preflight',
    async (_case, path, target) => {
      const input =
        target === undefined
          ? await fixture()
          : await fixture({ symlinks: [{ path, target }] });
      if (target === undefined) {
        await appendRawEntry(input, {
          name: path,
          body: Buffer.from('must not reach materialization'),
        });
      }
      const marker = await markOutput(input);

      await expect(verify(input)).rejects.toThrow(
        /outside the portable ASCII filename policy/i,
      );
      await expect(readFile(marker, 'utf8')).resolves.toBe('untouched');
      await expect(readdir(input.output)).resolves.toEqual([
        'must-survive-rejected-archive',
      ]);
    },
  );

  test('enforces the reviewed archive byte ceiling before allocating archive contents', async () => {
    await expect(
      execFileAsync(
        'node',
        [
          '--input-type=module',
          '--eval',
          [
            "import { assertCredentialedArchiveSize } from './scripts/release/extract-credentialed-output.mjs';",
            'assertCredentialedArchiveSize(1_073_741_825);',
          ].join('\n'),
        ],
        { cwd: repositoryRoot },
      ),
    ).rejects.toThrow(/exceeds the reviewed 1073741824-byte limit/i);
  });

  test('rejects an oversized sparse archive before reading it or replacing output', async () => {
    const input = await fixture();
    await truncate(input.archive, 1_073_741_825);
    const marker = await markOutput(input);

    await expect(verify(input)).rejects.toThrow(
      /exceeds the reviewed 1073741824-byte limit/i,
    );
    await expect(readFile(marker, 'utf8')).resolves.toBe('untouched');
  });

  test('uses an unpredictable staging directory that cannot be pre-planted from the extractor PID', async () => {
    const input = await fixture();
    const predictedPathRecord = join(input.root, 'predicted-staging-path');
    await execFileAsync(
      'bash',
      [
        '-c',
        [
          'set -euo pipefail',
          'predicted="$3.materializing-$$"',
          'mkdir "$predicted"',
          'printf untouched > "$predicted/sentinel"',
          'printf %s "$predicted" > "$4"',
          "exec node scripts/release/extract-credentialed-output.mjs \"$1\" \"$2\" \"$3\"",
        ].join('\n'),
        'bash',
        input.platform,
        input.download,
        input.output,
        predictedPathRecord,
      ],
      { cwd: repositoryRoot },
    );
    const predictedPath = await readFile(predictedPathRecord, 'utf8');

    await expect(
      readFile(join(predictedPath, 'sentinel'), 'utf8'),
    ).resolves.toBe('untouched');
    await expect(
      readFile(join(input.output, 'credentialed-output.json'), 'utf8'),
    ).resolves.toContain('"schemaVersion": 2');
  });

  test.each([
    ['bundle/hard', '1', 'bundle/base', /hard link/i],
    ['bundle/character-device', '3', '', /character device/i],
    ['bundle/block-device', '4', '', /block device/i],
    ['bundle/fifo', '6', '', /FIFO/i],
    ['bundle/socket', 's', '', /socket|unsupported archive member type/i],
    ['bundle/sparse', 'S', '', /sparse/i],
    ['PaxHeaders.0/bundle', 'x', '', /PAX extended header/i],
    ['GlobalHead.0', 'g', '', /PAX global header/i],
  ])(
    'rejects unauthorized archive member type %s (%s) before materialization',
    async (name, type, linkname, error) => {
      const input = await fixture({
        additionalFiles:
          type === '1' ? [{ path: 'bundle/base', body: 'base' }] : [],
      });
      await appendRawEntry(input, {
        name,
        type,
        linkname,
        body:
          type === 'x'
            ? Buffer.from('19 path=bundle/pax\n', 'utf8')
            : Buffer.alloc(0),
      });
      const marker = await markOutput(input);

      await expect(verify(input)).rejects.toThrow(error);
      await expect(readFile(marker, 'utf8')).resolves.toBe('untouched');
    },
  );

  test('rejects a symlink whose target is absent from the closed bundle namespace', async () => {
    const input = await fixture({
      symlinks: [{ path: 'bundle/missing-link', target: 'missing-target' }],
    });
    const marker = await markOutput(input);

    await expect(verify(input)).rejects.toThrow(
      /symbolic-link target is absent from the closed bundle namespace/i,
    );
    await expect(readFile(marker, 'utf8')).resolves.toBe('untouched');
  });

  test.each([
    ['target/./file', /dot symbolic-link target segment/i],
    ['target//file', /empty symbolic-link target segment/i],
    ['target\\file', /backslash path-separator alias/i],
    ['C:/target', /Windows-forbidden character/i],
  ])(
    'rejects a symbolic-link target spelling alias before materialization: %s',
    async (target, error) => {
      const input = await fixture({
        additionalFiles: [{ path: 'bundle/target/file', body: 'target' }],
        symlinks: [{ path: 'bundle/link', target }],
      });
      const marker = await markOutput(input);

      await expect(verify(input)).rejects.toThrow(error);
      await expect(readFile(marker, 'utf8')).resolves.toBe('untouched');
    },
  );

  test('rejects an archive member beneath a symbolic-link namespace entry', async () => {
    const input = await fixture({
      additionalFiles: [{ path: 'bundle/target', body: 'target' }],
      symlinks: [{ path: 'bundle/link', target: 'target' }],
    });
    await appendRawEntry(input, {
      name: 'bundle/link/payload',
      body: Buffer.from('must not be written through a link'),
    });
    const marker = await markOutput(input);

    await expect(verify(input)).rejects.toThrow(
      /archive member beneath a symbolic link/i,
    );
    await expect(readFile(marker, 'utf8')).resolves.toBe('untouched');
  });

  test.each([
    {
      name: 'invalid JSON',
      rewrite: '{not-json',
      error: /credentialed output manifest is not valid JSON/i,
    },
    {
      name: 'release tuple mismatch',
      rewrite: (manifest: Record<string, unknown>) => ({
        ...manifest,
        version: '9.9.9',
      }),
      error: /does not match the exact release platform tuple/i,
    },
    {
      name: 'closed inventory mismatch',
      rewrite: (manifest: Record<string, unknown>) => ({
        ...manifest,
        files: [],
      }),
      error:
        /invalid closed inventory|inventory does not match|escapes the closed config.*bundle handoff/i,
    },
    {
      name: 'release config mismatch',
      rewrite: (manifest: Record<string, unknown>) => ({
        ...manifest,
        releaseConfigSha256: '0'.repeat(64),
      }),
      error: /config or updater identity does not match/i,
    },
    {
      name: 'updater key mismatch',
      rewrite: (manifest: Record<string, unknown>) => ({
        ...manifest,
        updaterPublicKeySha256: '0'.repeat(64),
      }),
      error: /exact release platform tuple|config or updater identity does not match/i,
    },
  ])(
    'does not publish a structurally valid archive with a semantic $name',
    async ({ rewrite, error }) => {
      const input = await fixture();
      await rewriteCredentialedManifest(input, rewrite);
      const marker = await markOutput(input);

      await expect(verify(input)).rejects.toThrow(error);
      await expect(readFile(marker, 'utf8')).resolves.toBe('untouched');
      await expect(readdir(input.output)).resolves.toEqual([
        'must-survive-rejected-archive',
      ]);
      await expect(
        lstat(join(input.output, input.bundlePath)),
      ).rejects.toMatchObject({ code: 'ENOENT' });
      expect(
        (await readdir(input.root)).filter((name) =>
          name.startsWith('mdreader-credentialed-candidate.'),
        ),
      ).toEqual([]);
    },
  );

  test('rejects archive replacement before extraction', async () => {
    const input = await fixture();
    await writeFile(input.archive, 'replacement archive');

    await expect(verify(input)).rejects.toThrow(
      /archive bytes.*hash address/i,
    );
  });
});
