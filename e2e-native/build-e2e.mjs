import { execFileSync, spawnSync } from 'node:child_process';
import { readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, '..', '..', '..');
const MOBILE_SOURCE_COMMIT_FILE = resolve(
  REPO_ROOT,
  'apps',
  'mdreader',
  'src-tauri',
  '.e2e-source-commit',
);

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

export function stageMobileSourceCommit(
  configured,
  destination = MOBILE_SOURCE_COMMIT_FILE,
) {
  const sourceCommit = resolveSourceCommit(configured, () => {
    throw new Error('mobile source identity must be supplied explicitly');
  });
  const contents = `${sourceCommit}\n`;
  try {
    writeFileSync(destination, contents, { flag: 'wx', mode: 0o600 });
  } catch (error) {
    if (
      error instanceof Error &&
      'code' in error &&
      error.code === 'EEXIST'
    ) {
      throw new Error(
        `refusing to replace an existing mobile source-identity handoff: ${destination}`,
      );
    }
    throw error;
  }

  return () => {
    const current = readFileSync(destination, 'utf8');
    if (current !== contents) {
      throw new Error(
        `refusing to remove a changed mobile source-identity handoff: ${destination}`,
      );
    }
    rmSync(destination);
  };
}

export function runPnpm(args, sourceCommit) {
  const invocation = pnpmInvocation(args);
  const result = spawnSync(invocation.command, invocation.args, {
    cwd: REPO_ROOT,
    env: { ...process.env, MDREADER_SOURCE_COMMIT: sourceCommit },
    stdio: 'inherit',
  });
  if (result.error) throw result.error;
  return result.status ?? 1;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const sourceCommit = resolveSourceCommit();
  let status = runPnpm(
    ['--filter', '@mdreader/app...', 'run', 'build'],
    sourceCommit,
  );
  if (status === 0) {
    status = runPnpm(
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
  process.exitCode = status;
}
