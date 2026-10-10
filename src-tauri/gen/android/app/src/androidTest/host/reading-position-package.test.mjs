import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import { baselineSource, readingPositionPackage } from './reading-position-package.mjs';

test('the original package remains the default without admitting another source', () => {
  assert.equal(readingPositionPackage().sourceCommit, baselineSource);
  for (const source of ['', 'b28d487', '0'.repeat(40), '../candidate', 'unknown']) {
    assert.throws(() => readingPositionPackage(source, true), /exact reviewed package/);
  }
});

test('the corrected source cannot run a new import or accept a truthy opt-in', () => {
  const source = 'b28d4874239ce8ba89ac084d1cdb018d0e180cfe';
  for (const resume of [false, undefined, 'true', 1, {}]) {
    assert.throws(() => readingPositionPackage(source, resume), /retained two-document resume/);
  }
  assert.equal(readingPositionPackage(source, true).apkSha256,
    '06798c8e3291a890e59d6608f074a5c0b9684c6efbcd86c7696582379baea46e');
});
