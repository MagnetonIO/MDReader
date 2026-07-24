/**
 * Milestone PT — workspace seeding, shared by both GUI harnesses.
 *
 * The app grants a workspace one of two ways: the user picks a folder in the
 * native OS picker, or a persisted session is restored on launch (see
 * `AppShell`'s startup effect → `openRoot(session.root)`). Driving the native
 * NSOpenPanel / GTK file chooser from a test is brittle and, on a headless CI
 * runner, impossible — so instead we SEED the persisted session before launch:
 * write a `session.json` naming the fixture corpus as the workspace root, and the
 * app re-grants + indexes it on startup with no dialog. The backend command
 * (`watch_workspace`) canonicalises and grants the folder directly — no picker
 * involved — so a seeded root is a fully legitimate grant.
 *
 * `session.json` is a tauri-plugin-store file. The plugin resolves a bare store
 * name against the app's config dir, so this module reproduces that path per-OS
 * and writes the store's flat `{ "<key>": <value> }` shape. Node built-ins only.
 */

import { homedir } from 'node:os';
import { join } from 'node:path';
import { mkdir, rm, writeFile } from 'node:fs/promises';

/** The store key `persistence.ts` reads the session under (`store.get('session')`). */
const SESSION_KEY = 'session';
/** The session schema version `deserializeSession` accepts. */
const SESSION_VERSION = 1;

/**
 * The directory tauri-plugin-store resolves a bare store filename against — the
 * app's config dir for `identifier`.
 *
 *   - macOS:   ~/Library/Application Support/<identifier>
 *   - Linux:   $XDG_CONFIG_HOME/<identifier>  (default ~/.config/<identifier>)
 *   - Windows: %APPDATA%\<identifier>
 *
 * On macOS app_config_dir and app_data_dir coincide, so the same path serves the
 * engine's index dir too — handy but incidental.
 */
export function appConfigDir(identifier: string): string {
  const home = homedir();
  switch (process.platform) {
    case 'darwin':
      return join(home, 'Library', 'Application Support', identifier);
    case 'win32':
      return join(process.env.APPDATA ?? join(home, 'AppData', 'Roaming'), identifier);
    default:
      return join(process.env.XDG_CONFIG_HOME ?? join(home, '.config'), identifier);
  }
}

/** Absolute path of the seeded `session.json` for `identifier`. */
export function sessionStorePath(identifier: string): string {
  return join(appConfigDir(identifier), 'session.json');
}

/** The tauri-plugin-store payload that grants `rootAbs` on next launch. */
export function buildSessionSeed(rootAbs: string): Record<string, unknown> {
  return {
    [SESSION_KEY]: {
      version: SESSION_VERSION,
      root: rootAbs,
      // No open tabs: the reader shows its welcome placeholder until the flow
      // opens a document via the palette — so "a document renders" is a genuine
      // transition the harness drives, not a pre-baked tab.
      activeTabId: null,
      tabs: [],
      recent: [],
    },
  };
}

/**
 * Write the seed so the next launch grants `rootAbs`. Creates the config dir if
 * absent. Returns the path written, for logging.
 */
export async function seedWorkspace(identifier: string, rootAbs: string): Promise<string> {
  const path = sessionStorePath(identifier);
  await mkdir(appConfigDir(identifier), { recursive: true });
  await writeFile(path, JSON.stringify(buildSessionSeed(rootAbs), null, 2), 'utf8');
  return path;
}

/**
 * Remove a previously-seeded `session.json` so a run starts from a clean slate
 * (no leftover tabs/root from an earlier run). Ignores a missing file.
 */
export async function clearSeed(identifier: string): Promise<void> {
  await rm(sessionStorePath(identifier), { force: true });
}
