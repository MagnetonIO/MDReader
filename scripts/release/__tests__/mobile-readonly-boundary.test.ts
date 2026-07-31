import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

import { describe, expect, test } from 'vitest';

const repositoryRoot = resolve(process.cwd(), '..', '..');
const read = (path: string) =>
  readFileSync(resolve(repositoryRoot, path), 'utf8');

function commandRegistry(source: string, functionName: string): string {
  const functionStart = source.indexOf(`fn ${functionName}()`);
  if (functionStart === -1) throw new Error(`missing ${functionName}`);
  const registryStart = source.indexOf('collect_commands![', functionStart);
  if (registryStart === -1) {
    throw new Error(`missing ${functionName} command registry`);
  }
  const bodyStart = registryStart + 'collect_commands!['.length;
  const bodyEnd = source.indexOf('])', bodyStart);
  if (bodyEnd === -1) throw new Error(`unterminated ${functionName}`);
  return source.slice(bodyStart, bodyEnd);
}

describe('mobile read-only boundary', () => {
  test('uses a mobile command registry without content or updater mutations', () => {
    const source = read('apps/mdreader/src-tauri/src/lib.rs');
    const registry = commandRegistry(source, 'mobile_specta_builder');

    expect(registry).toContain('commands::ping');
    for (const mutation of [
      'commands::save_doc',
      'overlay::approve_fact',
      'overlay::reject_fact',
      'library::pick_and_add_library_source',
      'library::forget_library_source',
      'updater::install_update',
    ]) {
      expect(registry).not.toContain(mutation);
    }
  });

  test('uses the provider read-only Google Drive scope', () => {
    const connector = read(
      'catdb/crates/catdb-connector-google-drive/src/lib.rs',
    );
    const cloudAccounts = read(
      'apps/mdreader/src-tauri/src/cloud_accounts.rs',
    );

    expect(connector).toContain(
      'https://www.googleapis.com/auth/drive.readonly',
    );
    expect(`${connector}\n${cloudAccounts}`).not.toMatch(
      /GOOGLE_DRIVE_FILE_SCOPE|https:\/\/www\.googleapis\.com\/auth\/drive\.file/,
    );
  });
});
