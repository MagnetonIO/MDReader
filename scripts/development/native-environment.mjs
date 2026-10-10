import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, isAbsolute, join, resolve } from 'node:path';
import { parseEnv } from 'node:util';
import { nativeSyncPreset } from './native-sync-preset.mjs';

// Only allowlisted native build configuration crosses this boundary. Google
// desktop token parameters have no VITE_ prefix and never enter renderer DTOs.
// Never load user tokens or execute a .env as shell.
const nativeBuildNames = new Set([
  'MDREADER_MOBILE_SYNC_REDIRECT_URI',
  'MDREADER_MOBILE_DRIVE_CLIENT_ID',
  'MDREADER_MOBILE_DRIVE_REDIRECT_URI',
  'MDREADER_MOBILE_DROPBOX_CLIENT_ID',
  'MDREADER_MOBILE_DROPBOX_REDIRECT_URI',
  'MDREADER_GOOGLE_DRIVE_CLIENT_ID',
  'MDREADER_GOOGLE_DRIVE_CLIENT_SECRET',
  'MDREADER_DROPBOX_CLIENT_ID',
  'MDREADER_DROPBOX_REDIRECT_URI',
  'MDREADER_NATIVE_DRIVE_CLIENT_ID',
  'MDREADER_NATIVE_DRIVE_CLIENT_SECRET',
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
        if (nativeBuildNames.has(name)) env[name] = value;
      }
    }
  }
  // Explicit shell/CI settings win; absent shell values retain local settings.
  return { ...env, ...inherited };
}

// The launcher's checkout is authoritative, including inside Xcode. Reused
// dependency trees and inherited IDE variables must not select another tree.
function pinnedTools(root) {
  const channel = readFileSync(join(root, 'rust-toolchain.toml'), 'utf8')
    .match(/^\s*channel\s*=\s*"([^"]+)"/m)?.[1];
  if (!channel) throw new Error('Native builds require the repository Rust toolchain pin.');
  const tool = (name) => execFileSync('rustup', ['which', '--toolchain', channel, name], {
    cwd: root, encoding: 'utf8', timeout: 10_000, stdio: ['ignore', 'pipe', 'ignore'],
  }).trim();
  return { rustc: tool('rustc'), rustdoc: tool('rustdoc') };
}
export function nativeLaunchEnvironment(root, args, inherited = process.env, tools = pinnedTools) {
  // Explicit local/shell configuration stays authoritative, including disabled
  // or invalid overrides. A preset is never used to repair a bad override.
  const env = { ...nativeSyncPreset(root, args), ...nativeEnvironment(root, inherited) };
  env.TAURI_APP_PATH = join(root, 'src-tauri');
  env.TAURI_FRONTEND_PATH = root;
  // Nested mobile builds may run outside the checkout. Tauri forwards CARGO_*
  // but drops RUSTUP_TOOLCHAIN, so pass the actual pinned compiler explicitly.
  if (['ios', 'android'].includes(args[0])) {
    const pinned = tools(root);
    env.CARGO_BUILD_RUSTC = pinned.rustc;
    env.CARGO_BUILD_RUSTDOC = pinned.rustdoc;
  }
  // Let Xcode select the SDK for the requested mobile target. Its nested
  // xcode-script already receives the correct SDK and must retain it.
  if (args[0] === 'ios' && args[1] !== 'xcode-script') delete env.SDKROOT;
  return env;
}
