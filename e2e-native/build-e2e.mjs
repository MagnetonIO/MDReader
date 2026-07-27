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

function runPnpm(args, sourceCommit) {
  const result = spawnSync(process.platform === 'win32' ? 'pnpm.cmd' : 'pnpm', args, {
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
