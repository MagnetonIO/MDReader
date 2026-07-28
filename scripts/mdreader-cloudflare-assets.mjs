import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import {
  chmodSync,
  closeSync,
  constants,
  fstatSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readSync,
  realpathSync,
  readdirSync,
  renameSync,
  rmSync,
  writeFileSync,
  writeSync,
} from 'node:fs';
import { platform } from 'node:os';
import {
  basename,
  dirname,
  isAbsolute,
  join,
  parse,
  relative,
  resolve,
  sep,
} from 'node:path';

const MIB = 1024 * 1024;
const PINNED_COPY_PLATFORMS = new Set([
  'aix',
  'darwin',
  'freebsd',
  'linux',
  'openbsd',
  'sunos',
]);

export const MDREADER_CLOUDFLARE_ASSET_LIMITS = Object.freeze({
  maxFiles: 20_000,
  maxFileBytes: 25 * MIB,
  maxTotalBytes: 64 * MIB,
});

function displayBytes(value) {
  return value.toLocaleString('en-US');
}

function safeRelativePath(path, name) {
  assert.equal(typeof path, 'string', `${name} must be a relative path`);
  assert.equal(isAbsolute(path), false, `${name} must be a relative path`);
  assert.equal(path.includes('\\'), false, `${name} must use forward slashes`);
  assert.equal(path.includes('\0'), false, `${name} contains a null byte`);
  const segments = path.split('/');
  assert.ok(
    segments.length > 0
      && segments.every((segment) => segment !== '' && segment !== '.' && segment !== '..'),
    `${name} contains an unsafe path segment: ${path}`,
  );
  return segments;
}

/**
 * Returns the conservative collision key used for all supported publish
 * filesystems. NFKC covers compatibility characters (including long s), and
 * the supplemental default-case folds cover sharp-s and final sigma, which
 * are folded by common case-insensitive Unicode filesystems but not by
 * JavaScript's simple `toLowerCase`.
 */
function destinationNamespaceKey(path) {
  return safeRelativePath(path, 'Cloudflare asset destination')
    .map((segment) => segment
      .normalize('NFKC')
      .toLowerCase()
      .replaceAll('\u00df', 'ss')
      .replaceAll('\u03c2', '\u03c3')
      .normalize('NFC'))
    .join('/');
}

function snapshotIdentity(metadata, path, kind, name) {
  assert.equal(
    metadata.isSymbolicLink(),
    false,
    `Cloudflare asset source rejects symbolic link ${name}`,
  );
  assert.equal(
    kind === 'file' ? metadata.isFile() : metadata.isDirectory(),
    true,
    `Cloudflare asset source is not a regular ${kind}: ${name}`,
  );
  assert.ok(
    metadata.dev !== 0n && metadata.ino !== 0n,
    `Cloudflare asset source cannot establish stable identity for ${name} at ${path}`,
  );
  if (kind === 'file') {
    assert.equal(
      metadata.nlink,
      1n,
      `Cloudflare asset source rejects a file with multiple links: ${name}`,
    );
  }
  return Object.freeze({
    ctimeNs: metadata.ctimeNs,
    dev: metadata.dev,
    ino: metadata.ino,
    kind,
    mode: metadata.mode,
    mtimeNs: metadata.mtimeNs,
    nlink: metadata.nlink,
    size: metadata.size,
  });
}

function lstatBigInt(path, name) {
  try {
    return lstatSync(path, { bigint: true });
  } catch (error) {
    throw new Error(
      `Cloudflare asset source identity changed or disappeared for ${name}: ${error.message}`,
      { cause: error },
    );
  }
}

function snapshotDirectory(path, name, canonical = false) {
  const absolutePath = resolve(path);
  const identity = snapshotIdentity(
    lstatBigInt(absolutePath, name),
    absolutePath,
    'directory',
    name,
  );
  if (canonical) {
    assert.equal(
      realpathSync.native(absolutePath),
      absolutePath,
      `Cloudflare asset source root is not canonical: ${name}`,
    );
  }
  return Object.freeze({ identity, name, path: absolutePath });
}

function snapshotCanonicalDirectoryChain(path, name) {
  const absolutePath = resolve(path);
  const requestedRoot = snapshotDirectory(absolutePath, name);
  assert.equal(
    realpathSync.native(absolutePath),
    absolutePath,
    `Cloudflare asset source root is not canonical: ${name}`,
  );
  const root = parse(absolutePath).root;
  const segments = relative(root, absolutePath).split(sep).filter(Boolean);
  const ancestors = [
    absolutePath === root
      ? requestedRoot
      : snapshotDirectory(root, `${name} filesystem root`),
  ];
  let current = root;
  for (const segment of segments) {
    current = join(current, segment);
    ancestors.push(
      current === absolutePath
        ? requestedRoot
        : snapshotDirectory(current, `${name} ancestor ${segment}`),
    );
  }
  return Object.freeze(ancestors);
}

function assertSameIdentity(metadata, expected, name, role = 'source') {
  const actualKind = metadata.isFile()
    ? 'file'
    : metadata.isDirectory()
      ? 'directory'
      : 'other';
  assert.equal(
    metadata.isSymbolicLink(),
    false,
    `Cloudflare asset ${role} rejects symbolic link ${name}`,
  );
  assert.equal(
    actualKind,
    expected.kind,
    `Cloudflare asset ${role} identity changed for ${name}`,
  );
  const fields = expected.kind === 'file'
    ? ['dev', 'ino', 'mode', 'nlink', 'size', 'mtimeNs', 'ctimeNs']
    : ['dev', 'ino', 'mode'];
  for (const field of fields) {
    assert.equal(
      metadata[field],
      expected[field],
      `Cloudflare asset ${role} identity changed for ${name} (${field})`,
    );
  }
}

function assertCurrentDirectory(directory, role = 'source ancestor') {
  assertSameIdentity(
    lstatBigInt(directory.path, directory.name),
    directory.identity,
    directory.name,
    role,
  );
}

function snapshotFile(root, relativePath, sourceName, rootAncestors = [root]) {
  const segments = safeRelativePath(relativePath, sourceName);
  const ancestors = [...rootAncestors];
  let current = root.path;
  for (const segment of segments.slice(0, -1)) {
    current = join(current, segment);
    ancestors.push(snapshotDirectory(current, `${sourceName} ancestor ${segment}`));
  }
  const path = join(current, segments.at(-1));
  const identity = snapshotIdentity(
    lstatBigInt(path, sourceName),
    path,
    'file',
    sourceName,
  );
  return Object.freeze({
    ancestors: Object.freeze(ancestors),
    identity,
    path,
    relativePath,
    sourceName,
  });
}

/**
 * Captures one source file and every intended directory ancestor beneath a
 * canonical root. The returned identity is rechecked at the effect boundary.
 */
export function snapshotMdreaderCloudflareSourceFile(
  rootPath,
  relativePath,
  sourceName,
) {
  const rootAncestors = snapshotCanonicalDirectoryChain(
    rootPath,
    `${sourceName} root`,
  );
  const root = rootAncestors.at(-1);
  return snapshotFile(root, relativePath, sourceName, rootAncestors);
}

/**
 * Captures every regular file in a source tree without following links.
 */
export function snapshotMdreaderCloudflareSourceTree(rootPath, sourceName) {
  const rootAncestors = snapshotCanonicalDirectoryChain(rootPath, sourceName);
  const root = rootAncestors.at(-1);
  const files = [];

  function walk(current, relativeDirectory, ancestors) {
    for (const entry of readdirSync(current).sort()) {
      const path = join(current, entry);
      const relativePath = relativeDirectory
        ? `${relativeDirectory}/${entry}`
        : entry;
      const name = `${sourceName}/${relativePath}`;
      const metadata = lstatBigInt(path, name);
      assert.equal(
        metadata.isSymbolicLink(),
        false,
        `Cloudflare asset source rejects symbolic link ${name}`,
      );
      if (metadata.isDirectory()) {
        const directory = Object.freeze({
          identity: snapshotIdentity(metadata, path, 'directory', name),
          name,
          path,
        });
        walk(path, relativePath, [...ancestors, directory]);
        continue;
      }
      const identity = snapshotIdentity(metadata, path, 'file', name);
      files.push(Object.freeze({
        ancestors: Object.freeze(ancestors),
        identity,
        path,
        relativePath,
        sourceName: name,
      }));
    }
  }

  walk(root.path, '', rootAncestors);
  return Object.freeze(files);
}

/**
 * Fails explicitly if the host cannot guarantee no-follow source opens.
 */
export function assertMdreaderPinnedCopySupport() {
  const hostPlatform = platform();
  assert.ok(
    PINNED_COPY_PLATFORMS.has(hostPlatform),
    `Safe pinned Cloudflare asset staging is unavailable on ${hostPlatform}: Unix file identity semantics are required`,
  );
  assert.ok(
    Number.isInteger(constants.O_NOFOLLOW) && constants.O_NOFOLLOW !== 0,
    `Safe pinned Cloudflare asset staging is unavailable on ${hostPlatform}: O_NOFOLLOW is required`,
  );
  return constants.O_NOFOLLOW;
}

function invokeSourceOpenSeam(source, destination) {
  const seam = globalThis[Symbol.for('mdreader.cloudflare.before-source-open')];
  if (seam === undefined) return;
  assert.equal(
    typeof seam,
    'function',
    'Cloudflare source-open seam must be a function',
  );
  const result = seam({
    destination,
    source: source.path,
    sourceName: source.sourceName,
  });
  assert.equal(
    result instanceof Promise,
    false,
    'Cloudflare source-open seam must be synchronous',
  );
}

function invokePostCheckSourceOpenSeam(source, destination) {
  const seam =
    globalThis[Symbol.for('mdreader.cloudflare.after-source-ancestor-check')];
  if (seam === undefined) return;
  assert.equal(
    typeof seam,
    'function',
    'Cloudflare post-check source-open seam must be a function',
  );
  const result = seam({
    destination,
    source: source.path,
    sourceName: source.sourceName,
  });
  assert.equal(
    result instanceof Promise,
    false,
    'Cloudflare post-check source-open seam must be synchronous',
  );
}

function consumePinnedSource(
  source,
  { destination = undefined, invokeSeam = false, onChunk, onReady },
) {
  const noFollow = assertMdreaderPinnedCopySupport();
  if (invokeSeam) invokeSourceOpenSeam(source, destination);
  for (const ancestor of source.ancestors) assertCurrentDirectory(ancestor);
  if (invokeSeam) invokePostCheckSourceOpenSeam(source, destination);

  let descriptor;
  try {
    descriptor = openSync(source.path, constants.O_RDONLY | noFollow);
  } catch (error) {
    throw new Error(
      `Cloudflare asset source no-follow open failed for ${source.sourceName}: ${error.message}`,
      { cause: error },
    );
  }

  try {
    for (const ancestor of source.ancestors) assertCurrentDirectory(ancestor);
    const before = fstatSync(descriptor, { bigint: true });
    assertSameIdentity(before, source.identity, source.sourceName);
    onReady?.();

    const hash = createHash('sha256');
    const buffer = Buffer.allocUnsafe(64 * 1024);
    let bytes = 0;
    while (true) {
      const count = readSync(descriptor, buffer, 0, buffer.length, null);
      if (count === 0) break;
      const chunk = buffer.subarray(0, count);
      hash.update(chunk);
      onChunk(chunk);
      bytes += count;
    }

    for (const ancestor of source.ancestors) assertCurrentDirectory(ancestor);
    const after = fstatSync(descriptor, { bigint: true });
    assertSameIdentity(after, source.identity, source.sourceName);
    assert.equal(
      BigInt(bytes),
      source.identity.size,
      `Cloudflare asset source changed while reading ${source.sourceName}`,
    );
    return { bytes, digest: hash.digest('hex') };
  } finally {
    closeSync(descriptor);
  }
}

/**
 * Reads a snapshotted metadata file through its already-verified descriptor.
 */
export function readPinnedMdreaderCloudflareSource(source) {
  const chunks = [];
  const result = consumePinnedSource(source, {
    onChunk(chunk) {
      chunks.push(Buffer.from(chunk));
    },
  });
  return Buffer.concat(chunks, result.bytes);
}

function assertDestinationPlan(plan) {
  assert.ok(Array.isArray(plan), 'Cloudflare destination plan must be an array');
  const sorted = plan
    .map((entry) => ({
      entry,
      namespaceKey: destinationNamespaceKey(entry.destination),
    }))
    .sort((left, right) =>
    left.namespaceKey < right.namespaceKey
      ? -1
      : left.namespaceKey > right.namespaceKey
        ? 1
        : left.entry.destination < right.entry.destination
          ? -1
          : left.entry.destination > right.entry.destination
            ? 1
            : 0);
  const destinations = new Map();
  let totalBytes = 0n;
  for (let index = 0; index < sorted.length; index += 1) {
    const { entry, namespaceKey } = sorted[index];
    assert.ok(entry.source?.identity, `Missing source snapshot for ${entry.destination}`);
    assert.ok(
      index < MDREADER_CLOUDFLARE_ASSET_LIMITS.maxFiles,
      `Cloudflare asset staging contains more than ${displayBytes(MDREADER_CLOUDFLARE_ASSET_LIMITS.maxFiles)} files`,
    );
    assert.ok(
      entry.source.identity.size <= BigInt(MDREADER_CLOUDFLARE_ASSET_LIMITS.maxFileBytes),
      `Cloudflare asset ${entry.destination} is ${displayBytes(Number(entry.source.identity.size))} bytes; maximum is ${displayBytes(MDREADER_CLOUDFLARE_ASSET_LIMITS.maxFileBytes)}`,
    );
    totalBytes += entry.source.identity.size;
    assert.ok(
      totalBytes <= BigInt(MDREADER_CLOUDFLARE_ASSET_LIMITS.maxTotalBytes),
      `Cloudflare asset staging total exceeds ${displayBytes(MDREADER_CLOUDFLARE_ASSET_LIMITS.maxTotalBytes)} bytes`,
    );

    assert.equal(
      destinations.has(namespaceKey),
      false,
      `Cloudflare asset destination ${entry.destination} collides with ${destinations.get(namespaceKey) ?? 'another source'} after NFC and case normalization`,
    );
    let slash = namespaceKey.indexOf('/');
    while (slash !== -1) {
      const ancestor = namespaceKey.slice(0, slash);
      assert.ok(
        !destinations.has(ancestor),
        `Cloudflare asset destination ${entry.destination} collides with ${destinations.get(ancestor) ?? ancestor}`,
      );
      slash = namespaceKey.indexOf('/', slash + 1);
    }
    destinations.set(namespaceKey, entry.destination);
  }
  return sorted.map(({ entry }) => entry);
}

/**
 * Validates exact and file/directory-prefix collisions before staging writes.
 */
export function assertMdreaderCloudflareDestinationPlan(plan) {
  return assertDestinationPlan(plan);
}

function writeAll(descriptor, chunk) {
  let offset = 0;
  while (offset < chunk.length) {
    const written = writeSync(
      descriptor,
      chunk,
      offset,
      chunk.length - offset,
    );
    assert.ok(written > 0, 'Cloudflare staged asset write made no progress');
    offset += written;
  }
}

function ensureDestinationDirectory(root, relativeDirectory, rootSnapshot) {
  assertCurrentDirectory(rootSnapshot, 'destination ancestor');
  const ancestors = [rootSnapshot];
  let current = root;
  if (relativeDirectory === '.') return ancestors;
  for (const segment of safeRelativePath(
    relativeDirectory.split(sep).join('/'),
    'Cloudflare destination directory',
  )) {
    current = join(current, segment);
    try {
      mkdirSync(current, { mode: 0o700 });
    } catch (error) {
      if (error.code !== 'EEXIST') throw error;
    }
    const metadata = lstatSync(current, { bigint: true });
    assert.equal(
      metadata.isSymbolicLink(),
      false,
      `Cloudflare asset destination rejects symbolic link ${current}`,
    );
    assert.equal(
      metadata.isDirectory(),
      true,
      `Cloudflare asset destination ancestor is not a directory: ${current}`,
    );
    ancestors.push(Object.freeze({
      identity: snapshotIdentity(
        metadata,
        current,
        'directory',
        `destination ancestor ${current}`,
      ),
      name: `destination ancestor ${current}`,
      path: current,
    }));
  }
  return ancestors;
}

function digestDestination(path, expectedBytes, ancestors) {
  for (const ancestor of ancestors) {
    assertCurrentDirectory(ancestor, 'destination ancestor');
  }
  const descriptor = openSync(
    path,
    constants.O_RDONLY | assertMdreaderPinnedCopySupport(),
  );
  try {
    const metadata = fstatSync(descriptor, { bigint: true });
    assert.equal(
      metadata.isFile(),
      true,
      `Cloudflare staged asset is not a regular file: ${path}`,
    );
    assert.equal(
      metadata.size,
      BigInt(expectedBytes),
      `Cloudflare staged asset size changed: ${path}`,
    );
    const hash = createHash('sha256');
    const buffer = Buffer.allocUnsafe(64 * 1024);
    let bytes = 0;
    while (true) {
      const count = readSync(descriptor, buffer, 0, buffer.length, null);
      if (count === 0) break;
      hash.update(buffer.subarray(0, count));
      bytes += count;
    }
    assert.equal(bytes, expectedBytes, `Cloudflare staged asset changed: ${path}`);
    return hash.digest('hex');
  } finally {
    closeSync(descriptor);
  }
}

function copyPinnedSource(source, destination, destinationAncestors) {
  let destinationDescriptor;
  try {
    const result = consumePinnedSource(source, {
      destination,
      invokeSeam: true,
      onReady() {
        for (const ancestor of destinationAncestors) {
          assertCurrentDirectory(ancestor, 'destination ancestor');
        }
        destinationDescriptor = openSync(
          destination,
          constants.O_WRONLY
            | constants.O_CREAT
            | constants.O_EXCL
            | assertMdreaderPinnedCopySupport(),
          0o600,
        );
      },
      onChunk(chunk) {
        writeAll(destinationDescriptor, chunk);
      },
    });
    closeSync(destinationDescriptor);
    destinationDescriptor = undefined;
    assert.equal(
      digestDestination(destination, result.bytes, destinationAncestors),
      result.digest,
      `Cloudflare staged asset digest differs from pinned source ${source.sourceName}`,
    );
  } finally {
    if (destinationDescriptor !== undefined) closeSync(destinationDescriptor);
  }
}

function stagedFilePaths(directory) {
  const paths = [];
  function walk(current) {
    for (const entry of readdirSync(current).sort()) {
      const path = join(current, entry);
      const metadata = lstatSync(path);
      if (metadata.isDirectory()) {
        walk(path);
      } else if (metadata.isFile()) {
        paths.push(relative(directory, path).split(sep).join('/'));
      }
    }
  }
  walk(directory);
  return paths.sort();
}

const OUTPUT_RECOVERY_DIRECTORY = '.open-next-recovery';
const OUTPUT_RECOVERY_MARKER = '.mdreader-cloudflare-output-recovery-v1';
const OUTPUT_RECOVERY_MARKER_BYTES =
  'mdreader-cloudflare-output-recovery-v1\n';

function lstatIfPresent(path) {
  try {
    return lstatSync(path, { bigint: true });
  } catch (error) {
    if (error.code === 'ENOENT') return null;
    throw error;
  }
}

function assertPathAbsent(path, name) {
  assert.equal(
    lstatIfPresent(path),
    null,
    `${name} already exists: ${path}`,
  );
}

function snapshotPublishedOutput(destination) {
  const metadata = lstatIfPresent(destination);
  if (metadata === null) return null;
  assert.equal(
    metadata.isSymbolicLink(),
    false,
    `Cloudflare asset destination rejects symbolic link ${destination}`,
  );
  assert.equal(
    metadata.isDirectory(),
    true,
    `Cloudflare asset destination is not a directory: ${destination}`,
  );
  return Object.freeze({
    identity: snapshotIdentity(
      metadata,
      destination,
      'directory',
      'published Cloudflare .open-next output',
    ),
    name: 'published Cloudflare .open-next output',
    path: destination,
  });
}

function invokeSynchronousSeam(symbol, name, detail) {
  const seam = globalThis[Symbol.for(symbol)];
  if (seam === undefined) return;
  assert.equal(typeof seam, 'function', `${name} seam must be a function`);
  const result = seam(detail);
  assert.equal(result instanceof Promise, false, `${name} seam must be synchronous`);
}

function invokePublicationStep(outputStage, step) {
  invokeSynchronousSeam(
    'mdreader.cloudflare.publication-step',
    'Cloudflare publication',
    {
      destination: outputStage.destination,
      parent: outputStage.parent.path,
      stagingRoot: outputStage.stagingRoot,
      step,
    },
  );
}

function invokeCleanupStep(outputStage, step) {
  invokeSynchronousSeam(
    'mdreader.cloudflare.cleanup-step',
    'Cloudflare cleanup',
    {
      destination: outputStage.destination,
      parent: outputStage.parent.path,
      stagingRoot: outputStage.stagingRoot,
      step,
    },
  );
}

/**
 * Deletes one private directory only after the parent and target identities
 * have been checked at the deletion boundary. The caller chooses the seam so
 * tests can prove a pathname rebind is refused rather than recursively erased.
 */
function removePinnedPrivateDirectory({
  parent,
  snapshot,
  role,
  beforeRemove = undefined,
}) {
  beforeRemove?.();
  assertCurrentDirectory(parent, `${role} parent`);
  assertCurrentDirectory(snapshot, role);
  rmSync(snapshot.path, { recursive: true, force: false });
}

function snapshotOutputRecovery(parent) {
  const path = join(parent.path, OUTPUT_RECOVERY_DIRECTORY);
  if (lstatIfPresent(path) === null) return null;
  const container = snapshotDirectory(
    path,
    'Cloudflare output recovery directory',
    true,
  );
  const entries = readdirSync(container.path).sort();
  const allowed = new Set([OUTPUT_RECOVERY_MARKER, 'previous']);
  assert.ok(
    entries.every((entry) => allowed.has(entry)),
    `Cloudflare output recovery directory has unexpected entries: ${entries.join(', ')}`,
  );
  assert.ok(
    entries.length <= 2,
    'Cloudflare output recovery directory has an unsafe layout',
  );

  let marker = null;
  if (entries.includes(OUTPUT_RECOVERY_MARKER)) {
    marker = snapshotFile(
      container,
      OUTPUT_RECOVERY_MARKER,
      'Cloudflare output recovery marker',
      [parent, container],
    );
    assert.equal(
      readPinnedMdreaderCloudflareSource(marker).toString('utf8'),
      OUTPUT_RECOVERY_MARKER_BYTES,
      'Cloudflare output recovery marker is not recognized',
    );
  }

  let previous = null;
  if (entries.includes('previous')) {
    assert.notEqual(
      marker,
      null,
      'Cloudflare output recovery refuses an unmarked previous output',
    );
    previous = snapshotDirectory(
      join(container.path, 'previous'),
      'previous Cloudflare output recovery tree',
      true,
    );
  }
  if (marker === null) {
    assert.equal(
      entries.length,
      0,
      'Cloudflare output recovery refuses an unmarked recovery layout',
    );
  }
  return Object.freeze({ container, marker, previous });
}

function discardOutputRecovery(outputStage, recovery) {
  removePinnedPrivateDirectory({
    parent: outputStage.parent,
    snapshot: recovery.container,
    role: 'Cloudflare output recovery directory',
    beforeRemove() {
      invokePublicationStep(outputStage, 'before-recovery-cleanup');
    },
  });
}

function aggregatePublicationError(primary, additional, message, properties = {}) {
  if (additional.length === 0) return primary;
  const combined = new AggregateError([primary, ...additional], message);
  Object.assign(combined, properties);
  return combined;
}

/**
 * Recovers the one deliberate destination-absence interval in a two-rename
 * publish. A crash after moving the old output leaves it at the fixed,
 * discoverable `.open-next-recovery/previous`; the next invocation restores
 * it before staging or publishing anything new.
 */
function recoverMdreaderCloudflareOutput(parent, destination) {
  const recovery = snapshotOutputRecovery(parent);
  if (recovery === null) return;
  const published = snapshotPublishedOutput(destination);

  if (recovery.previous !== null && published === null) {
    assertCurrentDirectory(parent, 'recovery destination parent');
    assertCurrentDirectory(recovery.container, 'Cloudflare output recovery directory');
    assertCurrentDirectory(recovery.previous, 'previous Cloudflare output recovery tree');
    renameSync(recovery.previous.path, destination);
    assertSameIdentity(
      lstatBigInt(destination, 'recovered Cloudflare output'),
      recovery.previous.identity,
      'recovered Cloudflare output',
      'recovery destination',
    );
  }

  try {
    discardOutputRecovery({ destination, parent, stagingRoot: undefined }, recovery);
  } catch (cleanupError) {
    throw new AggregateError(
      [
        new Error('Cloudflare output recovery completed but recovery cleanup failed'),
        cleanupError,
      ],
      'Cloudflare output recovery cleanup failed',
    );
  }
}

/**
 * Creates a bounded-name, same-parent private tree for the complete Worker
 * output while leaving any previously published .open-next tree untouched.
 */
export function createMdreaderCloudflareOutputStage(openNext) {
  const absoluteOpenNext = resolve(openNext);
  assert.equal(
    basename(absoluteOpenNext),
    '.open-next',
    'Cloudflare staging may replace only the scoped .open-next directory',
  );
  const parent = snapshotDirectory(
    dirname(absoluteOpenNext),
    'Cloudflare output parent',
    true,
  );

  recoverMdreaderCloudflareOutput(parent, absoluteOpenNext);
  const destinationSnapshot = snapshotPublishedOutput(absoluteOpenNext);

  assertCurrentDirectory(parent, 'destination parent');
  const stagingRoot = mkdtempSync(join(parent.path, '.open-next-stage-'));
  let stagingSnapshot;
  try {
    chmodSync(stagingRoot, 0o700);
    stagingSnapshot = snapshotDirectory(
      stagingRoot,
      'Cloudflare private output staging root',
      true,
    );
    return Object.freeze({
      destination: absoluteOpenNext,
      destinationSnapshot,
      parent,
      stagingRoot,
      stagingSnapshot,
    });
  } catch (error) {
    const cleanupErrors = [];
    if (stagingSnapshot !== undefined) {
      try {
        removePinnedPrivateDirectory({
          parent,
          snapshot: stagingSnapshot,
          role: 'Cloudflare private output staging root',
        });
      } catch (cleanupError) {
        cleanupErrors.push(cleanupError);
      }
    }
    throw aggregatePublicationError(
      error,
      cleanupErrors,
      'Cloudflare output staging creation failed and cleanup was incomplete',
    );
  }
}

function assertPrivateOutputStage(outputStage) {
  assert.ok(
    outputStage?.stagingRoot && outputStage?.destination,
    'Cloudflare output stage is required',
  );
  assert.equal(
    dirname(outputStage.stagingRoot),
    outputStage.parent.path,
    'Cloudflare private output stage must share the publication parent',
  );
  assert.match(
    basename(outputStage.stagingRoot),
    /^\.open-next-stage-[A-Za-z0-9]{6}$/,
    'Cloudflare private output stage has an unsafe name',
  );
  assert.equal(
    basename(outputStage.destination),
    '.open-next',
    'Cloudflare output stage may publish only .open-next',
  );
  assertCurrentDirectory(outputStage.parent, 'destination parent');
  assertCurrentDirectory(outputStage.stagingSnapshot, 'private output staging root');
}

/**
 * Copies a validated asset plan beneath an already-private whole-output tree.
 */
export function stageMdreaderCloudflareAssetPlan(plan, outputStage) {
  const sorted = assertDestinationPlan(plan);
  assertMdreaderPinnedCopySupport();
  assertPrivateOutputStage(outputStage);
  const assets = join(outputStage.stagingRoot, 'assets');

  try {
    lstatSync(assets);
    assert.fail(`Cloudflare private asset destination already exists: ${assets}`);
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  mkdirSync(assets, { mode: 0o700 });
  const assetsSnapshot = snapshotDirectory(
    assets,
    'Cloudflare private asset staging root',
    true,
  );

  for (const entry of sorted) {
    assertPrivateOutputStage(outputStage);
    const destinationAncestors = ensureDestinationDirectory(
      assets,
      dirname(entry.destination),
      assetsSnapshot,
    );
    copyPinnedSource(
      entry.source,
      join(assets, entry.destination),
      destinationAncestors,
    );
  }
  const staged = assertMdreaderCloudflareAssets(assets);
  assert.deepEqual(
    stagedFilePaths(assets),
    sorted.map((entry) => entry.destination).sort(),
    'Cloudflare private staging tree differs from the validated copy plan',
  );
  return staged;
}

function digestStagedOutputFile(path, stagedPath, ancestors) {
  for (const ancestor of ancestors) {
    assertCurrentDirectory(ancestor, 'staged output ancestor');
  }
  const identity = snapshotIdentity(
    lstatBigInt(path, stagedPath),
    path,
    'file',
    stagedPath,
  );
  let descriptor;
  try {
    descriptor = openSync(
      path,
      constants.O_RDONLY | assertMdreaderPinnedCopySupport(),
    );
  } catch (error) {
    throw new Error(
      `Cloudflare staged output no-follow open failed for ${stagedPath}: ${error.message}`,
      { cause: error },
    );
  }

  try {
    for (const ancestor of ancestors) {
      assertCurrentDirectory(ancestor, 'staged output ancestor');
    }
    assertSameIdentity(
      fstatSync(descriptor, { bigint: true }),
      identity,
      stagedPath,
      'staged output',
    );
    const hash = createHash('sha256');
    const buffer = Buffer.allocUnsafe(64 * 1024);
    let bytes = 0n;
    while (true) {
      const count = readSync(descriptor, buffer, 0, buffer.length, null);
      if (count === 0) break;
      hash.update(buffer.subarray(0, count));
      bytes += BigInt(count);
    }
    for (const ancestor of ancestors) {
      assertCurrentDirectory(ancestor, 'staged output ancestor');
    }
    assertSameIdentity(
      fstatSync(descriptor, { bigint: true }),
      identity,
      stagedPath,
      'staged output',
    );
    assertSameIdentity(
      lstatBigInt(path, stagedPath),
      identity,
      stagedPath,
      'staged output path',
    );
    assert.equal(
      bytes,
      identity.size,
      `Cloudflare staged output changed while hashing ${stagedPath}`,
    );
    return Object.freeze({
      kind: 'file',
      path: stagedPath,
      sha256: hash.digest('hex'),
      size: bytes,
    });
  } finally {
    closeSync(descriptor);
  }
}

/**
 * Reopens and hashes every file in the complete private Worker output.
 */
export function snapshotMdreaderCloudflareOutputManifest(outputStage) {
  assertPrivateOutputStage(outputStage);
  const manifest = [];

  function walk(current, relativeDirectory, ancestors) {
    for (const ancestor of ancestors) {
      assertCurrentDirectory(ancestor, 'staged output ancestor');
    }
    const entries = readdirSync(current).sort();
    for (const entry of entries) {
      const path = join(current, entry);
      const stagedPath = relativeDirectory
        ? `${relativeDirectory}/${entry}`
        : entry;
      const metadata = lstatBigInt(path, stagedPath);
      assert.equal(
        metadata.isSymbolicLink(),
        false,
        `Cloudflare staged output rejects symbolic link ${stagedPath}`,
      );
      if (metadata.isDirectory()) {
        const directory = Object.freeze({
          identity: snapshotIdentity(
            metadata,
            path,
            'directory',
            stagedPath,
          ),
          name: stagedPath,
          path,
        });
        manifest.push(Object.freeze({ kind: 'directory', path: stagedPath }));
        walk(path, stagedPath, [...ancestors, directory]);
        assertCurrentDirectory(directory, 'staged output directory');
        continue;
      }
      assert.equal(
        metadata.isFile(),
        true,
        `Cloudflare staged output rejects non-regular entry ${stagedPath}`,
      );
      manifest.push(digestStagedOutputFile(path, stagedPath, ancestors));
    }
    for (const ancestor of ancestors) {
      assertCurrentDirectory(ancestor, 'staged output ancestor');
    }
  }

  walk(
    outputStage.stagingRoot,
    '',
    [outputStage.parent, outputStage.stagingSnapshot],
  );
  return Object.freeze(manifest);
}

function invokeBeforePublicationSeam(stagingRoot) {
  const seam = globalThis[Symbol.for('mdreader.cloudflare.before-publication')];
  if (seam === undefined) return;
  assert.equal(
    typeof seam,
    'function',
    'Cloudflare prepublication seam must be a function',
  );
  const result = seam({ stagingRoot });
  assert.equal(
    result instanceof Promise,
    false,
    'Cloudflare prepublication seam must be synchronous',
  );
}

function assertPublicationDestination(outputStage) {
  if (outputStage.destinationSnapshot !== null) {
    assertCurrentDirectory(
      outputStage.destinationSnapshot,
      'publication destination',
    );
    return;
  }
  try {
    lstatSync(outputStage.destination);
    assert.fail(
      `Cloudflare publication destination appeared during staging: ${outputStage.destination}`,
    );
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
}

function createOutputRecovery(outputStage) {
  const path = join(outputStage.parent.path, OUTPUT_RECOVERY_DIRECTORY);
  assertCurrentDirectory(outputStage.parent, 'destination parent');
  assertPathAbsent(path, 'Cloudflare output recovery directory');
  mkdirSync(path, { mode: 0o700 });
  let container;
  try {
    chmodSync(path, 0o700);
    container = snapshotDirectory(
      path,
      'Cloudflare output recovery directory',
      true,
    );
    writeFileSync(join(path, OUTPUT_RECOVERY_MARKER), OUTPUT_RECOVERY_MARKER_BYTES, {
      encoding: 'utf8',
      flag: 'wx',
      mode: 0o600,
    });
    const recovery = snapshotOutputRecovery(outputStage.parent);
    assert.notEqual(recovery, null, 'Cloudflare output recovery directory disappeared');
    return recovery;
  } catch (error) {
    const cleanupErrors = [];
    if (container !== undefined) {
      try {
        removePinnedPrivateDirectory({
          parent: outputStage.parent,
          snapshot: container,
          role: 'Cloudflare output recovery directory',
        });
      } catch (cleanupError) {
        cleanupErrors.push(cleanupError);
      }
    }
    throw aggregatePublicationError(
      error,
      cleanupErrors,
      'Cloudflare output recovery creation failed and cleanup was incomplete',
    );
  }
}

function movePreviousOutputToRecovery(outputStage, recovery, onMoved) {
  const previous = join(recovery.container.path, 'previous');
  invokePublicationStep(outputStage, 'before-backup-rename');
  assertCurrentDirectory(outputStage.parent, 'destination parent');
  assertCurrentDirectory(recovery.container, 'Cloudflare output recovery directory');
  assertPublicationDestination(outputStage);
  assertPathAbsent(previous, 'Cloudflare previous output recovery tree');
  renameSync(outputStage.destination, previous);
  onMoved();
  const snapshot = snapshotDirectory(
    previous,
    'previous Cloudflare output recovery tree',
    true,
  );
  assertSameIdentity(
    lstatBigInt(previous, 'previous Cloudflare output'),
    outputStage.destinationSnapshot.identity,
    'previous Cloudflare output',
    'publication backup',
  );
  invokePublicationStep(outputStage, 'after-backup-identity');
  return snapshot;
}

function moveReplacementIntoDestination(outputStage, onMoved) {
  invokePublicationStep(outputStage, 'before-replacement-rename');
  assertCurrentDirectory(outputStage.parent, 'destination parent');
  assertCurrentDirectory(
    outputStage.stagingSnapshot,
    'private output staging root',
  );
  assertPathAbsent(outputStage.destination, 'Cloudflare publication destination');
  renameSync(outputStage.stagingRoot, outputStage.destination);
  onMoved();
  assertSameIdentity(
    lstatBigInt(outputStage.destination, 'published Cloudflare output'),
    outputStage.stagingSnapshot.identity,
    'published Cloudflare output',
    'publication destination',
  );
  invokePublicationStep(outputStage, 'after-replacement-identity');
}

function moveReplacementBackToStage(outputStage) {
  invokePublicationStep(outputStage, 'before-rollback-replacement');
  assertCurrentDirectory(outputStage.parent, 'rollback destination parent');
  assertSameIdentity(
    lstatBigInt(outputStage.destination, 'published Cloudflare output'),
    outputStage.stagingSnapshot.identity,
    'published Cloudflare output',
    'rollback destination',
  );
  assertPathAbsent(outputStage.stagingRoot, 'Cloudflare private output staging root');
  renameSync(outputStage.destination, outputStage.stagingRoot);
  assertCurrentDirectory(outputStage.stagingSnapshot, 'private output staging root');
}

function restorePreviousOutput(outputStage, recovery, previous) {
  invokePublicationStep(outputStage, 'before-rollback-restore');
  assertCurrentDirectory(outputStage.parent, 'rollback destination parent');
  assertCurrentDirectory(recovery.container, 'Cloudflare output recovery directory');
  assertCurrentDirectory(previous, 'previous Cloudflare output recovery tree');
  assertPathAbsent(outputStage.destination, 'Cloudflare rollback destination');
  renameSync(previous.path, outputStage.destination);
  assertSameIdentity(
    lstatBigInt(outputStage.destination, 'restored Cloudflare output'),
    outputStage.destinationSnapshot.identity,
    'restored Cloudflare output',
    'rollback destination',
  );
}

/**
 * Revalidates the complete output manifest and publishes it with a
 * discoverable crash-recovery protocol. The two same-parent renames have a
 * short destination-absence interval, so the prior output is kept at the
 * fixed `.open-next-recovery/previous` until the replacement has been pinned
 * at its destination and recovery cleanup succeeds.
 */
export function publishMdreaderCloudflareOutput(outputStage, manifest) {
  assertPrivateOutputStage(outputStage);
  invokeBeforePublicationSeam(outputStage.stagingRoot);
  const reverified = snapshotMdreaderCloudflareOutputManifest(outputStage);
  assert.deepEqual(
    reverified,
    manifest,
    'Cloudflare staged output manifest differs immediately before publication',
  );
  assertPrivateOutputStage(outputStage);
  assertPublicationDestination(outputStage);

  let recovery = null;
  let previous = null;
  let previousMoved = false;
  let replacementMoved = false;
  try {
    if (outputStage.destinationSnapshot !== null) {
      recovery = createOutputRecovery(outputStage);
      previous = movePreviousOutputToRecovery(outputStage, recovery, () => {
        previousMoved = true;
      });
    }
    moveReplacementIntoDestination(outputStage, () => {
      replacementMoved = true;
    });
  } catch (error) {
    const rollbackErrors = [];
    if (replacementMoved) {
      try {
        moveReplacementBackToStage(outputStage);
        replacementMoved = false;
      } catch (rollbackError) {
        rollbackErrors.push(rollbackError);
      }
    }
    if (previousMoved && replacementMoved === false) {
      try {
        restorePreviousOutput(outputStage, recovery, previous);
        previousMoved = false;
      } catch (rollbackError) {
        rollbackErrors.push(rollbackError);
      }
    }

    // A recovery tree is deleted only after its prior output was restored or
    // was never moved. In particular, a failed restore keeps the only prior
    // release in place for manual or next-run recovery.
    if (recovery !== null && previousMoved === false) {
      try {
        discardOutputRecovery(outputStage, recovery);
      } catch (rollbackError) {
        rollbackErrors.push(rollbackError);
      }
    }
    const incomplete = previousMoved || replacementMoved;
    throw aggregatePublicationError(
      error,
      rollbackErrors,
      'Cloudflare output publication failed and rollback was incomplete',
      incomplete ? { mdreaderCloudflarePreserveStage: true } : {},
    );
  }

  if (recovery !== null) {
    try {
      discardOutputRecovery(outputStage, recovery);
    } catch (cleanupError) {
      throw aggregatePublicationError(
        new Error(
          'Cloudflare output publication committed but recovery cleanup failed',
        ),
        [cleanupError],
        'Cloudflare output publication committed with recovery cleanup failure',
        { mdreaderCloudflarePublicationCommitted: true },
      );
    }
  }
}

/**
 * Removes only the validated bounded-name private output tree.
 */
export function discardMdreaderCloudflareOutputStage(outputStage) {
  assert.equal(
    dirname(outputStage.stagingRoot),
    outputStage.parent.path,
    'Cloudflare private output cleanup escaped its parent',
  );
  assert.match(
    basename(outputStage.stagingRoot),
    /^\.open-next-stage-[A-Za-z0-9]{6}$/,
    'Cloudflare private output cleanup target has an unsafe name',
  );
  assertCurrentDirectory(outputStage.parent, 'private output cleanup parent');
  if (lstatIfPresent(outputStage.stagingRoot) === null) return;
  removePinnedPrivateDirectory({
    parent: outputStage.parent,
    snapshot: outputStage.stagingSnapshot,
    role: 'Cloudflare private output staging root',
    beforeRemove() {
      invokeCleanupStep(outputStage, 'before-stage-discard');
    },
  });
}

export function assertMdreaderCloudflareAssets(directory) {
  const root = lstatSync(directory);
  assert.equal(
    root.isSymbolicLink(),
    false,
    `Cloudflare asset staging rejects symbolic link ${directory}`,
  );
  assert.equal(
    root.isDirectory(),
    true,
    `Cloudflare asset staging root is not a directory: ${directory}`,
  );

  let fileCount = 0;
  let totalBytes = 0;
  let largestFileBytes = 0;
  let largestFile = '';

  function walk(current) {
    for (const entry of readdirSync(current).sort()) {
      const path = join(current, entry);
      const stagedPath = relative(directory, path).split(sep).join('/');
      const metadata = lstatSync(path);
      assert.equal(
        metadata.isSymbolicLink(),
        false,
        `Cloudflare asset staging rejects symbolic link ${stagedPath}`,
      );
      if (metadata.isDirectory()) {
        walk(path);
        continue;
      }
      assert.equal(
        metadata.isFile(),
        true,
        `Cloudflare asset staging rejects non-regular entry ${stagedPath}`,
      );

      fileCount += 1;
      assert.ok(
        fileCount <= MDREADER_CLOUDFLARE_ASSET_LIMITS.maxFiles,
        `Cloudflare asset staging contains ${displayBytes(fileCount)} files; maximum is ${displayBytes(MDREADER_CLOUDFLARE_ASSET_LIMITS.maxFiles)}`,
      );
      assert.ok(
        metadata.size <= MDREADER_CLOUDFLARE_ASSET_LIMITS.maxFileBytes,
        `Cloudflare asset ${stagedPath} is ${displayBytes(metadata.size)} bytes; maximum is ${displayBytes(MDREADER_CLOUDFLARE_ASSET_LIMITS.maxFileBytes)}`,
      );

      totalBytes += metadata.size;
      assert.ok(
        totalBytes <= MDREADER_CLOUDFLARE_ASSET_LIMITS.maxTotalBytes,
        `Cloudflare asset staging total is ${displayBytes(totalBytes)} bytes; maximum is ${displayBytes(MDREADER_CLOUDFLARE_ASSET_LIMITS.maxTotalBytes)}`,
      );
      if (metadata.size > largestFileBytes) {
        largestFileBytes = metadata.size;
        largestFile = stagedPath;
      }
    }
  }

  walk(directory);
  return { fileCount, totalBytes, largestFile, largestFileBytes };
}
