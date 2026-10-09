import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { parse } from 'yaml';
import { describe, expect, test } from 'vitest';

import {
  assertClosedRuntimeDirectory,
  validateAuthority,
  validatePinnedAuthorityDigest,
} from '../prepare-macos-ort-runtime.mjs';

const repositoryRoot = fileURLToPath(new URL('../../../', import.meta.url));
const authorityPath = resolve(
  repositoryRoot,
  'scripts',
  'release',
  'macos-ort-runtime.json',
);
const authorityDigestPath = resolve(
  repositoryRoot,
  'scripts',
  'release',
  'macos-ort-runtime.sha256',
);
const provisionerPath = resolve(
  repositoryRoot,
  'scripts',
  'release',
  'prepare-macos-ort-runtime.mjs',
);
const linkagePath = resolve(
  repositoryRoot,
  'scripts',
  'release',
  'verify-macos-runtime-linkage.sh',
);
const packagedRuntimePath = resolve(
  repositoryRoot,
  'scripts',
  'release',
  'verify-macos-packaged-runtime.sh',
);
const workflowPath = resolve(
  repositoryRoot,
  '.github',
  'workflows',
  'release.yml',
);

function workflow(): {
  jobs: Record<
    string,
    {
      steps: Array<{
        id?: string;
        if?: string;
        env?: Record<string, string>;
        run?: string;
      }>;
    }
  >;
} {
  return parse(readFileSync(workflowPath, 'utf8')) as ReturnType<
    typeof workflow
  >;
}

function step(job: string, id: string) {
  return workflow().jobs[job]!.steps.find((entry) => entry.id === id);
}

function resolvedEmbeddingStack(target: string): {
  fastembed: string;
  ort: string;
  ortSys: string;
} {
  const metadata = JSON.parse(
    execFileSync(
      'cargo',
      [
        'metadata',
        '--locked',
        '--format-version',
        '1',
        '--filter-platform',
        target,
      ],
      {
        cwd: repositoryRoot,
        encoding: 'utf8',
        maxBuffer: 64 * 1024 * 1024,
        stdio: ['ignore', 'pipe', 'pipe'],
      },
    ),
  ) as {
    packages: Array<{ id: string; name: string; version: string }>;
    resolve: {
      nodes: Array<{
        id: string;
        deps: Array<{ name: string; pkg: string }>;
      }>;
    };
  };
  const packages = new Map(metadata.packages.map((entry) => [entry.id, entry]));
  const nodes = new Map(metadata.resolve.nodes.map((entry) => [entry.id, entry]));
  const core = metadata.packages.find((entry) => entry.name === 'mdreader-core')!;
  const fastembedDependency = nodes
    .get(core.id)!
    .deps.find((entry) => entry.name.includes('fastembed'))!;
  const fastembed = packages.get(fastembedDependency.pkg)!;
  const ortDependency = nodes
    .get(fastembed.id)!
    .deps.find((entry) => entry.name === 'ort')!;
  const ort = packages.get(ortDependency.pkg)!;
  const ortSysDependency = nodes
    .get(ort.id)!
    .deps.find((entry) => entry.name === 'ort_sys')!;
  const ortSys = packages.get(ortSysDependency.pkg)!;
  return {
    fastembed: fastembed.version,
    ort: ort.version,
    ortSys: ortSys.version,
  };
}

describe('universal macOS ONNX Runtime release contract', () => {
  test('pins the reviewed source build and exact macOS launch contract', () => {
    const authorityBytes = readFileSync(authorityPath);
    const authority = JSON.parse(authorityBytes.toString('utf8'));
    expect(authority).toEqual({
      schemaVersion: 2,
      version: '1.24.2',
      source: {
        commit: '058787ceead760166e3c50a0a4cba8a833a6f53f',
        archiveUrl:
          'https://codeload.github.com/microsoft/onnxruntime/tar.gz/058787ceead760166e3c50a0a4cba8a833a6f53f',
        archiveBytes: 283_622_099,
        archiveSha256:
          'effd7da5ab292b18618e2d0675daba14dceef62d04ab3d7adf1c0ffb8a4eeb01',
        directory:
          'onnxruntime-058787ceead760166e3c50a0a4cba8a833a6f53f',
        dependencyManifest: 'cmake/deps.txt',
        dependencyManifestSha256:
          'a913db05c4e9965f9725aec7a5b9b10b071d13496e567283e81737ebe91ff027',
      },
      toolchain: {
        runner: 'macos-14',
        developerDir: '/Applications/Xcode_15.4.app/Contents/Developer',
        xcodeVersion: '15.4',
        xcodeBuild: '15F31d',
        sdkVersion: '14.5',
        cmakeVersion: '4.3.4',
        pythonVersion: '3.14.6',
      },
      build: {
        configuration: 'Release',
        target: 'onnxruntime',
        deploymentTarget: '12.0',
        clientPackageBuild: true,
        sourceDateEpoch: 1_771_477_564,
        parallelJobs: 8,
        architectures: ['arm64', 'x86_64'],
        cmakeDefinitions: [
          'CMAKE_OSX_DEPLOYMENT_TARGET=12.0',
          'FETCHCONTENT_TRY_FIND_PACKAGE_MODE=NEVER',
          'CMAKE_FIND_USE_PACKAGE_REGISTRY=OFF',
          'CMAKE_FIND_USE_SYSTEM_PACKAGE_REGISTRY=OFF',
        ],
      },
      dylibName: 'libonnxruntime.1.24.2.dylib',
      linkerAlias: 'libonnxruntime.dylib',
      linkerAliasTarget: 'libonnxruntime.1.24.2.dylib',
      installName: '@rpath/libonnxruntime.1.24.2.dylib',
      allowedDependencies: [
        '/System/Library/Frameworks/Foundation.framework/Versions/C/Foundation',
        '/System/Library/Frameworks/CoreFoundation.framework/Versions/A/CoreFoundation',
        '/usr/lib/libiconv.2.dylib',
        '/usr/lib/libc++.1.dylib',
        '/usr/lib/libSystem.B.dylib',
      ],
    });
    expect(authorityBytes.toString('utf8')).toBe(
      `${JSON.stringify(authority, null, 2)}\n`,
    );
  });

  test('checks canonical authority bytes against an independent pinned digest', () => {
    expect(existsSync(authorityDigestPath)).toBe(true);
    const authorityBytes = readFileSync(authorityPath);
    const authority = JSON.parse(authorityBytes.toString('utf8'));
    const pinnedDigest = readFileSync(authorityDigestPath, 'utf8').trim();
    expect(pinnedDigest).toMatch(/^[0-9a-f]{64}$/);
    expect(createHash('sha256').update(authorityBytes).digest('hex')).toBe(
      pinnedDigest,
    );
    expect(() => validatePinnedAuthorityDigest(pinnedDigest)).not.toThrow();
    expect(() =>
      validatePinnedAuthorityDigest('0'.repeat(64)),
    ).toThrow(/code-reviewed pin/i);
    expect(() =>
      validateAuthority(authority, authorityBytes, pinnedDigest),
    ).not.toThrow();
    expect(() =>
      validateAuthority(
        authority,
        Buffer.concat([authorityBytes, Buffer.from(' ')]),
        pinnedDigest,
      ),
    ).toThrow(/canonical|digest/i);
  });

  test('uses an executable fail-closed source builder instead of ort-sys downloads', () => {
    expect(statSync(provisionerPath).mode & 0o111).not.toBe(0);
    expect(() =>
      execFileSync(process.execPath, ['--check', provisionerPath], {
        stdio: 'pipe',
      }),
    ).not.toThrow();
    const source = readFileSync(provisionerPath, 'utf8');
    expect(source).toContain('macos-ort-runtime.json');
    expect(source).toContain('macos-ort-runtime.sha256');
    expect(source).toContain('archiveSha256');
    expect(source).toContain('CMAKE_OSX_DEPLOYMENT_TARGET');
    expect(source).toContain('FETCHCONTENT_TRY_FIND_PACKAGE_MODE');
    expect(source).toContain('build.py');
    expect(source).toContain('lipo');
    expect(source).toContain('otool');
    expect(source).toContain('ORT_LIB_LOCATION');
    expect(source).toContain('linkerAliasTarget');
    expect(source).toContain('symlink');
    expect(source).not.toContain('shell: true');
  });

  test('closes the ort-sys library directory against unreviewed entries', async () => {
    const authority = JSON.parse(readFileSync(authorityPath, 'utf8')) as {
      dylibName: string;
      linkerAlias: string;
      linkerAliasTarget: string;
    };
    const directory = mkdtempSync(resolve(tmpdir(), 'mdreader-ort-inventory-'));
    const closed = () => assertClosedRuntimeDirectory(directory, authority);
    try {
      writeFileSync(join(directory, authority.dylibName), 'runtime');
      symlinkSync(authority.linkerAliasTarget, join(directory, authority.linkerAlias));
      await expect(closed()).rejects.toThrow(/missing its reviewed entry/);

      writeFileSync(join(directory, 'runtime.json'), '{}');
      await expect(closed()).resolves.toBeUndefined();

      // ort-sys links whatever it finds in `ORT_LIB_LOCATION`: a stub library,
      // a second dylib, or a header directory can displace the reviewed runtime.
      for (const extra of [
        'libonnxruntime.tbd',
        'libonnxruntime_providers_shared.dylib',
        'libcustom.a',
      ]) {
        writeFileSync(join(directory, extra), 'displacement');
        await expect(closed()).rejects.toThrow(/unreviewed entry/);
        rmSync(join(directory, extra));
      }
      mkdirSync(join(directory, 'include'));
      await expect(closed()).rejects.toThrow(/unreviewed entry/);
      rmSync(join(directory, 'include'), { recursive: true });

      // The versioned dylib must be the real file, never a link to one.
      rmSync(join(directory, authority.dylibName));
      symlinkSync('/usr/lib/libSystem.B.dylib', join(directory, authority.dylibName));
      await expect(closed()).rejects.toThrow(/is not a file/);
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });

  test('records the LC_RPATH that makes the packaged runtime resolvable', () => {
    const buildScript = readFileSync(
      resolve(repositoryRoot, 'src-tauri', 'build.rs'),
      'utf8',
    );
    // Target-scoped: only the macOS bundle resolves the runtime through @rpath.
    expect(buildScript).toContain(
      'std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos")',
    );
    expect(buildScript).toContain(
      'cargo:rustc-link-arg-bins=-Wl,-rpath,@executable_path/../Frameworks',
    );

    const linkage = readFileSync(linkagePath, 'utf8');
    expect(statSync(linkagePath).mode & 0o111).not.toBe(0);
    expect(() =>
      execFileSync('/bin/bash', ['-n', linkagePath], { stdio: 'pipe' }),
    ).not.toThrow();
    expect(linkage).toContain('otool -arch "$2" -l "$1"');
    expect(linkage).toContain('LC_RPATH');
    expect(linkage).toContain('@executable_path/../Frameworks');
    expect(linkage).toContain('libonnxruntime.1.24.2.dylib');

    // Asserted while the build is still uncredentialed, and again on the signed
    // application, so an unlaunchable bundle can never reach publication.
    expect(
      readFileSync(
        resolve(repositoryRoot, 'scripts', 'release', 'prepare-direct-release.sh'),
        'utf8',
      ),
    ).toMatch(
      /verify-macos-runtime-linkage\.sh \\\n\s+staged \\\n\s+"target\/\$TARGET\/release\/mdreader-app"/,
    );
    expect(
      readFileSync(
        resolve(repositoryRoot, 'scripts', 'release', 'verify-macos-signature.sh'),
        'utf8',
      ),
    ).toContain('verify-macos-runtime-linkage.sh" bundled "$APP_PATH"');
  });

  test.runIf(process.platform === 'darwin')(
    'rejects a universal executable that cannot resolve the runtime at launch',
    () => {
      const directory = mkdtempSync(resolve(tmpdir(), 'mdreader-ort-linkage-'));
      const frameworks = join(directory, 'MDReader.app', 'Contents', 'Frameworks');
      const macos = join(directory, 'MDReader.app', 'Contents', 'MacOS');
      const clang = (args: string[]) =>
        execFileSync('clang', args, { cwd: directory, stdio: 'pipe' });
      const linkage = (mode: string, ...args: string[]) =>
        execFileSync('/bin/bash', [linkagePath, mode, ...args], {
          cwd: directory,
          stdio: 'pipe',
        });
      try {
        mkdirSync(frameworks, { recursive: true });
        mkdirSync(macos, { recursive: true });
        writeFileSync(join(directory, 'runtime.c'), 'int ort_stub(void){return 0;}\n');
        writeFileSync(
          join(directory, 'main.c'),
          'int ort_stub(void); int main(void){return ort_stub();}\n',
        );
        const dylib = join(frameworks, 'libonnxruntime.1.24.2.dylib');
        clang([
          '-dynamiclib',
          '-arch', 'arm64',
          '-arch', 'x86_64',
          '-mmacosx-version-min=12.0',
          '-install_name', '@rpath/libonnxruntime.1.24.2.dylib',
          'runtime.c',
          '-o', dylib,
        ]);
        const build = (output: string, extra: string[]) =>
          clang([
            '-arch', 'arm64',
            '-arch', 'x86_64',
            '-mmacosx-version-min=12.0',
            'main.c',
            dylib,
            ...extra,
            '-o', output,
          ]);

        // The defect: today's link records @rpath with no LC_RPATH to follow.
        build(join(macos, 'mdreader-app'), []);
        expect(() => linkage('bundled', join(directory, 'MDReader.app'))).toThrow(
          /LC_RPATH/,
        );

        build(join(macos, 'mdreader-app'), [
          '-Wl,-rpath,@executable_path/../Frameworks',
        ]);
        expect(() =>
          linkage('bundled', join(directory, 'MDReader.app')),
        ).not.toThrow();
        expect(() =>
          linkage('staged', join(macos, 'mdreader-app'), frameworks),
        ).not.toThrow();

        expect(existsSync(packagedRuntimePath)).toBe(true);
        expect(statSync(packagedRuntimePath).mode & 0o111).not.toBe(0);
        const updaterRoot = join(directory, 'updater');
        mkdirSync(updaterRoot);
        cpSync(
          join(directory, 'MDReader.app'),
          join(updaterRoot, 'MDReader.app'),
          { recursive: true },
        );
        expect(() =>
          execFileSync(
            '/bin/bash',
            [
              packagedRuntimePath,
              'dmg',
              join(directory, 'MDReader.app'),
              'updater',
              updaterRoot,
            ],
            { cwd: directory, stdio: 'pipe' },
          ),
        ).not.toThrow();

        const unreviewedRuntime = join(
          frameworks,
          'libonnxruntime_providers_shared.dylib',
        );
        writeFileSync(unreviewedRuntime, 'unreviewed runtime');
        expect(() =>
          execFileSync(
            '/bin/bash',
            [
              packagedRuntimePath,
              'dmg',
              join(directory, 'MDReader.app'),
              'updater',
              updaterRoot,
            ],
            { cwd: directory, stdio: 'pipe' },
          ),
        ).toThrow(/unreviewed.*Frameworks|runtime directory.*unreviewed/i);
        rmSync(unreviewedRuntime);

        const incompatibleRuntime = join(directory, 'incompatible-runtime.dylib');
        const reviewedRuntime = join(directory, 'reviewed-runtime.dylib');
        cpSync(dylib, reviewedRuntime);
        clang([
          '-dynamiclib',
          '-arch', 'arm64',
          '-arch', 'x86_64',
          '-mmacosx-version-min=13.0',
          '-install_name', '@rpath/libonnxruntime.1.24.2.dylib',
          'runtime.c',
          '-o', incompatibleRuntime,
        ]);
        cpSync(incompatibleRuntime, dylib);
        expect(() =>
          execFileSync(
            '/bin/bash',
            [
              packagedRuntimePath,
              'dmg',
              join(directory, 'MDReader.app'),
              'updater',
              updaterRoot,
            ],
            { cwd: directory, stdio: 'pipe' },
          ),
        ).toThrow(/minimum macOS.*13\.0.*12\.0/i);
        cpSync(reviewedRuntime, dylib);

        cpSync(
          incompatibleRuntime,
          join(
            updaterRoot,
            'MDReader.app',
            'Contents',
            'Frameworks',
            'libonnxruntime.1.24.2.dylib',
          ),
        );
        expect(() =>
          execFileSync(
            '/bin/bash',
            [
              packagedRuntimePath,
              'dmg',
              join(directory, 'MDReader.app'),
              'updater',
              updaterRoot,
            ],
            { cwd: directory, stdio: 'pipe' },
          ),
        ).toThrow(/minimum macOS.*13\.0.*12\.0/i);

        // A bundle whose Frameworks directory lost the runtime cannot launch.
        rmSync(dylib);
        expect(() => linkage('bundled', join(directory, 'MDReader.app'))).toThrow(
          /missing|does not provide|must contain exactly/,
        );
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    },
  );

  test.runIf(process.platform === 'darwin')(
    'checks every runtime slice dependency and minimum macOS version',
    () => {
      const directory = mkdtempSync(resolve(tmpdir(), 'mdreader-ort-slices-'));
      const clang = (args: string[]) =>
        execFileSync('clang', args, { cwd: directory, stdio: 'pipe' });
      const verify = (path: string) =>
        execFileSync('/bin/bash', [linkagePath, 'runtime', path], {
          cwd: directory,
          stdio: 'pipe',
        });
      try {
        writeFileSync(
          join(directory, 'runtime.c'),
          'int ort_stub(void){return 0;}\n',
        );
        writeFileSync(
          join(directory, 'runtime-with-dependency.c'),
          'int unreviewed(void); int ort_stub(void){return unreviewed();}\n',
        );
        writeFileSync(
          join(directory, 'unreviewed.c'),
          'int unreviewed(void){return 0;}\n',
        );
        const buildRuntime = (
          arch: 'arm64' | 'x86_64',
          minimum: string,
          source: string,
          output: string,
          dependency?: string,
        ) =>
          clang([
            '-dynamiclib',
            '-arch',
            arch,
            `-mmacosx-version-min=${minimum}`,
            '-install_name',
            '@rpath/libonnxruntime.1.24.2.dylib',
            source,
            ...(dependency ? [dependency] : []),
            '-o',
            output,
          ]);
        const arm = join(directory, 'runtime-arm64.dylib');
        const intel = join(directory, 'runtime-x86_64.dylib');
        const universal = join(directory, 'runtime-universal.dylib');
        buildRuntime('arm64', '12.0', 'runtime.c', arm);
        buildRuntime('x86_64', '12.0', 'runtime.c', intel);
        execFileSync('lipo', ['-create', arm, intel, '-output', universal]);
        expect(() => verify(universal)).not.toThrow();

        const slices = { arm64: arm, x86_64: intel } as const;
        for (const architecture of ['arm64', 'x86_64'] as const) {
          buildRuntime('arm64', '12.0', 'runtime.c', arm);
          buildRuntime('x86_64', '12.0', 'runtime.c', intel);
          buildRuntime(
            architecture,
            '13.0',
            'runtime.c',
            slices[architecture],
          );
          execFileSync('lipo', ['-create', arm, intel, '-output', universal]);
          expect(() => verify(universal)).toThrow(
            new RegExp(
              `${architecture}.*minimum macOS.*13\\.0.*12\\.0`,
              'i',
            ),
          );
        }

        for (const architecture of ['arm64', 'x86_64'] as const) {
          buildRuntime('arm64', '12.0', 'runtime.c', arm);
          buildRuntime('x86_64', '12.0', 'runtime.c', intel);
          const dependency = join(
            directory,
            `libunreviewed-${architecture}.dylib`,
          );
          clang([
            '-dynamiclib',
            '-arch',
            architecture,
            '-mmacosx-version-min=12.0',
            '-install_name',
            '@rpath/libunreviewed.dylib',
            'unreviewed.c',
            '-o',
            dependency,
          ]);
          buildRuntime(
            architecture,
            '12.0',
            'runtime-with-dependency.c',
            slices[architecture],
            dependency,
          );
          execFileSync('lipo', ['-create', arm, intel, '-output', universal]);
          expect(() => verify(universal)).toThrow(
            new RegExp(
              `${architecture}.*unreviewed dependency.*libunreviewed`,
              'i',
            ),
          );
        }
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    },
  );

  test('validates the Apple publisher identity before any credential is used', () => {
    const jobs = workflow().jobs;
    const authorize = jobs.prepare!.steps.find(
      (entry) => entry.id === 'authorize-apple-team',
    );
    expect(authorize?.env).toMatchObject({
      EXPECTED_APPLE_TEAM_ID: '${{ vars.MDREADER_APPLE_TEAM_ID }}',
    });
    expect(authorize?.run).toContain('^[A-Z0-9]{10}$');
    expect(authorize?.run).toContain('apple_team_id=$EXPECTED_APPLE_TEAM_ID');

    // Prepare owns the validation; every credentialed consumer reads its output.
    const prepareSteps = jobs.prepare!.steps;
    expect(prepareSteps.indexOf(authorize!)).toBeLessThan(
      prepareSteps.findIndex((entry) => entry.id === 'seal-release-support'),
    );
    const consumers = ['bundle-sign', 'verify'].flatMap((job) =>
      jobs[job]!.steps.filter(
        (entry) => entry.env?.EXPECTED_APPLE_TEAM_ID !== undefined,
      ),
    );
    expect(consumers.length).toBeGreaterThan(0);
    for (const consumer of consumers) {
      expect(consumer.env?.EXPECTED_APPLE_TEAM_ID).toBe(
        '${{ needs.prepare.outputs.apple_team_id }}',
      );
    }
  });

  test('isolates the universal macOS stack without downgrading Windows or Linux', () => {
    expect(resolvedEmbeddingStack('aarch64-apple-darwin')).toEqual({
      fastembed: '5.17.3',
      ort: '2.0.0-rc.12',
      ortSys: '2.0.0-rc.12',
    });
    for (const target of [
      'x86_64-pc-windows-msvc',
      'x86_64-unknown-linux-gnu',
    ]) {
      expect(resolvedEmbeddingStack(target)).toEqual({
        fastembed: '5.17.3',
        ort: '2.0.0-rc.12',
        ortSys: '2.0.0-rc.12',
      });
    }
  });

  test('provisions before compilation and seals the runtime into the closed handoff', () => {
    const prepare = step('build-unsigned', 'prepare-macos-ort-runtime');
    expect(prepare).toMatchObject({
      if: "matrix.platform == 'macos'",
    });
    expect(prepare?.run).toContain('prepare-macos-ort-runtime.mjs');

    const compile = step('build-unsigned', 'prepare-direct-release');
    expect(compile?.env).toMatchObject({
      MDREADER_MACOS_ORT_LIB_LOCATION:
        '${{ steps.prepare-macos-ort-runtime.outputs.ort_lib_location }}',
      MDREADER_MACOS_ORT_MANIFEST:
        '${{ steps.prepare-macos-ort-runtime.outputs.ort_manifest_path }}',
    });

    const seal = step('build-unsigned', 'seal-unsigned-handoff');
    expect(seal?.run).toContain('macos-ort-runtime.json');
    expect(seal?.run).toContain('macos-ort-runtime.sha256');
    expect(seal?.run).toContain('libonnxruntime.1.24.2.dylib');
    expect(seal?.run).toContain('runtime.json');

    const boundary = step('bundle-sign', 'verify-credential-boundaries');
    expect(boundary?.run).toContain('macos-ort-runtime.json');
    expect(boundary?.run).toContain('macos-ort-runtime.sha256');
    expect(boundary?.run).toContain('macos_ort_dylib_path');
    expect(boundary?.run).toContain('dylibSha256');

    const config = step('bundle-sign', 'generate-bundle-config-macos');
    expect(config?.env).toHaveProperty(
      'MACOS_ORT_DYLIB_PATH',
      '${{ steps.verify-credential-boundaries.outputs.macos_ort_dylib_path }}',
    );
    expect(config?.run).toContain('frameworks');
  });

  test('verifies the packaged runtime closure and nested signature', () => {
    const source = readFileSync(
      resolve(
        repositoryRoot,
        'scripts',
        'release',
        'verify-macos-signature.sh',
      ),
      'utf8',
    );
    expect(source).toContain(
      'Contents/Frameworks/libonnxruntime.1.24.2.dylib',
    );
    expect(source).toContain('lipo -archs');
    expect(source).toContain('otool -D');
    expect(source).toContain('@rpath/libonnxruntime.1.24.2.dylib');
    expect(source).toContain('otool -L');
    expect(source).toContain('codesign --verify --strict');
    expect(source).toContain('TeamIdentifier');
  });
});
