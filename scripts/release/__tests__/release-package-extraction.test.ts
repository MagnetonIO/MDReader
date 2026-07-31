import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  mkdtemp,
  mkdir,
  readFile,
  rm,
  symlink,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';

import { afterEach, describe, expect, test } from 'vitest';
import { reviewedExecutable } from './release-binary-fixtures';
import { resolveReviewedPackageBinary } from '../verify-package-binaries';

const execFileAsync = promisify(execFile);
const repositoryRoot = resolve(process.cwd(), '..', '..');
const extractor = resolve(
  repositoryRoot,
  'scripts',
  'release',
  'safe-package-extract.mjs',
);
const roots: string[] = [];
const APPIMAGE_LAUNCHER =
  '#!/bin/sh\nexec "$APPDIR/usr/bin/mdreader-app" "$@"\n';

interface TarMember {
  name: string;
  type?: string;
  body?: Buffer;
  size?: number;
  link?: string;
}

function octal(value: number, bytes: number): Buffer {
  return Buffer.from(`${value.toString(8).padStart(bytes - 1, '0')}\0`, 'ascii');
}

function tar(members: TarMember[]): Buffer {
  const blocks: Buffer[] = [];
  for (const member of members) {
    const body = member.body ?? Buffer.alloc(0);
    const size = member.size ?? body.byteLength;
    const header = Buffer.alloc(512);
    header.write(member.name, 0, 100, 'utf8');
    octal(member.type === '5' ? 0o755 : 0o644, 8).copy(header, 100);
    octal(0, 8).copy(header, 108);
    octal(0, 8).copy(header, 116);
    octal(size, 12).copy(header, 124);
    octal(0, 12).copy(header, 136);
    header.fill(0x20, 148, 156);
    header[156] = (member.type ?? '0').charCodeAt(0);
    if (member.link) header.write(member.link, 157, 100, 'utf8');
    header.write('ustar\0', 257, 6, 'ascii');
    header.write('00', 263, 2, 'ascii');
    octal(
      header.reduce((total, byte) => total + byte, 0),
      8,
    ).copy(header, 148);
    blocks.push(header, body);
    const padding = (512 - (body.byteLength % 512)) % 512;
    if (padding > 0) blocks.push(Buffer.alloc(padding));
  }
  blocks.push(Buffer.alloc(1024));
  return Buffer.concat(blocks);
}

function cpioEntry(
  name: string,
  mode: number,
  body: Uint8Array = new Uint8Array(),
  inode = 1,
): Buffer {
  const encodedName = Buffer.from(`${name}\0`, 'utf8');
  const fields = [
    inode,
    mode,
    0,
    0,
    1,
    0,
    body.byteLength,
    0,
    0,
    0,
    0,
    encodedName.byteLength,
    0,
  ];
  const header = Buffer.from(
    `070701${fields.map((value) => value.toString(16).padStart(8, '0')).join('')}`,
    'ascii',
  );
  const namePadding = Buffer.alloc(
    (4 - ((header.byteLength + encodedName.byteLength) % 4)) % 4,
  );
  const bodyPadding = Buffer.alloc((4 - (body.byteLength % 4)) % 4);
  return Buffer.concat([
    header,
    encodedName,
    namePadding,
    body,
    bodyPadding,
  ]);
}

function cpio(members: Array<{ name: string; mode: number; body?: Buffer }>) {
  return Buffer.concat([
    ...members.map((member, index) =>
      cpioEntry(member.name, member.mode, member.body, index + 1),
    ),
    cpioEntry('TRAILER!!!', 0, Buffer.alloc(0), members.length + 1),
  ]);
}

async function fixture(): Promise<{
  root: string;
  archive: string;
  output: string;
}> {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-safe-package-'));
  roots.push(root);
  return {
    root,
    archive: join(root, 'input.tar'),
    output: join(root, 'output'),
  };
}

async function extractTar(input: Awaited<ReturnType<typeof fixture>>) {
  return execFileAsync(process.execPath, [
    extractor,
    'tar',
    input.archive,
    input.output,
  ]);
}

afterEach(async () => {
  await Promise.all(
    roots.splice(0).map((root) => rm(root, { recursive: true, force: true })),
  );
});

describe('fail-closed final-package member extraction', () => {
  test('bounds extractor memory and wall clock while traversing 7z only once', async () => {
    const source = await readFile(extractor, 'utf8');
    expect(source).toContain('const MAX_ARCHIVE_BYTES = 128 * 1024 * 1024');
    expect(source).toContain('const MAX_EXTRACTED_BYTES = 256 * 1024 * 1024');
    expect(source).toContain('const TOOL_TIMEOUT_MS = 120_000');
    expect(source).toContain("['x', '-y',");
    expect(source).not.toContain("['x', '-so'");
    expect(source).not.toContain('Buffer.from(body)');
  });

  test('executes the shipped extractor for a closed regular-file archive', async () => {
    const input = await fixture();
    await writeFile(
      input.archive,
      tar([
        { name: 'usr/', type: '5' },
        { name: 'usr/bin/', type: '5' },
        { name: 'usr/bin/mdreader-app', body: Buffer.from('reviewed') },
      ]),
    );
    await expect(extractTar(input)).resolves.toBeDefined();
    await expect(readFile(join(input.output, 'usr/bin/mdreader-app'), 'utf8')).resolves.toBe(
      'reviewed',
    );
  });

  test('executes the shipped 7z listing validator before reading members', async () => {
    const input = await fixture();
    const source = join(input.root, 'seven-source');
    await mkdir(join(source, 'payload'), { recursive: true });
    await writeFile(join(source, 'payload', 'mdreader-app'), 'reviewed-7z');
    const archive = join(input.root, 'input.zip');
    await execFileAsync(
      '7z',
      ['a', '-tzip', archive, 'payload'],
      { cwd: source },
    );
    await expect(
      execFileAsync(process.execPath, [
        extractor,
        '7z',
        archive,
        input.output,
      ]),
    ).resolves.toBeDefined();
    await expect(
      readFile(join(input.output, 'payload', 'mdreader-app'), 'utf8'),
    ).resolves.toBe('reviewed-7z');
  });

  test('executes the shipped cpio parser and rejects traversal', async () => {
    const input = await fixture();
    await writeFile(
      input.archive,
      cpio([
        {
          name: '../escape',
          mode: 0o100644,
          body: Buffer.from('escape'),
        },
      ]),
    );
    await expect(
      execFileAsync(process.execPath, [
        extractor,
        'cpio',
        input.archive,
        input.output,
      ]),
    ).rejects.toThrow(/parent traversal/i);
  });

  test.each([
    ['absolute path', '/tmp/escape', '0', /absolute/i],
    ['parent traversal', '../escape', '0', /parent|traversal/i],
    ['symbolic link', 'usr/bin/link', '2', /symbolic link|link/i],
    ['character device', 'usr/bin/device', '3', /device/i],
  ])('rejects a %s before materialization', async (_label, name, type, error) => {
    const input = await fixture();
    await writeFile(input.archive, tar([{ name, type }]));
    await expect(extractTar(input)).rejects.toThrow(error);
  });

  test.each([
    [
      'duplicate destination',
      [
        { name: 'payload', body: Buffer.from('one') },
        { name: 'payload', body: Buffer.from('two') },
      ],
      /duplicate/i,
    ],
    [
      'case-colliding destination',
      [
        { name: 'MDReader/app', body: Buffer.from('one') },
        { name: 'mdreader/App', body: Buffer.from('two') },
      ],
      /case.*collision/i,
    ],
  ])('rejects a %s before materialization', async (_label, members, error) => {
    const input = await fixture();
    await writeFile(input.archive, tar(members));
    await expect(extractTar(input)).rejects.toThrow(error);
  });

  test('rejects excessive archive entry counts', async () => {
    const input = await fixture();
    await writeFile(
      input.archive,
      tar(
        Array.from({ length: 4097 }, (_, index) => ({
          name: `entries/${index}`,
        })),
      ),
    );
    await expect(extractTar(input)).rejects.toThrow(/entry count|4096/i);
  });

  test('rejects excessive declared extraction bytes before reading a body', async () => {
    const input = await fixture();
    await writeFile(
      input.archive,
      tar([{ name: 'oversized', size: 256 * 1024 * 1024 + 1 }]),
    );
    await expect(extractTar(input)).rejects.toThrow(/byte count|268435456/i);
  });

  test('the shipped post-extraction scanner rejects a symlink escape', async () => {
    const input = await fixture();
    await mkdir(input.output, { recursive: true });
    await symlink(join(input.root, 'outside'), join(input.output, 'escape'));
    await expect(
      execFileAsync(process.execPath, [extractor, 'scan', input.output]),
    ).rejects.toThrow(/symbolic link|escape/i);
  });

  test('rejects reviewed ballast beside a different AppImage launch target', async () => {
    const input = await fixture();
    const sourceCommit = '0123456789abcdef0123456789abcdef01234567';
    const root = join(input.root, 'appimage');
    await mkdir(join(root, 'usr', 'bin'), { recursive: true });
    await mkdir(join(root, 'ballast'), { recursive: true });
    await writeFile(join(root, 'usr', 'bin', 'not-mdreader'), 'other executable');
    await writeFile(
      join(root, 'ballast', 'mdreader-app'),
      reviewedExecutable('linux', sourceCommit),
    );
    await writeFile(join(root, 'AppRun'), '#!/bin/sh\n', { mode: 0o755 });
    await writeFile(
      join(root, 'dev.mdreader.app.desktop'),
      '[Desktop Entry]\nExec=not-mdreader\n',
    );
    await expect(
      resolveReviewedPackageBinary(root, 'linux', 'appimage', '1.0.0'),
    ).rejects.toThrow(/usr\/bin\/mdreader-app|entrypoint|sole reviewed/i);
  });

  test.each([
    ['empty launcher', '#!/bin/sh\n'],
    [
      'different executable',
      '#!/bin/sh\nexec "$APPDIR/usr/bin/not-mdreader" "$@"\n',
    ],
    [
      'command substitution',
      '#!/bin/sh\nexec "$(cat "$APPDIR/target")" "$@"\n',
    ],
    [
      'extra shell command',
      '#!/bin/sh\ntouch "$APPDIR/side-effect"\nexec "$APPDIR/usr/bin/mdreader-app" "$@"\n',
    ],
  ])('rejects an AppRun %s', async (_label, launcher) => {
    const input = await fixture();
    const sourceCommit = '0123456789abcdef0123456789abcdef01234567';
    const root = join(input.root, 'appimage');
    await mkdir(join(root, 'usr', 'bin'), { recursive: true });
    await writeFile(
      join(root, 'usr', 'bin', 'mdreader-app'),
      reviewedExecutable('linux', sourceCommit),
    );
    await writeFile(join(root, 'AppRun'), launcher, { mode: 0o755 });
    await writeFile(
      join(root, 'dev.mdreader.app.desktop'),
      '[Desktop Entry]\nExec=mdreader-app\n',
    );

    await expect(
      resolveReviewedPackageBinary(root, 'linux', 'appimage', '1.0.0'),
    ).rejects.toThrow(/AppRun.*usr\/bin\/mdreader-app|launcher/i);
  });

  test('accepts only the reviewed AppRun dispatch and matching desktop entry', async () => {
    const input = await fixture();
    const sourceCommit = '0123456789abcdef0123456789abcdef01234567';
    const root = join(input.root, 'appimage');
    await mkdir(join(root, 'usr', 'bin'), { recursive: true });
    await writeFile(
      join(root, 'usr', 'bin', 'mdreader-app'),
      reviewedExecutable('linux', sourceCommit),
    );
    await writeFile(join(root, 'AppRun'), APPIMAGE_LAUNCHER, {
      mode: 0o755,
    });
    await writeFile(
      join(root, 'dev.mdreader.app.desktop'),
      '[Desktop Entry]\nExec=mdreader-app %U\n',
    );

    await expect(
      resolveReviewedPackageBinary(root, 'linux', 'appimage', '1.0.0'),
    ).resolves.toMatchObject({
      entrypoint: {
        contract: 'linux-apprun-target',
        path: 'usr/bin/mdreader-app',
      },
    });
  });

  test.each([
    'Exec=usr/bin/mdreader-app',
    'Exec=/usr/bin/mdreader-app',
    'Exec=sh -c mdreader-app',
    'Exec=mdreader-app; other',
  ])('rejects an ambiguous desktop launch command: %s', async (command) => {
    const input = await fixture();
    const sourceCommit = '0123456789abcdef0123456789abcdef01234567';
    const root = join(input.root, 'appimage');
    await mkdir(join(root, 'usr', 'bin'), { recursive: true });
    await writeFile(
      join(root, 'usr', 'bin', 'mdreader-app'),
      reviewedExecutable('linux', sourceCommit),
    );
    await writeFile(join(root, 'AppRun'), APPIMAGE_LAUNCHER, {
      mode: 0o755,
    });
    await writeFile(
      join(root, 'dev.mdreader.app.desktop'),
      `[Desktop Entry]\n${command}\n`,
    );

    await expect(
      resolveReviewedPackageBinary(root, 'linux', 'appimage', '1.0.0'),
    ).rejects.toThrow(/desktop entry.*usr\/bin\/mdreader-app/i);
  });

  test('rejects an NSIS payload without the compiled install and launch contract', async () => {
    const input = await fixture();
    const sourceCommit = '0123456789abcdef0123456789abcdef01234567';
    const root = join(input.root, 'nsis');
    await mkdir(join(root, '$PLUGINSDIR', 'app-64bit'), {
      recursive: true,
    });
    await writeFile(
      join(root, '$PLUGINSDIR', 'app-64bit', 'mdreader-app.exe'),
      reviewedExecutable('windows', sourceCommit),
    );

    await expect(
      resolveReviewedPackageBinary(root, 'windows', 'nsis', '1.0.0'),
    ).rejects.toThrow(/NSIS.*contract|install.*launch/i);
  });

  test('accepts the compiled NSIS contract and rejects an extra application launch target', async () => {
    const input = await fixture();
    const sourceCommit = '0123456789abcdef0123456789abcdef01234567';
    const root = join(input.root, 'nsis');
    const payload = join(root, '$PLUGINSDIR', 'app-64bit');
    await mkdir(payload, { recursive: true });
    await writeFile(
      join(payload, 'mdreader-app.exe'),
      reviewedExecutable('windows', sourceCommit),
    );
    await writeFile(
      join(root, 'mdreader-package-contract'),
      'schemaVersion=1;product=MDReader;identifier=dev.mdreader.app;' +
        'version=1.0.0;installTarget=mdreader-app.exe;' +
        'launchTarget=mdreader-app.exe',
    );
    await expect(
      resolveReviewedPackageBinary(root, 'windows', 'nsis', '1.0.0'),
    ).resolves.toMatchObject({
      entrypoint: {
        contract: 'windows-nsis-install-target',
        path: 'mdreader-app.exe',
      },
    });

    await writeFile(
      join(payload, 'mdreader-shadow.exe'),
      reviewedExecutable('windows', sourceCommit),
    );
    await expect(
      resolveReviewedPackageBinary(root, 'windows', 'nsis', '1.0.0'),
    ).rejects.toThrow(/extra unreviewed.*launch target/i);
  });

  test('rejects a macOS bundle whose package version differs from the release', async () => {
    const input = await fixture();
    const sourceCommit = '0123456789abcdef0123456789abcdef01234567';
    const root = join(input.root, 'dmg');
    const app = join(root, 'MDReader.app', 'Contents');
    await mkdir(join(app, 'MacOS'), { recursive: true });
    await writeFile(
      join(app, 'MacOS', 'mdreader-app'),
      reviewedExecutable('macos', sourceCommit),
    );
    await writeFile(
      join(app, 'Info.plist'),
      `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>dev.mdreader.app</string>
<key>CFBundleExecutable</key><string>mdreader-app</string>
<key>CFBundleShortVersionString</key><string>9.9.9</string>
<key>CFBundleVersion</key><string>9.9.9</string>
</dict></plist>`,
    );

    await expect(
      resolveReviewedPackageBinary(root, 'macos', 'dmg', '1.0.0'),
    ).rejects.toThrow(/version.*1\.0\.0|bundle version/i);
  });

  test.each([
    ['missing', 0],
    ['duplicate', 2],
  ])(
    'the shipped final verifier rejects a %s app binary',
    async (_label, appBinaryCount) => {
      const input = await fixture();
      const sourceCommit = '0123456789abcdef0123456789abcdef01234567';
      const reviewed = reviewedExecutable('linux', sourceCommit);
      const reviewedRoot = join(input.root, 'credentialed');
      const reviewedPath = join(
        reviewedRoot,
        'reviewed-binary',
        'mdreader-app',
      );
      await mkdir(join(reviewedRoot, 'reviewed-binary'), { recursive: true });
      await writeFile(reviewedPath, reviewed);
      const manifestPath = join(reviewedRoot, 'credentialed-output.json');
      await writeFile(
        manifestPath,
        `${JSON.stringify({
          schemaVersion: 2,
          platform: 'linux',
          target: 'x86_64-unknown-linux-gnu',
          sourceCommit,
          unsignedHandoffSha256: 'a'.repeat(64),
          reviewedBinaryPath: 'reviewed-binary/mdreader-app',
          reviewedBinarySha256: createHash('sha256')
            .update(reviewed)
            .digest('hex'),
        })}\n`,
      );
      const artifact = join(input.root, 'package.bin');
      await writeFile(artifact, 'package');
      const args = [
        'exec',
        'tsx',
        'scripts/release/verify-package-binaries.ts',
        '--platform',
        'linux',
        '--target',
        'x86_64-unknown-linux-gnu',
        '--version',
        '1.0.0',
        '--source-commit',
        sourceCommit,
        '--credentialed-manifest',
        manifestPath,
        '--output',
        join(input.root, 'evidence.json'),
      ];
      for (const kind of ['appimage', 'deb', 'rpm', 'updater']) {
        const packageRoot = join(input.root, kind);
        await mkdir(join(packageRoot, 'usr', 'bin'), { recursive: true });
        const count = kind === 'appimage' ? appBinaryCount : 1;
        if (count > 0) {
          await writeFile(
            join(packageRoot, 'usr', 'bin', 'mdreader-app'),
            reviewed,
          );
        }
        if (count > 1) {
          await mkdir(join(packageRoot, 'ballast'), { recursive: true });
          await writeFile(join(packageRoot, 'ballast', 'mdreader-app'), reviewed);
        }
        if (kind === 'appimage' || kind === 'updater') {
          await writeFile(join(packageRoot, 'AppRun'), APPIMAGE_LAUNCHER, {
            mode: 0o755,
          });
          await writeFile(
            join(packageRoot, 'dev.mdreader.app.desktop'),
            '[Desktop Entry]\nExec=mdreader-app\n',
          );
        }
        args.push('--package', `${kind}=${packageRoot}`);
        args.push('--artifact', `${kind}=${artifact}`);
      }
      await expect(
        execFileAsync('pnpm', args, { cwd: repositoryRoot }),
      ).rejects.toThrow(
        new RegExp(
          appBinaryCount === 0
            ? 'sole reviewed executable|usr/bin/mdreader-app'
            : 'sole reviewed executable|usr/bin/mdreader-app',
          'i',
        ),
      );
    },
  );
});
