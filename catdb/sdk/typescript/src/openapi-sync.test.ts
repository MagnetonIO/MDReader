import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { expect, it } from 'vitest';

// Resolve paths from this test file so it is independent of the cwd.
const srcDir = dirname(fileURLToPath(import.meta.url)); // .../catdb/sdk/typescript/src
const pkgRoot = join(srcDir, '..'); // .../catdb/sdk/typescript
const spec = join(pkgRoot, '..', '..', 'proto', 'openapi.json'); // catdb/proto/openapi.json
const committed = join(srcDir, 'openapi.gen.ts');
const bin = join(pkgRoot, 'node_modules', '.bin', 'openapi-typescript');

/**
 * Drift gate: the committed `src/openapi.gen.ts` must be byte-identical to what
 * the pinned `openapi-typescript` regenerates from `catdb/proto/openapi.json`.
 * This is the CI check that fails when the SDK's generated types fall out of
 * step with the committed contract — regenerate with `pnpm run generate`.
 */
it('committed openapi.gen.ts matches a fresh generation from proto/openapi.json', () => {
  expect(existsSync(bin), `openapi-typescript binary missing at ${bin}`).toBe(true);

  const tmp = mkdtempSync(join(tmpdir(), 'catdb-openapi-'));
  const out = join(tmp, 'openapi.gen.ts');
  try {
    execFileSync(bin, [spec, '-o', out], { stdio: 'pipe' });
    const regenerated = readFileSync(out, 'utf8');
    const current = readFileSync(committed, 'utf8');
    expect(
      regenerated,
      'src/openapi.gen.ts is stale — regenerate with: pnpm --filter @catdb/client run generate',
    ).toBe(current);
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
});
