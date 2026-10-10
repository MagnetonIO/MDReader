import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import { baselineSource, readingPositionPackage } from './reading-position-package.mjs';

test('the original package remains the default without admitting another source', () => {
  assert.equal(readingPositionPackage().sourceCommit, baselineSource);
  for (const source of ['', 'b28d487', 'b28d4874239ce8ba89ac084d1cdb018d0e180cfe', '0'.repeat(40), '../candidate', 'unknown']) {
    assert.throws(() => readingPositionPackage(source, true), /exact reviewed package/);
  }
});

test('the corrected source cannot run a new import or accept a truthy opt-in', () => {
  const source = 'c367d56d406c01cc9b36dbfd74bc7c9bdc45060f';
  for (const resume of [false, undefined, 'true', 1, {}]) {
    assert.throws(() => readingPositionPackage(source, resume), /retained two-document resume/);
  }
  assert.equal(readingPositionPackage(source, true).apkSha256,
    'b2f07994410221d424564aad34211eba556b18f759adb45222ded193bfbb688f');
});
