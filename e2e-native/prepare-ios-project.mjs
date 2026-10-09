import { execFileSync } from 'node:child_process';
import { lstatSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { REPO_ROOT } from './repository.mjs';
const { parse, stringify } = createRequire(resolve(REPO_ROOT, 'package.json'))('yaml');

/** Tauri keeps an existing project.yml. Change identity only in a linked,
 * disposable worktree; preserve the tracked mobile plugins and URL handlers. */
export function prepareIosProject(root = REPO_ROOT) {
  if (!lstatSync(resolve(root, '.git')).isFile()) {
    throw Error('iOS E2E preparation requires a disposable linked Git worktree');
  }
  const top = execFileSync('git', ['rev-parse', '--show-toplevel'], { cwd: root, encoding: 'utf8' }).trim();
  if (realpathSync(top) !== realpathSync(root)) throw Error('Unexpected E2E checkout root');
  const config = JSON.parse(readFileSync(resolve(root, 'src-tauri/tauri.e2e.conf.json'), 'utf8'));
  if (config.identifier !== 'dev.mdreader.app.e2e' || config.productName !== 'MDReader E2E') {
    throw Error('Unexpected E2E app identity');
  }
  const path = resolve(root, 'src-tauri/gen/apple/project.yml');
  const stamp = lstatSync(path);
  if (!stamp.isFile() || stamp.nlink !== 1) throw Error('E2E project must be a regular owned file');
  const project = parse(readFileSync(path, 'utf8'));
  const app = project?.settingGroups?.app?.base;
  if (!app || !['dev.mdreader.app', config.identifier].includes(app.PRODUCT_BUNDLE_IDENTIFIER)
    || !['MDReader', config.productName].includes(app.PRODUCT_NAME)
    || Object.values(project.targets ?? {}).some(target => target.settings?.base?.PRODUCT_BUNDLE_IDENTIFIER
      || target.settings?.base?.PRODUCT_NAME)) {
    throw Error('Unrecognized Apple project identity layout');
  }
  project.options.bundleIdPrefix = config.identifier;
  app.PRODUCT_BUNDLE_IDENTIFIER = config.identifier;
  app.PRODUCT_NAME = config.productName;
  writeFileSync(path, stringify(project));
  return config.identifier;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { console.log(`Prepared disposable iOS identity: ${prepareIosProject()}`); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
