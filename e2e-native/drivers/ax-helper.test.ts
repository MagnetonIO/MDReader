import assert from 'node:assert/strict';
import { chmod, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { makeAx } from './ax-helper.ts';

async function fakeAxHelper(errorLine: string): Promise<{ bin: string; attempts: string }> {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-ax-helper-'));
  const bin = join(root, 'axquery.mjs');
  const attempts = join(root, 'attempts');
  await writeFile(
    bin,
    [
      '#!/usr/bin/env node',
      "import { existsSync, readFileSync, writeFileSync } from 'node:fs';",
      `const attempts = ${JSON.stringify(attempts)};`,
      "const count = existsSync(attempts) ? Number(readFileSync(attempts, 'utf8')) + 1 : 1;",
      "writeFileSync(attempts, String(count));",
      `if (count === 1) { console.error(${JSON.stringify(errorLine)}); process.exit(3); }`,
      "console.log('Sample Project');",
    ].join('\n'),
  );
  await chmod(bin, 0o755);
  return { bin, attempts };
}

test('a transient AX cannot-complete read is retried and never converted to empty', async () => {
  const helper = await fakeAxHelper('AX_READ_ERROR: AXRole code=-25204');
  assert.equal(makeAx(helper.bin, 42).headings(), 'Sample Project');
  assert.equal(await readFile(helper.attempts, 'utf8'), '2');
});

test('a WebKit node invalidated during traversal is retried from the current root', async () => {
  const helper = await fakeAxHelper('AX_READ_ERROR: AXDOMClassList code=-25202');
  assert.equal(makeAx(helper.bin, 42).headings(), 'Sample Project');
  assert.equal(await readFile(helper.attempts, 'utf8'), '2');
});

test('another AX read error remains fail-closed without retry', async () => {
  const helper = await fakeAxHelper('AX_READ_ERROR: AXRole code=-25205');
  assert.throws(() => makeAx(helper.bin, 42).headings());
  assert.equal(await readFile(helper.attempts, 'utf8'), '1');
});
