import { defineConfig } from 'vitest/config';
import base from '@mdreader/config/vitest';

// Runs ONLY the committed parity-reference generator (src/gen-parity-reference.ts)
// under default Node resolution. Invoked via the `gen:parity-reference` script;
// not part of the normal test suite.
export default defineConfig({
  test: {
    ...base.test,
    include: ['src/gen-parity-reference.ts'],
    environment: 'node',
  },
});
