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

import { homedir, tmpdir } from 'node:os';
import { basename, join, relative, resolve } from 'node:path';
import { existsSync, lstatSync } from 'node:fs';
import { mkdir, rm, writeFile } from 'node:fs/promises';

import { E2E_BUNDLE_IDENTIFIER } from './flows.ts';

/** The store key `persistence.ts` reads the session under (`store.get('session')`). */
const SESSION_KEY = 'session';
/** The session schema version `deserializeSession` accepts. */
const SESSION_VERSION = 1;

/**
 * Every tauri-plugin-store file the app persists (`shell/persistence.ts`), and
 * therefore everything {@link clearSeed} has to remove for a run to start from
 * the product's real defaults.
 *
 * `settings.json` belongs on this list as much as `session.json` does, and
 * leaving it off cost a lane a day: it carries `lensSchema`, so a workspace
 * left with an interpretation in force hands the NEXT run an app that is
 * already reading through that lens. The View As step then has nothing to
 * adopt — the sheet opens on "Reading as this" — and the run fails claiming the
 * lens never took effect, which is the opposite of what happened. It also
 * carries `inspectorOpen` and `reviewer`, which the Inspector step toggles and
 * signs.
 */
const STORE_FILES = ['session.json', 'settings.json'] as const;

/**
 * The directory tauri-plugin-store resolves a bare store filename against — the
 * app's config dir for the E2E bundle identifier.
 *
 *   - macOS:   ~/Library/Application Support/<identifier>
 *   - Linux:   $XDG_CONFIG_HOME/<identifier>  (default ~/.config/<identifier>)
 *   - Windows: %APPDATA%\<identifier>
 *
 * On macOS app_config_dir and app_data_dir coincide, so the same path serves the
 * engine's index dir too — handy but incidental.
 */
function platformConfigDir(identifier: string): string {
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

/** A profile that has passed the native harness's fail-closed cleanup checks. */
export interface E2EProfile {
  readonly identifier: typeof E2E_BUNDLE_IDENTIFIER;
  readonly configDir: string;
}

function isStrictDescendant(path: string, parent: string): boolean {
  const relation = relative(parent, path);
  return relation !== '' && !relation.startsWith(`..${process.platform === 'win32' ? '\\' : '/'}`) && relation !== '..' && !relation.startsWith('/');
}

/**
 * Construct the only profile destructive native helpers accept. Tests may pass a
 * `mkdtemp(join(tmpdir(), 'mdreader-e2e-'))` directory; production runs use the
 * E2E bundle's real per-user config directory.
 */
export function e2eProfile(configDir = platformConfigDir(E2E_BUNDLE_IDENTIFIER)): E2EProfile {
  const profile: E2EProfile = { identifier: E2E_BUNDLE_IDENTIFIER, configDir: resolve(configDir) };
  assertE2EProfile(profile);
  return profile;
}

/** Refuse every path except the E2E product's state or an explicitly test-owned directory. */
export function assertE2EProfile(profile: E2EProfile): asserts profile is E2EProfile {
  const candidate = profile as { identifier?: unknown; configDir?: unknown };
  if (candidate.identifier !== E2E_BUNDLE_IDENTIFIER || typeof candidate.configDir !== 'string') {
    throw new Error('refusing destructive seed operation without a validated E2E profile');
  }

  const configDir = resolve(candidate.configDir);
  const defaultDir = resolve(platformConfigDir(E2E_BUNDLE_IDENTIFIER));
  const testRoot = resolve(tmpdir());
  const testOwned = isStrictDescendant(configDir, testRoot) && basename(configDir).startsWith('mdreader-e2e-');
  if (
    (configDir !== defaultDir && !testOwned) ||
    (existsSync(configDir) && lstatSync(configDir).isSymbolicLink())
  ) {
    throw new Error('refusing destructive seed operation without a validated E2E profile');
  }
}

/** Absolute path of the seeded `session.json` for a validated E2E profile. */
export function appConfigDir(profile: E2EProfile): string {
  assertE2EProfile(profile);
  return resolve(profile.configDir);
}

/** Absolute path of the seeded `session.json` for a validated E2E profile. */
export function sessionStorePath(profile: E2EProfile): string {
  return join(appConfigDir(profile), 'session.json');
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
export async function seedWorkspace(profile: E2EProfile, rootAbs: string): Promise<string> {
  assertE2EProfile(profile);
  const path = sessionStorePath(profile);
  await mkdir(appConfigDir(profile), { recursive: true });
  await writeFile(path, JSON.stringify(buildSessionSeed(rootAbs), null, 2), 'utf8');
  return path;
}

/**
 * Remove every persisted store file so a run starts from a clean slate — no
 * leftover tabs/root, and no leftover UI state (see {@link STORE_FILES}).
 * Ignores files that are not there.
 */
export async function clearSeed(profile: E2EProfile): Promise<void> {
  assertE2EProfile(profile);
  const dir = appConfigDir(profile);
  for (const file of STORE_FILES) await rm(join(dir, file), { force: true });
  await rm(join(dir, 'workspaces'), { recursive: true, force: true });
}
