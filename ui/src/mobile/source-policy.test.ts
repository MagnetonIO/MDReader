import { afterEach, describe, expect, test, vi } from 'vitest';

const native = vi.hoisted(() => ({
  result: { status: 'ok', data: null } as unknown,
}));

vi.mock('../bindings', () => ({
  commands: {
    mobileEnterpriseConnection: async () => native.result,
  },
}));

import { loadEnterpriseServerConfig } from './source-policy';

afterEach(() => {
  native.result = { status: 'ok', data: null };
});

describe('mobile Enterprise native configuration', () => {
  test('loads only a validated non-secret connection selected by native state', async () => {
    native.result = {
      status: 'ok',
      data: {
        connection_id: 'managed-connection-42',
        label: 'Enterprise',
      },
    };
    await expect(loadEnterpriseServerConfig()).resolves.toEqual({
      connectionId: 'managed-connection-42',
    });

    native.result = {
      status: 'ok',
      data: {
        connection_id: 'bad\nconnection',
        label: 'Enterprise',
      },
    };
    await expect(loadEnterpriseServerConfig()).resolves.toBeNull();
  });

  test('native unavailability and refusal remain unconfigured', async () => {
    native.result = { status: 'error', error: { kind: 'credential-unavailable' } };
    await expect(loadEnterpriseServerConfig()).resolves.toBeNull();
  });
});
