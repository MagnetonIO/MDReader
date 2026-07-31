import { defineConfig } from 'vitest/config';
import base from '@mdreader/config/vitest';

export default defineConfig({
  test: {
    ...base.test,
    environment: 'jsdom',
  },
});
