/** The harness lives directly under this standalone repository. */
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const E2E_ROOT = dirname(fileURLToPath(import.meta.url));
export const REPO_ROOT = resolve(E2E_ROOT, '..');
export const MOBILE_SOURCE_COMMIT_FILE = join(REPO_ROOT, 'src-tauri', '.e2e-source-commit');
