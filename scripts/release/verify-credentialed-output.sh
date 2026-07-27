#!/usr/bin/env bash
set -euo pipefail

PLATFORM="${1:?platform is required}"
VERSION="${2:?version is required}"
SOURCE_COMMIT="${3:?source commit is required}"
TARGET="${4:?target is required}"
BUNDLES="${5:?bundles are required}"
DOWNLOAD_ROOT="${6:?credentialed archive download root is required}"
OUTPUT_ROOT="${7:?credentialed output root is required}"
UPDATER_PUBLIC_KEY_SHA256="${8:?updater public key fingerprint is required}"

export PLATFORM DOWNLOAD_ROOT OUTPUT_ROOT
node <<'NODE'
const { createHash } = require('node:crypto');
const { execFileSync } = require('node:child_process');
const {
  lstatSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  mkdirSync,
} = require('node:fs');
const { basename, join, resolve } = require('node:path');
const downloadRoot = realpathSync(process.env.DOWNLOAD_ROOT);
const names = readdirSync(downloadRoot);
if (names.length !== 1) {
  throw new Error('exactly one hash-addressed credentialed archive is required');
}
const artifactRoot = join(downloadRoot, names[0]);
const expectedPrefix = `mdreader-bundled-${process.env.PLATFORM}-`;
const artifactName = basename(artifactRoot);
if (
  !artifactName.startsWith(expectedPrefix) ||
  !/^[0-9a-f]{64}$/.test(artifactName.slice(expectedPrefix.length))
) {
  throw new Error('credentialed archive artifact name is not hash-addressed');
}
const entries = readdirSync(artifactRoot);
if (
  entries.length !== 1 ||
  entries[0] !== 'credentialed-output.tar'
) {
  throw new Error('credentialed archive handoff contains an unexpected file');
}
const archivePath = join(artifactRoot, entries[0]);
const metadata = lstatSync(archivePath);
if (!metadata.isFile() || metadata.isSymbolicLink()) {
  throw new Error('credentialed archive is not an exact regular file');
}
const actual = createHash('sha256')
  .update(readFileSync(archivePath))
  .digest('hex');
if (actual !== artifactName.slice(expectedPrefix.length)) {
  throw new Error('credentialed archive bytes do not match the artifact hash address');
}
const outputRoot = resolve(process.env.OUTPUT_ROOT);
if (basename(outputRoot) !== 'mdreader-credentialed-output') {
  throw new Error('credentialed extraction root is not the dedicated runner path');
}
rmSync(outputRoot, { recursive: true, force: true });
mkdirSync(outputRoot, { recursive: true, mode: 0o700 });
const archiveEntries = execFileSync('tar', ['-tf', archivePath], {
  encoding: 'utf8',
})
  .split(/\r?\n/)
  .filter(Boolean);
if (archiveEntries.length === 0) {
  throw new Error('credentialed archive is empty');
}
for (const encoded of archiveEntries) {
  const path = encoded.replace(/^(?:\.\/)+/, '').replace(/\/$/, '');
  if (path === '') continue;
  const segments = path.split('/');
  if (
    encoded.startsWith('/') ||
    encoded.includes('\\') ||
    segments.some((segment) => segment === '' || segment === '..') ||
    (path !== 'credentialed-output.json' &&
      path !== 'release-config.json' &&
      path !== 'bundle' &&
      !path.startsWith('bundle/'))
  ) {
    throw new Error(`credentialed archive contains an unsafe path: ${encoded}`);
  }
}
execFileSync('tar', ['-xf', archivePath, '-C', outputRoot], {
  stdio: 'pipe',
});
NODE

pnpm exec tsx scripts/release/release-cli.ts verify-credentialed-output \
  --platform "$PLATFORM" \
  --version "$VERSION" \
  --source-commit "$SOURCE_COMMIT" \
  --target "$TARGET" \
  --bundles "$BUNDLES" \
  --updater-public-key-sha256 "$UPDATER_PUBLIC_KEY_SHA256" \
  --root "$OUTPUT_ROOT" \
  --manifest "$OUTPUT_ROOT/credentialed-output.json"
