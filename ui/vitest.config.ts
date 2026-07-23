import base from '@mdreader/config/vitest';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    ...base.test,
    environment: 'jsdom',
  },
});
