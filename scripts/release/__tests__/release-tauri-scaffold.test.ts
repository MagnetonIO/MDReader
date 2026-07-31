import { execFile } from 'node:child_process';
import {
  chmod,
  copyFile,
  cp,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  realpath,
  rm,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { promisify } from 'node:util';

import { afterEach, expect, test } from 'vitest';

const execFileAsync = promisify(execFile);
const repositoryRoot = resolve(process.cwd(), '..', '..');
const roots: string[] = [];

afterEach(async () => {
  await Promise.all(
    roots.splice(0).map((root) => rm(root, { recursive: true, force: true })),
  );
});

test.runIf(process.platform === 'darwin')(
  'the isolated minimal scaffold is sufficient for the real Tauri bundle-only discovery path',
  async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-tauri-scaffold-'));
    roots.push(root);
    const workspace = join(root, 'workspace');
    const tauriRoot = join(workspace, 'apps', 'mdreader', 'src-tauri');
    const uiDist = join(workspace, 'apps', 'mdreader', 'ui', 'dist');
    const target = 'universal-apple-darwin';
    const targetRoot = join(workspace, 'target');
    const binary = join(targetRoot, target, 'release', 'mdreader-app');
    const runtime = join(root, 'runtime', 'node_modules', '@tauri-apps');
    await Promise.all([
      mkdir(join(tauriRoot, 'src'), { recursive: true }),
      mkdir(join(tauriRoot, 'icons'), { recursive: true }),
      mkdir(uiDist, { recursive: true }),
      mkdir(dirname(binary), { recursive: true }),
      mkdir(runtime, { recursive: true }),
    ]);

    const linkedCliRoot = dirname(
      resolve(
        repositoryRoot,
        'apps',
        'mdreader',
        'ui',
        'node_modules',
        '@tauri-apps',
        'cli',
        'package.json',
      ),
    );
    const cliRoot = await realpath(linkedCliRoot);
    const nativeName =
      process.arch === 'arm64' ? 'cli-darwin-arm64' : 'cli-darwin-x64';
    const nativeRoot = await realpath(join(cliRoot, '..', nativeName));
    await cp(cliRoot, join(runtime, 'cli'), {
      recursive: true,
      dereference: true,
    });
    await cp(nativeRoot, join(runtime, nativeName), {
      recursive: true,
      dereference: true,
    });

    for (const icon of [
      '32x32.png',
      '128x128.png',
      '128x128@2x.png',
      'icon.icns',
      'icon.ico',
    ]) {
      await copyFile(
        join(
          repositoryRoot,
          'apps',
          'mdreader',
          'src-tauri',
          'icons',
          icon,
        ),
        join(tauriRoot, 'icons', icon),
      );
    }
    await writeFile(
      join(tauriRoot, 'Cargo.toml'),
      '[package]\nname = "mdreader-app"\nversion = "1.0.0"\nedition = "2021"\ndescription = "MDReader desktop shell"\nlicense = "Apache-2.0 OR MIT"\n\n[features]\ndirect-updater = []\n\n[[bin]]\nname = "mdreader-app"\npath = "src/main.rs"\n',
    );
    await writeFile(join(tauriRoot, 'src', 'main.rs'), 'fn main() {}\n');
    await writeFile(
      join(uiDist, 'index.html'),
      '<!doctype html><title>MDReader</title>\n',
    );
    await writeFile(binary, '#!/bin/sh\nexit 0\n');
    await chmod(binary, 0o755);
    await writeFile(
      join(tauriRoot, 'tauri.conf.json'),
      `${JSON.stringify({
        productName: 'MDReader',
        identifier: 'dev.mdreader.app',
        version: '1.0.0',
        build: {
          beforeBuildCommand: '',
          beforeBundleCommand: '',
          frontendDist: '../ui/dist',
        },
        bundle: {
          active: true,
          icon: [
            'icons/32x32.png',
            'icons/128x128.png',
            'icons/128x128@2x.png',
            'icons/icon.icns',
            'icons/icon.ico',
          ],
        },
      }, null, 2)}\n`,
    );

    const cli = join(runtime, 'cli', 'tauri.js');
    const { stderr } = await execFileAsync(
      process.execPath,
      [
        cli,
        'bundle',
        '--target',
        target,
        '--bundles',
        'app',
        '--features',
        'direct-updater',
        '--no-sign',
        '--config',
        join(tauriRoot, 'tauri.conf.json'),
      ],
      {
        cwd: tauriRoot,
        env: {
          ...process.env,
          CARGO_TARGET_DIR: targetRoot,
          GITHUB_WORKSPACE: '',
          CI: 'true',
        },
        maxBuffer: 16 * 1024 * 1024,
      },
    );

    expect(stderr).not.toMatch(/could not find|not found/i);
    const appRoot = join(targetRoot, target, 'release', 'bundle', 'macos');
    expect(await readdir(appRoot)).toEqual(
      expect.arrayContaining(['MDReader.app']),
    );
    expect(
      await readFile(
        join(appRoot, 'MDReader.app', 'Contents', 'Info.plist'),
        'utf8',
      ),
    ).toContain('dev.mdreader.app');
  },
);
