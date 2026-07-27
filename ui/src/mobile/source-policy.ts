import { commands } from '../bindings';

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
  /** Non-secret identifier for an origin + bearer held entirely by native state. */
  connectionId: string;
}

export type EnterpriseConfigResult =
  | { status: 'ok'; config: EnterpriseServerConfig }
  | { status: 'error'; message: string };

/** Validate the non-secret handle for a connection already admitted natively. */
export function validateEnterpriseServerConfig(
  connectionId: string,
): EnterpriseConfigResult {
  const id = connectionId.trim();
  if (
    id.length === 0 ||
    id.length > 200 ||
    [...id].some((character) => character <= '\u001f' || character === '\u007f')
  ) {
    return {
      status: 'error',
      message: 'Choose a valid native-managed connection.',
    };
  }
  return { status: 'ok', config: { connectionId: id } };
}

/**
 * Ask native state for the active connection. Ordinary mobile builds return
 * no connection until a platform secure-vault enrollment exists; the renderer
 * cannot invent or persist one.
 */
export async function loadEnterpriseServerConfig(): Promise<EnterpriseServerConfig | null> {
  try {
    const result = await commands.mobileEnterpriseConnection();
    if (result.status === 'error' || result.data === null) return null;
    const validated = validateEnterpriseServerConfig(result.data.connection_id);
    return validated.status === 'ok' ? validated.config : null;
  } catch {
    return null;
  }
}
