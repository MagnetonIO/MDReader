import { defineConfig } from 'vitest/config';

// Self-contained Vitest config. This platform package deliberately does not
// extend @mdreader/config (packages/**) to keep the dependency direction
// one-way (packages depend on @catdb/client, never the reverse).
export default defineConfig({
  test: {
    include: ['src/**/*.{test,spec}.ts'],
    environment: 'node',
    reporters: ['default'],
    clearMocks: true,
  },
});
