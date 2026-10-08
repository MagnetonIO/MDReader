import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, isAbsolute, join, resolve } from 'node:path';
import { parseEnv } from 'node:util';

// Only build configuration crosses this boundary. Never load old tokens or
// client secrets into a frontend/Vite environment or execute a .env as shell.
const publicNames = new Set([
  'MDREADER_MOBILE_SYNC_REDIRECT_URI',
  'MDREADER_GOOGLE_DRIVE_CLIENT_ID',
  'MDREADER_DROPBOX_CLIENT_ID',
  'MDREADER_DROPBOX_REDIRECT_URI',
  'MDREADER_NATIVE_DRIVE_CLIENT_ID',
  'MDREADER_NATIVE_DROPBOX_CLIENT_ID',
  'MDREADER_NATIVE_DROPBOX_ACCESS_TYPE',
  'MDREADER_NATIVE_DROPBOX_APP_FOLDER_REGISTRATION_SHA256',
  'MDREADER_NATIVE_DROPBOX_REDIRECT_URI',
  ...['STAGE', 'ISSUER', 'CLIENT_ID', 'AUDIENCE', 'AUTHORIZATION_ENDPOINT',
    'TOKEN_ENDPOINT', 'JWKS_ENDPOINT', 'ENDPOINT', 'REDIRECT_URI']
    .map((name) => `MDREADER_NATIVE_SYNC_${name}`),
]);

export function nativeEnvironment(root, inherited = process.env) {
  const env = {};
  const roots = [];
  try {
    const common = execFileSync('git', ['rev-parse', '--git-common-dir'], {
      cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'],
    }).trim();
    const directory = dirname(isAbsolute(common) ? common : resolve(root, common));
    if (directory !== root) roots.push(directory);
  } catch { /* Source archives have no shared worktree configuration. */ }
  roots.push(root);
  for (const directory of roots) {
    for (const filename of ['.env', '.env.local']) {
      let values;
      try { values = parseEnv(readFileSync(join(directory, filename), 'utf8')); }
      catch (error) {
        if (error?.code === 'ENOENT') continue;
        throw new Error(`Cannot load local native configuration (${filename}).`);
      }
      for (const [name, value] of Object.entries(values)) {
        if (publicNames.has(name)) env[name] = value;
      }
    }
  }
  // Explicit shell/CI settings win; absent shell values retain local settings.
  return { ...env, ...inherited };
}
