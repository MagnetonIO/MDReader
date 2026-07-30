import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join, resolve } from 'node:path';

import { E2E_BUNDLE_IDENTIFIER } from './flows.ts';

/** Refuse a macOS bundle unless its packaged identity is the native E2E identity. */
export function assertE2EBundle(appPath: string): void {
  const infoPlist = join(resolve(appPath), 'Contents', 'Info.plist');
  if (!existsSync(infoPlist)) {
    throw new Error(`refusing app without an E2E bundle identifier: missing ${infoPlist}`);
  }

  let identifier: string;
  try {
    identifier = execFileSync('/usr/bin/plutil', ['-extract', 'CFBundleIdentifier', 'raw', infoPlist], {
      encoding: 'utf8',
    }).trim();
  } catch {
    throw new Error(`refusing app without an E2E bundle identifier: unreadable ${infoPlist}`);
  }

  if (identifier !== E2E_BUNDLE_IDENTIFIER) {
    throw new Error(`refusing app with non-E2E bundle identifier ${JSON.stringify(identifier)}`);
  }
}
