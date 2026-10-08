import { constants, closeSync, fstatSync, fsyncSync, lstatSync, openSync, readSync, unlinkSync, writeFileSync } from 'node:fs';

export function validCommit(value) {
  return typeof value === 'string' && /^[0-9a-f]{40}$/.test(value) && value !== '0'.repeat(40);
}

/** Exclusive creation; cleanup never removes a replaced/changed file or follows a symlink. */
export function stageHandoff(path, commit) {
  if (!validCommit(commit)) throw Error('release source identity must be a lowercase nonzero Git SHA');
  const contents = Buffer.from(`${commit}\n`);
  let descriptor;
  try {
    descriptor = openSync(path, constants.O_RDWR | constants.O_CREAT | constants.O_EXCL, 0o600);
  } catch {
    throw Error('cannot exclusively create release source handoff; inspect any existing handoff manually');
  }
  const identity = fstatSync(descriptor);
  try {
    writeFileSync(descriptor, contents);
    fsyncSync(descriptor);
  } catch (error) {
    closeSync(descriptor);
    throw error; // Keep any partial owned handoff visible for explicit recovery.
  }
  // Retain the original inode until cleanup, preventing inode reuse after replacement.
  return () => {
    try {
      const current = lstatSync(path);
      if (!current.isFile() || current.dev !== identity.dev || current.ino !== identity.ino || current.nlink !== 1 || current.size !== 41) {
        throw Error('refusing to remove replaced release source handoff');
      }
      const opened = fstatSync(descriptor);
      const buffer = Buffer.alloc(42);
      const length = readSync(descriptor, buffer, 0, buffer.length, 0);
      if (opened.nlink !== 1 || opened.size !== 41 || length !== 41 || !buffer.subarray(0, length).equals(contents)) {
        throw Error('refusing to remove changed release source handoff');
      }
      const final = lstatSync(path);
      if (!final.isFile() || final.dev !== identity.dev || final.ino !== identity.ino || final.nlink !== 1) {
        throw Error('refusing to remove replaced release source handoff');
      }
      unlinkSync(path);
    } finally {
      closeSync(descriptor);
    }
  };
}
