#!/usr/bin/env node

import { createHash, randomUUID } from 'node:crypto';
import { closeSync, lstatSync, openSync, readFileSync } from 'node:fs';
import {
  mkdir,
  open,
  readFile,
  readdir,
  readlink,
  rename,
  stat,
  symlink,
  unlink,
  writeFile,
} from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const HERE = dirname(fileURLToPath(import.meta.url));
const AUTHORITY_PATH = join(HERE, 'macos-ort-runtime.json');
const HASH = /^[0-9a-f]{64}$/;
const ORT_LIB_LOCATION = 'ORT_LIB_LOCATION';
const MAX_DOWNLOAD_BYTES = 50_000_000;

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

export function validateAuthority(value) {
  const expectedKeys = [
    'architectures',
    'archiveBytes',
    'archiveEntry',
    'archiveSha256',
    'archiveUrl',
    'dylibBytes',
    'dylibName',
    'dylibSha256',
    'installName',
    'linkerAlias',
    'linkerAliasTarget',
    'schemaVersion',
    'version',
  ];
  if (
    value === null ||
    typeof value !== 'object' ||
    Array.isArray(value) ||
    JSON.stringify(Object.keys(value).sort()) !== JSON.stringify(expectedKeys) ||
    value.schemaVersion !== 1 ||
    value.version !== '1.23.2' ||
    value.archiveUrl !==
      'https://github.com/microsoft/onnxruntime/releases/download/v1.23.2/onnxruntime-osx-universal2-1.23.2.tgz' ||
    value.archiveBytes !== 42_818_339 ||
    value.archiveSha256 !==
      '49ae8e3a66ccb18d98ad3fe7f5906b6d7887df8a5edd40f49eb2b14e20885809' ||
    value.archiveEntry !==
      'onnxruntime-osx-universal2-1.23.2/lib/libonnxruntime.1.23.2.dylib' ||
    value.dylibName !== 'libonnxruntime.1.23.2.dylib' ||
    value.linkerAlias !== 'libonnxruntime.dylib' ||
    value.linkerAliasTarget !== value.dylibName ||
    value.dylibBytes !== 74_902_752 ||
    value.dylibSha256 !==
      '6fee21a0dbcaa98fe082cb4f7ed07ec5def439df36198f47b61dc205e7d2a1fa' ||
    JSON.stringify(value.architectures) !==
      JSON.stringify(['arm64', 'x86_64']) ||
    value.installName !== '@rpath/libonnxruntime.1.23.2.dylib' ||
    !HASH.test(value.archiveSha256) ||
    !HASH.test(value.dylibSha256)
  ) {
    throw new Error('macOS ONNX Runtime authority is not the reviewed universal2 identity');
  }
  return value;
}

// `ORT_LIB_LOCATION` is a search directory: ort-sys links whatever library or
// stub it finds there, so an extra `.dylib`, `.tbd`, or `.a` alongside the
// reviewed runtime can silently displace it. The directory is therefore closed
// to exactly the three reviewed entries.
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
  if (
    !metadata.isFile() ||
    metadata.isSymbolicLink() ||
    metadata.nlink !== 1
  ) {
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
    throw new Error('ONNX Runtime archive URL must be credential-free HTTPS');
  }
  const response = await fetch(parsed, {
    redirect: 'follow',
    signal: AbortSignal.timeout(300_000),
  });
  if (!response.ok || !response.body || new URL(response.url).protocol !== 'https:') {
    throw new Error(`ONNX Runtime archive download failed with status ${response.status}`);
  }
  const declaredBytes = Number(response.headers.get('content-length'));
  if (
    Number.isFinite(declaredBytes) &&
    (declaredBytes > MAX_DOWNLOAD_BYTES || declaredBytes !== expectedBytes)
  ) {
    throw new Error('ONNX Runtime archive length differs from its reviewed identity');
  }
  const handle = await open(destination, 'wx', 0o600);
  let bytes = 0;
  try {
    for await (const chunk of response.body) {
      bytes += chunk.byteLength;
      if (bytes > expectedBytes || bytes > MAX_DOWNLOAD_BYTES) {
        throw new Error('ONNX Runtime archive exceeded its reviewed byte length');
      }
      await handle.write(chunk);
    }
  } finally {
    await handle.close();
  }
  if (bytes !== expectedBytes) {
    throw new Error('ONNX Runtime archive was truncated');
  }
}

function commandText(command, args) {
  const result = spawnSync(command, args, {
    encoding: 'utf8',
    maxBuffer: 1024 * 1024,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  if (result.error || result.status !== 0) {
    throw new Error(
      `${command} failed while verifying ONNX Runtime: ${result.error?.message ?? result.stderr.trim()}`,
    );
  }
  return result.stdout.trim();
}

export function validateArchitectures(output, expected) {
  const actual = [...new Set(output.split(/\s+/).filter(Boolean))].sort();
  const wanted = [...expected].sort();
  if (JSON.stringify(actual) !== JSON.stringify(wanted)) {
    throw new Error(
      `ONNX Runtime dylib architectures ${actual.join(',')} do not match ${wanted.join(',')}`,
    );
  }
}

export function validateInstallNames(output, expected, architectureCount) {
  const names = output
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.startsWith('@'));
  if (
    names.length !== architectureCount ||
    names.some((name) => name !== expected)
  ) {
    throw new Error('ONNX Runtime dylib install name is not the reviewed @rpath identity');
  }
}

async function verifyDylib(path, authority) {
  const metadata = await assertRegularFile(path, 'ONNX Runtime dylib');
  if (
    metadata.size !== authority.dylibBytes ||
    (await sha256File(path)) !== authority.dylibSha256
  ) {
    throw new Error('ONNX Runtime dylib bytes differ from the reviewed identity');
  }
  validateArchitectures(
    commandText('lipo', ['-archs', path]),
    authority.architectures,
  );
  validateInstallNames(
    commandText('otool', ['-D', path]),
    authority.installName,
    authority.architectures.length,
  );
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

async function ensureLinkerAlias(path, authority) {
  try {
    await verifyLinkerAlias(path, authority);
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error;
    await symlink(authority.linkerAliasTarget, path);
    await verifyLinkerAlias(path, authority);
  }
}

async function extractDylib(archivePath, destination, authority) {
  const descriptor = openSync(destination, 'wx', 0o600);
  let result;
  try {
    result = spawnSync('tar', ['-xOf', archivePath, authority.archiveEntry], {
      stdio: ['ignore', descriptor, 'pipe'],
      encoding: 'utf8',
      maxBuffer: 1024 * 1024,
    });
  } finally {
    closeSync(descriptor);
  }
  if (result.error || result.status !== 0) {
    throw new Error(
      `failed to extract the reviewed ONNX Runtime dylib: ${result.error?.message ?? result.stderr.trim()}`,
    );
  }
}

function runtimeManifest(authority, authorityBytes) {
  return `${JSON.stringify({
    schemaVersion: 1,
    authoritySha256: sha256(authorityBytes),
    version: authority.version,
    archiveUrl: authority.archiveUrl,
    archiveBytes: authority.archiveBytes,
    archiveSha256: authority.archiveSha256,
    dylib: {
      path: authority.dylibName,
      bytes: authority.dylibBytes,
      sha256: authority.dylibSha256,
      architectures: authority.architectures,
      installName: authority.installName,
    },
    linkerAlias: {
      path: authority.linkerAlias,
      target: authority.linkerAliasTarget,
    },
  }, null, 2)}\n`;
}

async function emitOutputs(
  output,
  dylibPath,
  manifestPath,
  manifest,
  authority,
  authorityBytes,
) {
  const outputLines = [
    `ort_lib_location=${output}`,
    `ort_dylib_path=${dylibPath}`,
    `ort_dylib_sha256=${authority.dylibSha256}`,
    `ort_manifest_path=${manifestPath}`,
    `ort_manifest_sha256=${sha256(manifest)}`,
    `ort_authority_sha256=${sha256(authorityBytes)}`,
  ];
  if (process.env.GITHUB_OUTPUT) {
    await writeFile(process.env.GITHUB_OUTPUT, `${outputLines.join('\n')}\n`, {
      encoding: 'utf8',
      flag: 'a',
    });
  }
}

async function prepare(args) {
  if (process.platform !== 'darwin') {
    throw new Error('universal macOS ONNX Runtime preparation requires macOS tooling');
  }
  const authorityBytes = readFileSync(AUTHORITY_PATH);
  const authority = validateAuthority(
    JSON.parse(authorityBytes.toString('utf8')),
  );
  const output = resolve(requiredOption(args, '--output'));
  const archiveOption = optionalOption(args, '--archive');
  await mkdir(output, { recursive: true, mode: 0o700 });
  const outputMetadata = lstatSync(output);
  if (!outputMetadata.isDirectory() || outputMetadata.isSymbolicLink()) {
    throw new Error('ONNX Runtime output must be one real directory');
  }

  const temporaryArchive = archiveOption
    ? undefined
    : join(output, `.onnxruntime-${randomUUID()}.tgz`);
  const archivePath = archiveOption ? resolve(archiveOption) : temporaryArchive;
  const temporaryDylib = join(output, `.${authority.dylibName}.${randomUUID()}`);
  const dylibPath = join(output, authority.dylibName);
  const linkerAliasPath = join(output, authority.linkerAlias);
  const manifestPath = join(output, 'runtime.json');
  const temporaryManifest = join(output, `.runtime.${randomUUID()}.json`);
  const manifest = runtimeManifest(authority, authorityBytes);

  if (args.includes('--verify')) {
    await assertClosedRuntimeDirectory(output, authority);
    await verifyDylib(dylibPath, authority);
    await verifyLinkerAlias(linkerAliasPath, authority);
    if ((await readFile(manifestPath, 'utf8')) !== manifest) {
      throw new Error('ONNX Runtime manifest differs from the reviewed identity');
    }
    await assertRegularFile(manifestPath, 'ONNX Runtime manifest');
    await emitOutputs(
      output,
      dylibPath,
      manifestPath,
      manifest,
      authority,
      authorityBytes,
    );
    process.stdout.write(
      `verified universal macOS ONNX Runtime ${authority.version}; ${ORT_LIB_LOCATION}=${output}\n`,
    );
    return;
  }

  try {
    if (temporaryArchive) {
      await downloadArchive(
        authority.archiveUrl,
        temporaryArchive,
        authority.archiveBytes,
      );
    }
    const archiveMetadata = await assertRegularFile(
      archivePath,
      'ONNX Runtime archive',
    );
    if (
      archiveMetadata.size !== authority.archiveBytes ||
      (await sha256File(archivePath)) !== authority.archiveSha256
    ) {
      throw new Error('ONNX Runtime archive differs from its reviewed identity');
    }

    try {
      await verifyDylib(dylibPath, authority);
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error;
      await extractDylib(archivePath, temporaryDylib, authority);
      await verifyDylib(temporaryDylib, authority);
      await rename(temporaryDylib, dylibPath);
    }
    await ensureLinkerAlias(linkerAliasPath, authority);

    await writeFile(temporaryManifest, manifest, {
      encoding: 'utf8',
      flag: 'wx',
      mode: 0o600,
    });
    await rename(temporaryManifest, manifestPath);
  } finally {
    for (const path of [
      temporaryArchive,
      temporaryDylib,
      temporaryManifest,
    ]) {
      if (!path) continue;
      try {
        await unlink(path);
      } catch (error) {
        if (error?.code !== 'ENOENT') throw error;
      }
    }
  }

  // Inventoried after the temporary files are gone, so the directory handed to
  // ort-sys is closed to exactly the reviewed runtime.
  await assertClosedRuntimeDirectory(output, authority);
  await emitOutputs(
    output,
    dylibPath,
    manifestPath,
    manifest,
    authority,
    authorityBytes,
  );
  process.stdout.write(
    `prepared universal macOS ONNX Runtime ${authority.version}; ${ORT_LIB_LOCATION}=${output}\n`,
  );
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  prepare(process.argv.slice(2)).catch((error) => {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  });
}
