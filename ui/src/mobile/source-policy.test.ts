import { afterEach, describe, expect, test, vi } from 'vitest';

const persisted = vi.hoisted(() => ({
  value: null as unknown,
}));

vi.mock('@tauri-apps/plugin-store', () => ({
  load: async () => ({
    get: async () => persisted.value,
    set: async (_key: string, value: unknown) => {
      persisted.value = value;
    },
    save: async () => undefined,
  }),
}));

import {
  loadEnterpriseServerConfig,
  saveEnterpriseServerConfig,
} from './source-policy';

afterEach(() => {
  persisted.value = null;
});

describe('mobile Enterprise configuration persistence', () => {
  test('loads only a validated non-secret server boundary', async () => {
    persisted.value = {
      serverUrl: 'https://catdb.example.com',
      sessionReference: 'managed-session-42',
    };
    await expect(loadEnterpriseServerConfig()).resolves.toEqual(persisted.value);

    persisted.value = {
      serverUrl: 'https://admin:secret@catdb.example.com',
      sessionReference: 'managed-session-42',
    };
    await expect(loadEnterpriseServerConfig()).resolves.toBeNull();
  });

  test('a saved configuration is consumable on the next launch', async () => {
    const config = {
      serverUrl: 'https://catdb.example.com',
      sessionReference: 'managed-session-42',
    };
    await saveEnterpriseServerConfig(config);
    await expect(loadEnterpriseServerConfig()).resolves.toEqual(config);
  });
});
