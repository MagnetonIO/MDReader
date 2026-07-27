import { load } from '@tauri-apps/plugin-store';

export type MobileSourceKind =
  | 'device-folder'
  | 'google-drive'
  | 'dropbox'
  | 'enterprise-catdb';

export interface MobileSourcePolicy {
  kind: MobileSourceKind;
  label: string;
  connection: 'native-grant-unavailable' | 'secure-vault-unavailable' | 'server-session';
}

/** The complete, read-only mobile V1 source vocabulary. */
export const MOBILE_SOURCE_ALLOWLIST: readonly MobileSourcePolicy[] = [
  {
    kind: 'device-folder',
    label: 'Device folders',
    connection: 'native-grant-unavailable',
  },
  {
    kind: 'google-drive',
    label: 'Google Drive',
    connection: 'secure-vault-unavailable',
  },
  { kind: 'dropbox', label: 'Dropbox', connection: 'secure-vault-unavailable' },
  {
    kind: 'enterprise-catdb',
    label: 'Enterprise CatDB server',
    connection: 'server-session',
  },
];

export interface EnterpriseServerConfig {
  serverUrl: string;
  /** Opaque reference to a server-managed session; never a token or password. */
  sessionReference: string;
}

export type EnterpriseConfigResult =
  | { status: 'ok'; config: EnterpriseServerConfig }
  | { status: 'error'; message: string };

function isExactLoopback(hostname: string): boolean {
  return hostname === 'localhost' || hostname === '127.0.0.1' || hostname === '[::1]';
}

/** Validate the non-secret mobile server boundary and return a canonical origin. */
export function validateEnterpriseServerConfig(
  serverUrl: string,
  sessionReference: string,
): EnterpriseConfigResult {
  let parsed: URL;
  try {
    parsed = new URL(serverUrl);
  } catch {
    return { status: 'error', message: 'Enter a valid CatDB server URL.' };
  }
  if (parsed.username || parsed.password) {
    return { status: 'error', message: 'The server URL must not include credentials.' };
  }
  if (parsed.protocol !== 'https:' && !(parsed.protocol === 'http:' && isExactLoopback(parsed.hostname))) {
    return {
      status: 'error',
      message: 'Use HTTPS unless the CatDB server is on this device loopback.',
    };
  }
  if (parsed.search || parsed.hash || (parsed.pathname !== '' && parsed.pathname !== '/')) {
    return {
      status: 'error',
      message: 'Use the CatDB server origin without a path, query, or fragment.',
    };
  }
  const reference = sessionReference.trim();
  if (
    reference.length === 0 ||
    reference.length > 200 ||
    [...reference].some((character) => character <= '\u001f' || character === '\u007f')
  ) {
    return {
      status: 'error',
      message: 'Choose a valid server-managed session reference.',
    };
  }
  return {
    status: 'ok',
    config: { serverUrl: parsed.origin, sessionReference: reference },
  };
}

const SERVER_CONFIG_KEY = 'enterprise-server';

/** Persist only the validated URL and opaque session reference, never credentials. */
export async function saveEnterpriseServerConfig(config: EnterpriseServerConfig): Promise<void> {
  const store = await load('mobile-sources.json', { autoSave: false });
  await store.set(SERVER_CONFIG_KEY, config);
  await store.save();
}

/**
 * Load and revalidate the saved server boundary. A hand-edited or stale store
 * entry is never promoted into an authenticated adapter.
 */
export async function loadEnterpriseServerConfig(): Promise<EnterpriseServerConfig | null> {
  try {
    const store = await load('mobile-sources.json', { autoSave: false });
    const raw = await store.get(SERVER_CONFIG_KEY);
    if (!raw || typeof raw !== 'object') return null;
    const candidate = raw as Record<string, unknown>;
    if (
      typeof candidate.serverUrl !== 'string' ||
      typeof candidate.sessionReference !== 'string'
    ) {
      return null;
    }
    const validated = validateEnterpriseServerConfig(
      candidate.serverUrl,
      candidate.sessionReference,
    );
    return validated.status === 'ok' ? validated.config : null;
  } catch {
    return null;
  }
}
