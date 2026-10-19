import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { makeAx } from './ax-helper.ts';

// Fake helpers are Node scripts run through this Node, so they work on every
// host (Windows cannot exec a shebang script).
const fakeAx = (script: string, pid: number) => makeAx(process.execPath, pid, [script]);

async function fakeAxHelper(
  errorLine: string,
  failuresBeforeSuccess = 1,
): Promise<{ bin: string; attempts: string }> {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-ax-helper-'));
  const bin = join(root, 'axquery.mjs');
  const attempts = join(root, 'attempts');
  await writeFile(
    bin,
    [
      "import { existsSync, readFileSync, writeFileSync } from 'node:fs';",
      `const attempts = ${JSON.stringify(attempts)};`,
      "const count = existsSync(attempts) ? Number(readFileSync(attempts, 'utf8')) + 1 : 1;",
      "writeFileSync(attempts, String(count));",
      `if (count <= ${failuresBeforeSuccess}) { console.error(${JSON.stringify(errorLine)}); process.exit(3); }`,
      "console.log('Sample Project');",
    ].join('\n'),
  );
  return { bin, attempts };
}

async function focusedAxHelper(output: string): Promise<string> {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-ax-focused-'));
  const bin = join(root, 'axquery.mjs');
  await writeFile(
    bin,
    `console.log(${JSON.stringify(output)});\n`,
  );
  return bin;
}

test('a transient AX cannot-complete read is retried and never converted to empty', async () => {
  const helper = await fakeAxHelper('AX_READ_ERROR: AXRole code=-25204');
  assert.equal(fakeAx(helper.bin, 42).headings(), 'Sample Project');
  assert.equal(await readFile(helper.attempts, 'utf8'), '2');
});

test('a WebKit node invalidated during traversal is retried from the current root', async () => {
  const helper = await fakeAxHelper('AX_READ_ERROR: AXDOMClassList code=-25202');
  assert.equal(fakeAx(helper.bin, 42).headings(), 'Sample Project');
  assert.equal(await readFile(helper.attempts, 'utf8'), '2');
});

test('a changing WebKit subtree may stay busy across several bounded retries', async () => {
  const helper = await fakeAxHelper('AX_READ_ERROR: AXDOMClassList code=-25204', 5);
  assert.equal(fakeAx(helper.bin, 42).headings(), 'Sample Project');
  assert.equal(await readFile(helper.attempts, 'utf8'), '6');
});

test('another AX read error remains fail-closed without retry', async () => {
  const helper = await fakeAxHelper('AX_READ_ERROR: AXRole code=-25205');
  assert.throws(() => fakeAx(helper.bin, 42).headings());
  assert.equal(await readFile(helper.attempts, 'utf8'), '1');
});

test('focused parses only the helper boolean contract', async () => {
  assert.equal(fakeAx(await focusedAxHelper('1'), 42).focused('mdsearch__input'), true);
  assert.equal(fakeAx(await focusedAxHelper('0'), 42).focused('mdsearch__input'), false);
  const malformed = await focusedAxHelper('yes');
  assert.throws(
    () => fakeAx(malformed, 42).focused('mdsearch__input'),
    /returned unparseable output/,
  );
});

test('value returns the helper value without interpreting labels or descendants', async () => {
  assert.equal(
    fakeAx(await focusedAxHelper('overview'), 42).value('mdsearch__input'),
    'overview',
  );
});
