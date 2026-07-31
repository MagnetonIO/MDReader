import { existsSync } from 'node:fs';
import { expect, test } from 'vitest';

import { CONFIG_FILE_PATHS } from './index.js';

test.each(Object.entries(CONFIG_FILE_PATHS))('%s exists on disk', (_key, path) => {
  expect(existsSync(path)).toBe(true);
});
