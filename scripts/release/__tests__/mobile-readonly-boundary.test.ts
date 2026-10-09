import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, test } from 'vitest';

const repositoryRoot = fileURLToPath(new URL('../../../', import.meta.url));
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
    const source = read('src-tauri/src/lib.rs');
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

  test('uses only the selected-file Google Drive scope for fresh native grants', () => {
    const connector = read(
      'crates/mdreader-connector-google-drive/src/native.rs',
    );
    const scopes = read(
      'crates/mdreader-credentials/src/grant/scopes.rs',
    );
    const oauth = read('src-tauri/src/provider_oauth.rs');
    expect(connector).toContain('https://www.googleapis.com/auth/drive.file');
    expect(scopes).toContain('https://www.googleapis.com/auth/drive.file');
    expect(oauth).toContain('mdreader_credentials::DRIVE_FILE_SCOPE');
    expect(`${connector}\n${scopes}\n${oauth}`).not.toContain(
      'https://www.googleapis.com/auth/drive.readonly',
    );
  });
});
