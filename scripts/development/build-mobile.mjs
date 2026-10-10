import { execFileSync, spawn } from 'node:child_process';
import { realpathSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { stageHandoff, validCommit } from './release-handoff.mjs';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const pause = (milliseconds) => new Promise((done) => setTimeout(done, milliseconds));

function requireMobileRuntimeResolver(environment) {
  // ort-sys rc.12 uses the first defined flag in this order. Mobile nested
  // builders can filter environments, so reject every explicit disabling flag
  // instead of relying on an earlier false flag to mask a later true flag.
  for (const name of ['CARGO_NET_OFFLINE', 'ORT_SKIP_DOWNLOAD', 'ORT_OFFLINE']) {
    const value = environment[name];
    // Direct API callers must use process.env's string/undefined shape. Spawn
    // stringifies other values, which could otherwise bypass this preflight.
    if (value !== undefined && typeof value !== 'string') {
      throw Error(`${name} must be a string or unset for a Tauri mobile build`);
    }
    if (value === '1' || value?.toLowerCase() === 'true') {
      throw Error(`${name} disables the pinned ONNX runtime download/cache resolver; unset it or set it to false/0 before a Tauri mobile build. A cached runtime still requires the resolver; Tauri does not forward ORT_LIB_PATH as a workaround.`);
    }
  }
}

function git(root, args) {
  return execFileSync('git', args, { cwd: root, encoding: 'utf8', maxBuffer: 1024 * 1024, timeout: 10_000 });
}
export function cleanHead(root) {
  if (realpathSync(git(root, ['rev-parse', '--show-toplevel']).trim()) !== realpathSync(root)) {
    throw Error('mobile wrapper must reside in the standalone Git checkout root');
  }
  const head = git(root, ['rev-parse', '--verify', 'HEAD^{commit}']).trim();
  if (!validCommit(head)) throw Error('current Git HEAD is not a lowercase nonzero SHA');
  if (git(root, ['status', '--porcelain=v1', '-z', '--untracked-files=all', '--ignore-submodules=none']).length) {
    throw Error('mobile source build requires a clean Git checkout (tracked and untracked files)');
  }
  return head;
}
function groupAlive(pid) {
  try { process.kill(-pid, 0); return true; }
  catch (error) { if (error.code === 'ESRCH') return false; throw error; }
}
function signalGroup(pid, signal) {
  try { process.kill(-pid, signal); }
  catch (error) { if (error.code !== 'ESRCH') throw error; }
}
async function quiesce(pid) {
  for (let attempt = 0; attempt < 50; attempt++) {
    if (!groupAlive(pid)) return { stopped: true, forced: false };
    await pause(100);
  }
  signalGroup(pid, 'SIGKILL');
  for (let attempt = 0; attempt < 50; attempt++) {
    if (!groupAlive(pid)) return { stopped: true, forced: true };
    await pause(100);
  }
  return { stopped: false, forced: true };
}

/** Runs only pnpm tauri <ios|android> build, with literal argv and no shell. */
export async function runMobileBuild(platform, args = [], root = ROOT, environment = process.env) {
  if (!['darwin', 'linux'].includes(process.platform)) throw Error('mobile source wrapper currently requires macOS or Linux');
  if (!['ios', 'android'].includes(platform)) throw Error('usage: node scripts/development/build-mobile.mjs <ios|android> [Tauri build arguments]');
  if (args.some((arg) => typeof arg !== 'string' || arg.includes('\0'))) throw Error('invalid mobile build argument');
  requireMobileRuntimeResolver(environment);
  const commit = cleanHead(root);
  if (environment.MDREADER_RELEASE_SOURCE_COMMIT !== undefined && environment.MDREADER_RELEASE_SOURCE_COMMIT !== commit) {
    throw Error('inherited release source identity differs from current clean Git HEAD');
  }
  const cleanup = stageHandoff(resolve(root, 'src-tauri/.release-source-commit'), commit);
  let child;
  let stopped = true;
  let interrupted;
  let forceStop;
  const forward = (signal) => {
    interrupted ??= signal;
    if (child?.pid) {
      signalGroup(child.pid, signal);
      forceStop ??= setTimeout(() => signalGroup(child.pid, 'SIGKILL'), 5000);
    }
  };
  const onInterrupt = () => forward('SIGINT');
  const onTerminate = () => forward('SIGTERM');
  process.on('SIGINT', onInterrupt);
  process.on('SIGTERM', onTerminate);
  let status = 1;
  let failure;
  try {
    // Close the source-change window before the child starts; this is not a source lock.
    if (cleanHead(root) !== commit) throw Error('Git HEAD changed before mobile build');
    child = spawn('pnpm', ['tauri', platform, 'build', ...args], {
      cwd: root,
      env: { ...environment, CARGO_BUILD_JOBS: '2', MDREADER_RELEASE_SOURCE_COMMIT: commit },
      stdio: 'inherit', shell: false, detached: true,
    });
    stopped = false;
    const outcome = await new Promise((done) => {
      child.once('error', (error) => done({ error }));
      child.once('close', (code, signal) => done({ code, signal }));
    });
    const quiescence = child.pid ? await quiesce(child.pid) : { stopped: true, forced: false };
    stopped = quiescence.stopped;
    if (!stopped) throw Error('nested mobile build processes remain; handoff retained for manual recovery');
    if (quiescence.forced && !interrupted) throw Error('nested builders outlived pnpm and were terminated; mobile build is not verified');
    if (outcome.error) throw Error('cannot start pnpm mobile build');
    status = outcome.code ?? ((outcome.signal === 'SIGINT' || interrupted === 'SIGINT') ? 130 : 143);
    if (interrupted) status = interrupted === 'SIGINT' ? 130 : 143;
    if (status === 0 && cleanHead(root) !== commit) throw Error('Git HEAD changed during mobile build');
  } catch (error) {
    failure = error;
  } finally {
    clearTimeout(forceStop);
    process.removeListener('SIGINT', onInterrupt);
    process.removeListener('SIGTERM', onTerminate);
    if (stopped) {
      try { cleanup(); }
      catch (error) {
        if (failure || status !== 0) console.error(error.message);
        else failure = error;
      }
    }
  }
  if (failure) throw failure;
  return status;
}

if (process.argv[1] && realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url))) {
  try { process.exitCode = await runMobileBuild(process.argv[2], process.argv.slice(3)); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
