#!/usr/bin/env node

import {
  lstatSync,
  realpathSync,
  renameSync,
  rmSync,
} from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';

function fail(message) {
  throw new Error(`credentialed output publication refused: ${message}`);
}

function exactDirectory(path, label) {
  const metadata = lstatSync(path);
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
    fail(`${label} is not an exact directory`);
  }
  return metadata;
}

function main() {
  const [candidateRootInput, outputRootInput, runnerTempInput] =
    process.argv.slice(2);
  if (
    candidateRootInput === undefined ||
    outputRootInput === undefined ||
    runnerTempInput === undefined
  ) {
    throw new Error(
      'usage: publish-credentialed-output.mjs <candidate-root> <output-root> <runner-temp>',
    );
  }

  const runnerTemp = realpathSync(runnerTempInput);
  const runnerMetadata = exactDirectory(runnerTemp, 'RUNNER_TEMP');

  const outputParent = realpathSync(dirname(resolve(outputRootInput)));
  const outputRoot = join(outputParent, basename(outputRootInput));
  if (
    outputParent !== runnerTemp ||
    basename(outputRoot) !== 'mdreader-credentialed-output'
  ) {
    fail('output root must be the dedicated path in RUNNER_TEMP');
  }

  const candidateInput = resolve(candidateRootInput);
  const candidateMetadata = exactDirectory(candidateInput, 'candidate root');
  const candidateRoot = realpathSync(candidateInput);
  const candidateParent = realpathSync(dirname(candidateRoot));
  if (
    basename(candidateRoot) !== 'mdreader-credentialed-output' ||
    dirname(candidateParent) !== runnerTemp ||
    !basename(candidateParent).startsWith('mdreader-credentialed-candidate.')
  ) {
    fail('candidate root is not an isolated path in RUNNER_TEMP');
  }
  if (candidateMetadata.dev !== runnerMetadata.dev) {
    fail('candidate and final output are not on the same trusted filesystem');
  }

  rmSync(outputRoot, { recursive: true, force: true });
  renameSync(candidateRoot, outputRoot);
}

main();
