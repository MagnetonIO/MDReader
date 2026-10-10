import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import { baselineSource, readingPositionPackage } from './reading-position-package.mjs';

test('the original package remains the default without admitting another source', () => {
  assert.equal(readingPositionPackage().sourceCommit, baselineSource);
  for (const source of ['', 'b28d487', 'b28d4874239ce8ba89ac084d1cdb018d0e180cfe', 'c367d56d406c01cc9b36dbfd74bc7c9bdc45060f', '0'.repeat(40), '../candidate', 'unknown']) {
    assert.throws(() => readingPositionPackage(source, true), /exact reviewed package/);
  }
});

test('the corrected source cannot run a new import or accept a truthy opt-in', () => {
  const source = '2dcc8fcdcd3ab2cf77a1ef8ac497c5b3302fc742';
  for (const resume of [false, undefined, 'true', 1, {}]) {
    assert.throws(() => readingPositionPackage(source, resume), /retained two-document resume/);
  }
  assert.equal(readingPositionPackage(source, true).apkSha256,
    'e8a7904b8bbe6e2f2440f269e260afb5a166ba7b592c8fb9da1c23bb85933acb');
});
