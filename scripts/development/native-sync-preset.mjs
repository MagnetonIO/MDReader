import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const flag = '--sync-development';
const fields = ['STAGE', 'ISSUER', 'CLIENT_ID', 'AUDIENCE', 'AUTHORIZATION_ENDPOINT',
  'TOKEN_ENDPOINT', 'JWKS_ENDPOINT', 'ENDPOINT', 'REDIRECT_URI']
  .map((name) => `MDREADER_NATIVE_SYNC_${name}`);

// This launcher option selects public registration only. It never signs in,
// reads a credential, creates a library or queues a document for upload.
export function nativeCliArgs(args) {
  const count = args.filter((arg) => arg === flag).length;
  if (count > 1 || count === 1 && !['dev', 'build'].includes(args[0])) {
    throw new Error('--sync-development is supported once for desktop dev/build only.');
  }
  return args.filter((arg) => arg !== flag);
}

export function nativeSyncPreset(root, args) {
  nativeCliArgs(args);
  if (!args.includes(flag)) return {};
  const value = JSON.parse(readFileSync(join(root, 'scripts/development/native-sync-development.json'), 'utf8'));
  const expected = ['schemaVersion', ...fields].sort();
  if (value === null || typeof value !== 'object' || Array.isArray(value)
      || value.schemaVersion !== 1
      || JSON.stringify(Object.keys(value).sort()) !== JSON.stringify(expected)
      || fields.some((name) => typeof value[name] !== 'string' || !value[name] || value[name].length > 2048)
      || value.MDREADER_NATIVE_SYNC_STAGE !== 'development') {
    throw new Error('The public native development sync registration is incomplete or invalid.');
  }
  return Object.fromEntries(fields.map((name) => [name, value[name]]));
}
