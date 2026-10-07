import base from '@mdreader/config/vitest';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  resolve: {
    alias: {
      '@catdb/client': new URL(
        '../catdb/sdk/typescript/src/index.ts',
        import.meta.url,
      ).pathname,
      '@tauri-apps/plugin-os': new URL('./src/test/tauri-os.ts', import.meta.url).pathname,
    },
  },
  test: {
    ...base.test,
    environment: 'jsdom',
  },
});
