import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import { delimiter, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { lstatSync, statSync } from 'node:fs';
import { nativeLaunchEnvironment } from './native-environment.mjs';
import { nativeCliArgs } from './native-sync-preset.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));
const require = createRequire(join(root, 'ui/package.json'));
let cli;
try {
  cli = join(dirname(require.resolve('@tauri-apps/cli/package.json')), 'tauri.js');
} catch {
  console.error('Tauri CLI is not installed. Run pnpm install --frozen-lockfile from the repository root.');
  process.exit(1);
}
const target = join(root, 'target');
try {
  if (lstatSync(target).isSymbolicLink()) {
    try {
      if (!statSync(target).isDirectory()) throw new Error('not a directory');
    } catch {
      console.error('The repository target symlink does not point to an existing directory. Restore its cache directory or remove only the broken symlink before starting Tauri.');
      process.exit(1);
    }
  }
} catch (error) {
  if (error?.code !== 'ENOENT') throw error;
}
const args = process.argv.slice(2);
const env = nativeLaunchEnvironment(root, args);
if (process.platform === 'darwin') {
  // Signing must resolve the system xattr rather than a Python environment shim.
  const pathKey = Object.keys(env).find((key) => key.toLowerCase() === 'path') ?? 'PATH';
  env[pathKey] = ['/usr/bin', '/bin', '/usr/sbin', '/sbin', env[pathKey] ?? ''].join(delimiter);
}
env.CARGO_BUILD_JOBS ??= '2';
const child = spawn(process.execPath, [cli, ...nativeCliArgs(args)], {
  cwd: root,
  env,
  stdio: 'inherit',
  shell: false,
});
child.on('error', () => {
  console.error('Could not start the installed Tauri CLI. Check the Node version and installed dependencies.');
  process.exitCode = 1;
});
for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => {
    if (child.exitCode === null && child.signalCode === null) child.kill(signal);
  });
}
child.on('exit', (code, signal) => {
  process.exitCode = code ?? (signal === 'SIGINT' ? 130 : signal === 'SIGTERM' ? 143 : 1);
});
