#!/usr/bin/env node

import { createHash, randomUUID } from 'node:crypto';
import { lstatSync, readFileSync } from 'node:fs';
import {
  mkdir,
  mkdtemp,
  open,
  readFile,
  readdir,
  readlink,
  rename,
  rm,
  stat,
  symlink,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const HERE = dirname(fileURLToPath(import.meta.url));
const AUTHORITY_PATH = join(HERE, 'macos-ort-runtime.json');
const AUTHORITY_DIGEST_PATH = join(HERE, 'macos-ort-runtime.sha256');
const PINNED_AUTHORITY_SHA256 =
  'eb08ba6f3a0cd30e684148e1f362e57a313fd7cf066403d44c09260663065760';
const LINKAGE_VERIFIER = join(HERE, 'verify-macos-runtime-linkage.sh');
const HASH = /^[0-9a-f]{64}$/;
const ORT_LIB_LOCATION = 'ORT_LIB_LOCATION';
const MAX_DOWNLOAD_BYTES = 300_000_000;
const MAX_ARCHIVE_ENTRIES = 100_000;

export function validatePinnedAuthorityDigest(authorityDigest) {
  if (authorityDigest !== PINNED_AUTHORITY_SHA256) {
    throw new Error(
      'macOS ONNX Runtime authority digest differs from its code-reviewed pin',
    );
  }
}

function requiredOption(args, name) {
  const index = args.indexOf(name);
  const value = index >= 0 ? args[index + 1] : undefined;
  if (!value || value.startsWith('--')) {
    throw new Error(`missing required option ${name}`);
  }
  return value;
}

function optionalOption(args, name) {
  const index = args.indexOf(name);
  const value = index >= 0 ? args[index + 1] : undefined;
  if (value?.startsWith('--')) {
    throw new Error(`missing value for option ${name}`);
  }
  return value;
}

function sha256(body) {
  return createHash('sha256').update(body).digest('hex');
}

async function sha256File(path) {
  const handle = await open(path, 'r');
  const digest = createHash('sha256');
  try {
    for await (const chunk of handle.createReadStream()) {
      digest.update(chunk);
    }
  } finally {
    await handle.close();
  }
  return digest.digest('hex');
}

function canonicalJson(value) {
  return `${JSON.stringify(value, null, 2)}\n`;
}

function exactKeys(value, keys) {
  return (
    value !== null &&
    typeof value === 'object' &&
    !Array.isArray(value) &&
    JSON.stringify(Object.keys(value).sort()) ===
      JSON.stringify([...keys].sort())
  );
}

export function validateAuthority(value, authorityBytes, pinnedDigest) {
  const canonical = canonicalJson(value);
  if (
    !HASH.test(pinnedDigest) ||
    sha256(authorityBytes) !== pinnedDigest ||
    !Buffer.from(authorityBytes).equals(Buffer.from(canonical))
  ) {
    throw new Error(
      'macOS ONNX Runtime authority digest or canonical bytes are invalid',
    );
  }
  if (
    !exactKeys(value, [
      'allowedDependencies',
      'build',
      'dylibName',
      'installName',
      'linkerAlias',
      'linkerAliasTarget',
      'schemaVersion',
      'source',
      'toolchain',
      'version',
    ]) ||
    value.schemaVersion !== 2 ||
    value.version !== '1.24.2' ||
    !exactKeys(value.source, [
      'archiveBytes',
      'archiveSha256',
      'archiveUrl',
      'commit',
      'dependencyManifest',
      'dependencyManifestSha256',
      'directory',
    ]) ||
    value.source.commit !== '058787ceead760166e3c50a0a4cba8a833a6f53f' ||
    value.source.archiveUrl !==
      'https://codeload.github.com/microsoft/onnxruntime/tar.gz/058787ceead760166e3c50a0a4cba8a833a6f53f' ||
    value.source.archiveBytes !== 283_622_099 ||
    value.source.archiveSha256 !==
      'effd7da5ab292b18618e2d0675daba14dceef62d04ab3d7adf1c0ffb8a4eeb01' ||
    value.source.directory !==
      'onnxruntime-058787ceead760166e3c50a0a4cba8a833a6f53f' ||
    value.source.dependencyManifest !== 'cmake/deps.txt' ||
    value.source.dependencyManifestSha256 !==
      'a913db05c4e9965f9725aec7a5b9b10b071d13496e567283e81737ebe91ff027' ||
    !exactKeys(value.toolchain, [
      'cmakeVersion',
      'developerDir',
      'pythonVersion',
      'runner',
      'sdkVersion',
      'xcodeBuild',
      'xcodeVersion',
    ]) ||
    value.toolchain.runner !== 'macos-14' ||
    value.toolchain.developerDir !==
      '/Applications/Xcode_15.4.app/Contents/Developer' ||
    value.toolchain.xcodeVersion !== '15.4' ||
    value.toolchain.xcodeBuild !== '15F31d' ||
    value.toolchain.sdkVersion !== '14.5' ||
    value.toolchain.cmakeVersion !== '4.3.4' ||
    value.toolchain.pythonVersion !== '3.14.6' ||
    !exactKeys(value.build, [
      'architectures',
      'clientPackageBuild',
      'cmakeDefinitions',
      'configuration',
      'deploymentTarget',
      'parallelJobs',
      'sourceDateEpoch',
      'target',
    ]) ||
    value.build.configuration !== 'Release' ||
    value.build.target !== 'onnxruntime' ||
    value.build.deploymentTarget !== '12.0' ||
    value.build.clientPackageBuild !== true ||
    value.build.sourceDateEpoch !== 1_771_477_564 ||
    value.build.parallelJobs !== 8 ||
    JSON.stringify(value.build.architectures) !==
      JSON.stringify(['arm64', 'x86_64']) ||
    JSON.stringify(value.build.cmakeDefinitions) !==
      JSON.stringify([
        'CMAKE_OSX_DEPLOYMENT_TARGET=12.0',
        'FETCHCONTENT_TRY_FIND_PACKAGE_MODE=NEVER',
        'CMAKE_FIND_USE_PACKAGE_REGISTRY=OFF',
        'CMAKE_FIND_USE_SYSTEM_PACKAGE_REGISTRY=OFF',
      ]) ||
    value.dylibName !== 'libonnxruntime.1.24.2.dylib' ||
    value.linkerAlias !== 'libonnxruntime.dylib' ||
    value.linkerAliasTarget !== value.dylibName ||
    value.installName !== '@rpath/libonnxruntime.1.24.2.dylib' ||
    JSON.stringify(value.allowedDependencies) !==
      JSON.stringify([
        '/System/Library/Frameworks/Foundation.framework/Versions/C/Foundation',
        '/System/Library/Frameworks/CoreFoundation.framework/Versions/A/CoreFoundation',
        '/usr/lib/libiconv.2.dylib',
        '/usr/lib/libc++.1.dylib',
        '/usr/lib/libSystem.B.dylib',
      ])
  ) {
    throw new Error(
      'macOS ONNX Runtime authority is not the reviewed source-build identity',
    );
  }
  return value;
}

function command(commandName, args, options = {}) {
  const result = spawnSync(commandName, args, {
    encoding: 'utf8',
    maxBuffer: options.maxBuffer ?? 16 * 1024 * 1024,
    stdio: options.inherit
      ? ['ignore', 'inherit', 'inherit']
      : ['ignore', 'pipe', 'pipe'],
    cwd: options.cwd,
    env: options.env,
  });
  if (result.error || result.status !== 0) {
    const detail = result.error?.message ?? result.stderr?.trim() ?? '';
    throw new Error(`${commandName} failed while preparing ONNX Runtime: ${detail}`);
  }
  return result.stdout?.trim() ?? '';
}

function authorityEnvironment(authority) {
  return {
    ...process.env,
    DEVELOPER_DIR: authority.toolchain.developerDir,
  };
}

function verifyToolchain(authority) {
  const env = authorityEnvironment(authority);
  const xcode = command('xcodebuild', ['-version'], { env });
  const sdk = command('xcrun', ['--sdk', 'macosx', '--show-sdk-version'], {
    env,
  });
  const cmake = command('cmake', ['--version'], { env });
  const python = command('python3', ['--version'], { env });
  if (
    xcode !==
      `Xcode ${authority.toolchain.xcodeVersion}\nBuild version ${authority.toolchain.xcodeBuild}` ||
    sdk !== authority.toolchain.sdkVersion ||
    cmake.split(/\r?\n/, 1)[0] !==
      `cmake version ${authority.toolchain.cmakeVersion}` ||
    python !== `Python ${authority.toolchain.pythonVersion}`
  ) {
    throw new Error(
      'macOS ONNX Runtime source build toolchain differs from its reviewed identity',
    );
  }
}

// `ORT_LIB_LOCATION` is a search directory: ort-sys links whatever library or
// stub it finds there. Keep the handoff directory closed to these three entries.
export async function assertClosedRuntimeDirectory(output, authority) {
  const expected = new Map([
    [authority.dylibName, 'file'],
    [authority.linkerAlias, 'symlink'],
    ['runtime.json', 'file'],
  ]);
  const entries = await readdir(output, { withFileTypes: true });
  const seen = new Set();
  for (const entry of entries) {
    const kind = expected.get(entry.name);
    if (kind === undefined) {
      throw new Error(
        `ONNX Runtime library directory contains an unreviewed entry: ${entry.name}`,
      );
    }
    if (kind === 'symlink' ? !entry.isSymbolicLink() : !entry.isFile()) {
      throw new Error(
        `ONNX Runtime library directory entry is not a ${kind}: ${entry.name}`,
      );
    }
    seen.add(entry.name);
  }
  for (const name of expected.keys()) {
    if (!seen.has(name)) {
      throw new Error(
        `ONNX Runtime library directory is missing its reviewed entry: ${name}`,
      );
    }
  }
}

async function assertRegularFile(path, label) {
  const metadata = lstatSync(path);
  if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.nlink !== 1) {
    throw new Error(`${label} must be one regular, non-linked file`);
  }
  return metadata;
}

async function downloadArchive(url, destination, expectedBytes) {
  const parsed = new URL(url);
  if (
    parsed.protocol !== 'https:' ||
    parsed.username !== '' ||
    parsed.password !== '' ||
    parsed.search !== '' ||
    parsed.hash !== ''
  ) {
    throw new Error('ONNX Runtime source URL must be credential-free HTTPS');
  }
  const response = await fetch(parsed, {
    redirect: 'follow',
    signal: AbortSignal.timeout(600_000),
  });
  if (!response.ok || !response.body || new URL(response.url).protocol !== 'https:') {
    throw new Error(
      `ONNX Runtime source download failed with status ${response.status}`,
    );
  }
  const declaredLength = response.headers.get('content-length');
  const declaredBytes =
    declaredLength === null ? undefined : Number(declaredLength);
  if (
    declaredBytes !== undefined &&
    Number.isFinite(declaredBytes) &&
    (declaredBytes > MAX_DOWNLOAD_BYTES || declaredBytes !== expectedBytes)
  ) {
    throw new Error(
      'ONNX Runtime source archive length differs from its reviewed identity',
    );
  }
  const handle = await open(destination, 'wx', 0o600);
  let bytes = 0;
  try {
    for await (const chunk of response.body) {
      bytes += chunk.byteLength;
      if (bytes > expectedBytes || bytes > MAX_DOWNLOAD_BYTES) {
        throw new Error(
          'ONNX Runtime source archive exceeded its reviewed byte length',
        );
      }
      await handle.write(chunk);
    }
  } finally {
    await handle.close();
  }
  if (bytes !== expectedBytes) {
    throw new Error('ONNX Runtime source archive was truncated');
  }
}

async function verifySourceArchive(path, authority) {
  const metadata = await assertRegularFile(path, 'ONNX Runtime source archive');
  if (
    metadata.size !== authority.source.archiveBytes ||
    (await sha256File(path)) !== authority.source.archiveSha256
  ) {
    throw new Error(
      'ONNX Runtime source archive differs from its reviewed identity',
    );
  }
}

function inspectArchive(path, authority) {
  const listing = command('tar', ['-tzf', path], {
    maxBuffer: 64 * 1024 * 1024,
  }).split(/\r?\n/);
  if (listing.length < 1 || listing.length > MAX_ARCHIVE_ENTRIES) {
    throw new Error('ONNX Runtime source archive inventory is not bounded');
  }
  const prefix = `${authority.source.directory}/`;
  for (const entry of listing) {
    const normalized = entry.replace(/\/$/, '');
    if (
      normalized.length === 0 ||
      normalized.includes('\0') ||
      (normalized !== authority.source.directory &&
        !normalized.startsWith(prefix)) ||
      normalized.split('/').some((component) => component === '..')
    ) {
      throw new Error(
        `ONNX Runtime source archive contains an unreviewed path: ${entry}`,
      );
    }
  }
}

async function extractSource(archivePath, work, authority) {
  inspectArchive(archivePath, authority);
  command('tar', ['-xzf', archivePath, '-C', work]);
  const source = join(work, authority.source.directory);
  const metadata = lstatSync(source);
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
    throw new Error('ONNX Runtime source root is not one real directory');
  }
  const dependencyManifest = join(
    source,
    authority.source.dependencyManifest,
  );
  await assertRegularFile(
    dependencyManifest,
    'ONNX Runtime dependency manifest',
  );
  if (
    (await sha256File(dependencyManifest)) !==
    authority.source.dependencyManifestSha256
  ) {
    throw new Error(
      'ONNX Runtime dependency manifest differs from its reviewed identity',
    );
  }
  return source;
}

function validateArchitectures(output, expected) {
  const actual = [...new Set(output.split(/\s+/).filter(Boolean))].sort();
  const wanted = [...expected].sort();
  if (JSON.stringify(actual) !== JSON.stringify(wanted)) {
    throw new Error(
      `ONNX Runtime dylib architectures ${actual.join(',')} do not match ${wanted.join(',')}`,
    );
  }
}

function validateInstallNames(output, expected, architectureCount) {
  const names = output
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.startsWith('@'));
  if (
    names.length !== architectureCount ||
    names.some((name) => name !== expected)
  ) {
    throw new Error(
      'ONNX Runtime dylib install name is not the reviewed @rpath identity',
    );
  }
}

function verifyRuntimePolicy(path, authority) {
  validateArchitectures(
    command('lipo', ['-archs', path]),
    authority.build.architectures,
  );
  validateInstallNames(
    command('otool', ['-D', path]),
    authority.installName,
    authority.build.architectures.length,
  );
  command('/bin/bash', [LINKAGE_VERIFIER, 'runtime', path]);
}

async function buildThinRuntime(source, work, architecture, authority) {
  const buildDirectory = join(work, `build-${architecture}`);
  const prefixMap = `-ffile-prefix-map=${work}=/usr/src/mdreader-ort`;
  const args = [
    join(source, 'tools', 'ci_build', 'build.py'),
    '--build_dir',
    buildDirectory,
    '--config',
    authority.build.configuration,
    '--update',
    '--build',
    '--build_shared_lib',
    '--client_package_build',
    '--skip_tests',
    '--skip_submodule_sync',
    '--compile_no_warning_as_error',
    '--parallel',
    String(authority.build.parallelJobs),
    '--target',
    authority.build.target,
    '--osx_arch',
    architecture,
    '--cmake_extra_defines',
    ...authority.build.cmakeDefinitions,
    `CMAKE_C_FLAGS=${prefixMap}`,
    `CMAKE_CXX_FLAGS=${prefixMap}`,
  ];
  command('python3', args, {
    inherit: true,
    env: {
      ...authorityEnvironment(authority),
      SOURCE_DATE_EPOCH: String(authority.build.sourceDateEpoch),
      ZERO_AR_DATE: '1',
      PYTHONHASHSEED: '0',
    },
  });
  const dylib = join(
    buildDirectory,
    authority.build.configuration,
    authority.dylibName,
  );
  await assertRegularFile(dylib, `${architecture} ONNX Runtime dylib`);
  validateArchitectures(command('lipo', ['-archs', dylib]), [architecture]);
  validateInstallNames(command('otool', ['-D', dylib]), authority.installName, 1);
  return dylib;
}

async function fileIdentity(path, architecture) {
  const metadata = await stat(path);
  return {
    architecture,
    bytes: metadata.size,
    sha256: await sha256File(path),
  };
}

async function createRuntimeManifest(
  authority,
  authorityDigest,
  dylibPath,
  slices,
) {
  const metadata = await stat(dylibPath);
  return canonicalJson({
    schemaVersion: 2,
    authoritySha256: authorityDigest,
    version: authority.version,
    source: authority.source,
    toolchain: authority.toolchain,
    build: authority.build,
    dylib: {
      path: authority.dylibName,
      bytes: metadata.size,
      sha256: await sha256File(dylibPath),
      architectures: authority.build.architectures,
      installName: authority.installName,
      slices,
      allowedDependencies: authority.allowedDependencies,
      deploymentTarget: authority.build.deploymentTarget,
    },
    linkerAlias: {
      path: authority.linkerAlias,
      target: authority.linkerAliasTarget,
    },
  });
}

async function verifyLinkerAlias(path, authority) {
  const metadata = lstatSync(path);
  if (!metadata.isSymbolicLink()) {
    throw new Error('ONNX Runtime linker alias must be one symbolic link');
  }
  if ((await readlink(path)) !== authority.linkerAliasTarget) {
    throw new Error(
      'ONNX Runtime linker alias does not target the reviewed versioned dylib',
    );
  }
}

async function verifyPreparedRuntime(
  output,
  authority,
  authorityDigest,
) {
  await assertClosedRuntimeDirectory(output, authority);
  const dylibPath = join(output, authority.dylibName);
  const manifestPath = join(output, 'runtime.json');
  await assertRegularFile(dylibPath, 'ONNX Runtime dylib');
  await assertRegularFile(manifestPath, 'ONNX Runtime manifest');
  await verifyLinkerAlias(join(output, authority.linkerAlias), authority);
  verifyRuntimePolicy(dylibPath, authority);
  const manifestBytes = await readFile(manifestPath);
  const manifest = JSON.parse(manifestBytes.toString('utf8'));
  if (
    !Buffer.from(manifestBytes).equals(Buffer.from(canonicalJson(manifest))) ||
    manifest.schemaVersion !== 2 ||
    manifest.authoritySha256 !== authorityDigest ||
    manifest.version !== authority.version ||
    JSON.stringify(manifest.source) !== JSON.stringify(authority.source) ||
    JSON.stringify(manifest.toolchain) !== JSON.stringify(authority.toolchain) ||
    JSON.stringify(manifest.build) !== JSON.stringify(authority.build) ||
    !exactKeys(manifest.dylib, [
      'allowedDependencies',
      'architectures',
      'bytes',
      'deploymentTarget',
      'installName',
      'path',
      'sha256',
      'slices',
    ]) ||
    manifest.dylib.path !== authority.dylibName ||
    manifest.dylib.bytes !== (await stat(dylibPath)).size ||
    manifest.dylib.sha256 !== (await sha256File(dylibPath)) ||
    JSON.stringify(manifest.dylib.architectures) !==
      JSON.stringify(authority.build.architectures) ||
    manifest.dylib.installName !== authority.installName ||
    JSON.stringify(manifest.dylib.allowedDependencies) !==
      JSON.stringify(authority.allowedDependencies) ||
    manifest.dylib.deploymentTarget !== authority.build.deploymentTarget ||
    !Array.isArray(manifest.dylib.slices) ||
    JSON.stringify(
      manifest.dylib.slices.map((slice) => slice.architecture),
    ) !== JSON.stringify(authority.build.architectures) ||
    manifest.dylib.slices.some(
      (slice) =>
        !exactKeys(slice, ['architecture', 'bytes', 'sha256']) ||
        !Number.isSafeInteger(slice.bytes) ||
        slice.bytes < 1 ||
        !HASH.test(slice.sha256),
    ) ||
    JSON.stringify(manifest.linkerAlias) !==
      JSON.stringify({
        path: authority.linkerAlias,
        target: authority.linkerAliasTarget,
      })
  ) {
    throw new Error(
      'ONNX Runtime manifest differs from the reviewed source-build identity',
    );
  }
  const slicesRoot = await mkdtemp(
    join(resolve(process.env.RUNNER_TEMP || tmpdir()), 'mdreader-ort-slices-'),
  );
  try {
    for (const slice of manifest.dylib.slices) {
      const slicePath = join(slicesRoot, `${slice.architecture}.dylib`);
      command('lipo', [
        dylibPath,
        '-thin',
        slice.architecture,
        '-output',
        slicePath,
      ]);
      const metadata = await assertRegularFile(
        slicePath,
        `${slice.architecture} ONNX Runtime slice`,
      );
      if (
        metadata.size !== slice.bytes ||
        (await sha256File(slicePath)) !== slice.sha256
      ) {
        throw new Error(
          `${slice.architecture} ONNX Runtime slice differs from its measured identity`,
        );
      }
    }
  } finally {
    await rm(slicesRoot, { recursive: true, force: true });
  }
  return {
    dylibPath,
    dylibSha256: manifest.dylib.sha256,
    manifestPath,
    manifestSha256: sha256(manifestBytes),
  };
}

async function emitOutputs(output, identity, authorityDigest) {
  const outputLines = [
    `ort_lib_location=${output}`,
    `ort_dylib_path=${identity.dylibPath}`,
    `ort_dylib_sha256=${identity.dylibSha256}`,
    `ort_manifest_path=${identity.manifestPath}`,
    `ort_manifest_sha256=${identity.manifestSha256}`,
    `ort_authority_sha256=${authorityDigest}`,
  ];
  if (process.env.GITHUB_OUTPUT) {
    await writeFile(process.env.GITHUB_OUTPUT, `${outputLines.join('\n')}\n`, {
      encoding: 'utf8',
      flag: 'a',
    });
  }
}

async function loadAuthority() {
  const authorityBytes = readFileSync(AUTHORITY_PATH);
  const digestBytes = readFileSync(AUTHORITY_DIGEST_PATH, 'utf8');
  if (!/^[0-9a-f]{64}\n$/.test(digestBytes)) {
    throw new Error('macOS ONNX Runtime pinned authority digest is invalid');
  }
  const authorityDigest = digestBytes.trim();
  validatePinnedAuthorityDigest(authorityDigest);
  const authority = validateAuthority(
    JSON.parse(authorityBytes.toString('utf8')),
    authorityBytes,
    authorityDigest,
  );
  return { authority, authorityDigest };
}

async function prepare(args) {
  if (process.platform !== 'darwin') {
    throw new Error(
      'universal macOS ONNX Runtime preparation requires macOS tooling',
    );
  }
  const { authority, authorityDigest } = await loadAuthority();
  const output = resolve(requiredOption(args, '--output'));
  if (args.includes('--verify')) {
    const identity = await verifyPreparedRuntime(
      output,
      authority,
      authorityDigest,
    );
    await emitOutputs(output, identity, authorityDigest);
    process.stdout.write(
      `verified universal macOS ONNX Runtime ${authority.version}; ${ORT_LIB_LOCATION}=${output}\n`,
    );
    return;
  }

  verifyToolchain(authority);
  await mkdir(output, { recursive: true, mode: 0o700 });
  const outputMetadata = lstatSync(output);
  if (!outputMetadata.isDirectory() || outputMetadata.isSymbolicLink()) {
    throw new Error('ONNX Runtime output must be one real directory');
  }
  if ((await readdir(output)).length !== 0) {
    throw new Error('ONNX Runtime output directory must start empty');
  }

  const work = await mkdtemp(
    join(resolve(process.env.RUNNER_TEMP || tmpdir()), 'mdreader-ort-source-'),
  );
  const archiveOption = optionalOption(args, '--archive');
  const archivePath = archiveOption
    ? resolve(archiveOption)
    : join(work, `onnxruntime-source-${randomUUID()}.tgz`);
  const temporaryDylib = join(work, `universal-${randomUUID()}.dylib`);
  try {
    if (!archiveOption) {
      await downloadArchive(
        authority.source.archiveUrl,
        archivePath,
        authority.source.archiveBytes,
      );
    }
    await verifySourceArchive(archivePath, authority);
    const source = await extractSource(archivePath, work, authority);
    const slices = [];
    const thin = [];
    for (const architecture of authority.build.architectures) {
      const path = await buildThinRuntime(source, work, architecture, authority);
      thin.push(path);
      slices.push(await fileIdentity(path, architecture));
    }
    command('lipo', ['-create', ...thin, '-output', temporaryDylib]);
    await assertRegularFile(temporaryDylib, 'universal ONNX Runtime dylib');
    verifyRuntimePolicy(temporaryDylib, authority);

    const dylibPath = join(output, authority.dylibName);
    await rename(temporaryDylib, dylibPath);
    await symlink(authority.linkerAliasTarget, join(output, authority.linkerAlias));
    const manifest = await createRuntimeManifest(
      authority,
      authorityDigest,
      dylibPath,
      slices,
    );
    await writeFile(join(output, 'runtime.json'), manifest, {
      encoding: 'utf8',
      flag: 'wx',
      mode: 0o600,
    });
    const identity = await verifyPreparedRuntime(
      output,
      authority,
      authorityDigest,
    );
    await emitOutputs(output, identity, authorityDigest);
  } finally {
    await rm(work, { recursive: true, force: true });
  }
  process.stdout.write(
    `prepared universal macOS ONNX Runtime ${authority.version}; ${ORT_LIB_LOCATION}=${output}\n`,
  );
}

if (
  process.argv[1] &&
  fileURLToPath(import.meta.url) === resolve(process.argv[1])
) {
  prepare(process.argv.slice(2)).catch((error) => {
    process.stderr.write(
      `${error instanceof Error ? error.message : String(error)}\n`,
    );
    process.exitCode = 1;
  });
}
