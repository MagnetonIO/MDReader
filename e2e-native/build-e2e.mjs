import { execFileSync, spawnSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, '..', '..', '..');

export function resolveSourceCommit(
  configured = process.env.MDREADER_SOURCE_COMMIT,
  readHead = () =>
    execFileSync('git', ['rev-parse', 'HEAD'], {
      cwd: REPO_ROOT,
      encoding: 'utf8',
    }).trim(),
) {
  const commit = configured ?? readHead();
  if (!/^[0-9a-f]{40}$/.test(commit)) {
    throw new Error(
      'MDREADER_SOURCE_COMMIT must be a lowercase 40-character Git commit',
    );
  }
  return commit;
}

export function pnpmInvocation(
  args,
  platform = process.platform,
  commandProcessor = process.env.ComSpec ?? 'cmd.exe',
) {
  if (platform !== 'win32') return { command: 'pnpm', args };
  if (!args.every((argument) => /^[A-Za-z0-9@./:_-]+$/.test(argument))) {
    throw new Error('refusing to pass an unsafe pnpm argument to cmd.exe');
  }
  return {
    command: commandProcessor,
    args: ['/d', '/s', '/c', ['pnpm', ...args].join(' ')],
  };
}

function runPnpm(args, sourceCommit) {
  const invocation = pnpmInvocation(args);
  const result = spawnSync(invocation.command, invocation.args, {
    cwd: REPO_ROOT,
    env: { ...process.env, MDREADER_SOURCE_COMMIT: sourceCommit },
    stdio: 'inherit',
  });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const sourceCommit = resolveSourceCommit();
  runPnpm(['--filter', '@mdreader/app...', 'run', 'build'], sourceCommit);
  runPnpm(
    [
      '--filter',
      '@mdreader/app',
      'tauri',
      'build',
      '--bundles',
      'app',
      '--features',
      'e2e-build',
      '--config',
      'src-tauri/tauri.e2e.conf.json',
    ],
    sourceCommit,
  );
}
