import assert from 'node:assert/strict';
import { lstatSync, readdirSync } from 'node:fs';
import { join, relative, sep } from 'node:path';

const MIB = 1024 * 1024;

export const MDREADER_CLOUDFLARE_ASSET_LIMITS = Object.freeze({
  maxFiles: 20_000,
  maxFileBytes: 25 * MIB,
  maxTotalBytes: 64 * MIB,
});

function displayBytes(value) {
  return value.toLocaleString('en-US');
}

export function assertMdreaderCloudflareSourceFile(path, sourceName) {
  const metadata = lstatSync(path);
  assert.equal(
    metadata.isSymbolicLink(),
    false,
    `Cloudflare asset source rejects symbolic link ${sourceName}`,
  );
  assert.equal(
    metadata.isFile(),
    true,
    `Cloudflare asset source is not a regular file: ${sourceName}`,
  );
  return path;
}

export function assertMdreaderCloudflareSourceTree(directory, sourceName) {
  const root = lstatSync(directory);
  assert.equal(
    root.isSymbolicLink(),
    false,
    `Cloudflare asset source rejects symbolic link ${sourceName}`,
  );
  assert.equal(
    root.isDirectory(),
    true,
    `Cloudflare asset source is not a directory: ${sourceName}`,
  );

  function walk(current) {
    for (const entry of readdirSync(current).sort()) {
      const path = join(current, entry);
      const sourcePath = `${sourceName}/${relative(directory, path).split(sep).join('/')}`;
      const metadata = lstatSync(path);
      assert.equal(
        metadata.isSymbolicLink(),
        false,
        `Cloudflare asset source rejects symbolic link ${sourcePath}`,
      );
      if (metadata.isDirectory()) {
        walk(path);
        continue;
      }
      assert.equal(
        metadata.isFile(),
        true,
        `Cloudflare asset source rejects non-regular entry ${sourcePath}`,
      );
    }
  }

  walk(directory);
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
