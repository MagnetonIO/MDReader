import { defineConfig } from 'vitest/config';
import base from './vitest.base.js';

export default defineConfig({
  test: {
    ...base.test,
    environment: 'node',
  },
});
