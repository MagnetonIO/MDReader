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

import { createHash, randomUUID } from 'node:crypto';
import { homedir, tmpdir } from 'node:os';
import { basename, dirname, isAbsolute, join, relative, resolve } from 'node:path';
import {
  lstat,
  mkdir,
  readFile,
  readdir,
  readlink,
  realpath,
  rename,
  rm,
  writeFile,
} from 'node:fs/promises';

import { E2E_BUNDLE_IDENTIFIER } from './flows.ts';

export { E2E_BUNDLE_IDENTIFIER } from './flows.ts';

export interface SourceFingerprintEntry {
  readonly kind: 'file' | 'symlink' | 'directory';
  readonly size: string;
  readonly sha256: string;
  readonly mtimeNs: string;
}

export type SourceFingerprint = Record<string, SourceFingerprintEntry>;

/**
 * Fingerprint every source-tree file by membership, bytes, length, and mtime.
 *
 * The native P8 leg takes one snapshot before launch and one after shutdown.
 * A byte-identical rewrite is therefore still visible through `mtimeNs`, while
 * a rename/add/delete changes the record's sorted key set.
 */
export async function fingerprintSourceTree(root: string): Promise<SourceFingerprint> {
  const absoluteRoot = resolve(root);
  const fingerprint: SourceFingerprint = {};

  const visit = async (directory: string): Promise<void> => {
    const entries = await readdir(directory, { withFileTypes: true });
    entries.sort((left, right) => left.name.localeCompare(right.name));
    for (const entry of entries) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) {
        const children = await readdir(path);
        if (children.length === 0) {
          const key = `${relative(absoluteRoot, path).replaceAll('\\', '/')}/`;
          fingerprint[key] = {
            kind: 'directory',
            size: '0',
            sha256: createHash('sha256').update('').digest('hex'),
            mtimeNs: '0',
          };
          continue;
        }
        await visit(path);
        continue;
      }
      const metadata = await lstat(path, { bigint: true });
      const key = relative(absoluteRoot, path).replaceAll('\\', '/');
      if (entry.isFile()) {
        const bytes = await readFile(path);
        fingerprint[key] = {
          kind: 'file',
          size: metadata.size.toString(),
          sha256: createHash('sha256').update(bytes).digest('hex'),
          mtimeNs: metadata.mtimeNs.toString(),
        };
        continue;
      }
      if (entry.isSymbolicLink()) {
        const target = await readlink(path);
        fingerprint[key] = {
          kind: 'symlink',
          size: metadata.size.toString(),
          sha256: createHash('sha256').update(target).digest('hex'),
          mtimeNs: metadata.mtimeNs.toString(),
        };
        continue;
      }
      throw new Error(`cannot fingerprint unsupported source entry: ${path}`);
    }
  };

  await visit(absoluteRoot);
  return fingerprint;
}

export interface NativeRunOutcome {
  readonly ok: boolean;
  readonly failedAt: string | null;
  readonly results: readonly unknown[];
}

export interface NativeRunSummary extends NativeRunOutcome {
  readonly artifacts: string;
  readonly error?: string;
  readonly sourceIntegrity: {
    readonly unchanged: boolean;
    readonly changed: readonly string[];
  };
}

function changedSourcePaths(
  before: SourceFingerprint,
  after: SourceFingerprint,
): string[] {
  return [...new Set([...Object.keys(before), ...Object.keys(after)])]
    .sort()
    .filter((path) => JSON.stringify(before[path]) !== JSON.stringify(after[path]));
}

async function writeJsonAtomic(path: string, value: unknown): Promise<void> {
  const temporary = `${path}.${randomUUID()}.tmp`;
  await writeFile(temporary, JSON.stringify(value, null, 2), 'utf8');
  await rename(temporary, path);
}

/**
 * Run one native flow in a unique artifact directory and always finalize its
 * source-integrity evidence. A setup/session/flow exception becomes a recorded
 * failed result after the `after` snapshot is taken; it cannot leave a fresh
 * `before` paired with stale fixed-path files from an earlier run.
 */
export async function withNativeRunArtifacts(
  base: string,
  source: string,
  run: (artifacts: string) => Promise<NativeRunOutcome>,
): Promise<NativeRunSummary> {
  const artifacts = join(
    resolve(base),
    `run-${new Date().toISOString().replaceAll(':', '-')}-${process.pid}-${randomUUID()}`,
  );
  await mkdir(artifacts, { recursive: true });
  const before = await fingerprintSourceTree(source);
  await writeJsonAtomic(join(artifacts, 'source-fingerprint-before.json'), before);

  let outcome: NativeRunOutcome = { ok: false, failedAt: 'harness-error', results: [] };
  let runError: string | undefined;
  try {
    outcome = await run(artifacts);
  } catch (cause) {
    runError = cause instanceof Error ? cause.message : String(cause);
  }

  let after: SourceFingerprint = {};
  let finalizationError: string | undefined;
  try {
    after = await fingerprintSourceTree(source);
    await writeJsonAtomic(join(artifacts, 'source-fingerprint-after.json'), after);
  } catch (cause) {
    finalizationError = cause instanceof Error ? cause.message : String(cause);
  }
  const changed = changedSourcePaths(before, after);
  const unchanged = finalizationError === undefined && changed.length === 0;
  const error = [runError, finalizationError && `source finalization failed: ${finalizationError}`]
    .filter((message): message is string => message !== undefined)
    .join('; ');
  const summary: NativeRunSummary = {
    ok: outcome.ok && unchanged,
    failedAt: unchanged ? outcome.failedAt : 'source-integrity',
    artifacts,
    sourceIntegrity: { unchanged, changed },
    results: outcome.results,
    ...(error.length === 0 ? {} : { error }),
  };
  await writeJsonAtomic(join(artifacts, 'result.json'), summary);
  return summary;
}

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
 * already reading through that lens. The Semantic view step then has nothing to
 * adopt — the sheet opens on "Reading as this" — and the run fails claiming the
 * lens never took effect, which is the opposite of what happened. It also
 * carries `inspectorOpen` and `reviewer`, which the Inspector step toggles and
 * signs.
 */
const STORE_FILES = ['session.json', 'settings.json'] as const;

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

/** A profile accepted by the native harness's fail-closed cleanup boundary. */
export interface E2EProfile {
  readonly identifier: typeof E2E_BUNDLE_IDENTIFIER;
  readonly configDir: string;
}

function isStrictDescendant(path: string, parent: string): boolean {
  const relation = relative(parent, path);
  return relation !== '' && relation !== '..' && !relation.startsWith(`..${process.platform === 'win32' ? '\\' : '/'}`) && !isAbsolute(relation);
}

function isMissing(error: unknown): boolean {
  return typeof error === 'object' && error !== null && 'code' in error && error.code === 'ENOENT';
}

async function nearestExistingAncestor(path: string): Promise<string> {
  let current = resolve(path);
  while (true) {
    try {
      await lstat(current);
      return current;
    } catch (error) {
      if (!isMissing(error)) throw error;
      const parent = dirname(current);
      if (parent === current) throw new Error(`refusing destructive seed operation without a validated E2E profile: no existing ancestor for ${path}`);
      current = parent;
    }
  }
}

/** Resolve an existing ancestor so comparisons use the filesystem's real path. */
async function canonicalPath(path: string): Promise<string> {
  const absolute = resolve(path);
  const existing = await nearestExistingAncestor(absolute);
  return resolve(await realpath(existing), relative(existing, absolute));
}

/** Reject symlinks below a trusted root while allowing platform-owned root aliases. */
async function assertNoSymlinksBelow(path: string, root: string): Promise<void> {
  const boundary = resolve(root);
  let current = resolve(path);
  while (current !== boundary) {
    if (!isStrictDescendant(current, boundary)) {
      throw new Error('refusing destructive seed operation without a validated E2E profile');
    }
    try {
      if ((await lstat(current)).isSymbolicLink()) {
        throw new Error('refusing destructive seed operation without a validated E2E profile: symlinked state path');
      }
    } catch (error) {
      if (!isMissing(error)) throw error;
    }
    current = dirname(current);
  }
}

async function assertNotSymlink(path: string): Promise<void> {
  try {
    if ((await lstat(path)).isSymbolicLink()) {
      throw new Error('refusing destructive seed operation without a validated E2E profile: symlinked state target');
    }
  } catch (error) {
    if (!isMissing(error)) throw error;
  }
}

/**
 * Produce the only profile destructive seed helpers accept. Production runs use
 * the E2E bundle's OS config directory; unit tests may use a `mkdtemp` directory
 * below the OS temp root whose name begins with `mdreader-e2e-`.
 */
export function createE2EProfile(
  identifier: string = E2E_BUNDLE_IDENTIFIER,
  configDir: string = platformConfigDir(E2E_BUNDLE_IDENTIFIER),
): E2EProfile {
  const profile = { identifier, configDir: resolve(configDir) } as E2EProfile;
  assertE2EProfile(profile);
  return profile;
}

/** Refuse non-E2E identities and broad paths before any state mutation. */
export function assertE2EProfile(profile: E2EProfile): asserts profile is E2EProfile {
  const candidate = profile as { identifier?: unknown; configDir?: unknown };
  if (candidate.identifier !== E2E_BUNDLE_IDENTIFIER || typeof candidate.configDir !== 'string') {
    throw new Error('refusing destructive seed operation without a validated E2E profile');
  }

  const configDir = resolve(candidate.configDir);
  const expected = resolve(platformConfigDir(E2E_BUNDLE_IDENTIFIER));
  const temporary = resolve(tmpdir());
  const testOwned = isStrictDescendant(configDir, temporary) && basename(configDir).startsWith('mdreader-e2e-');
  if (configDir !== expected && !testOwned) {
    throw new Error('refusing destructive seed operation without a validated E2E profile');
  }
}

/** Canonical E2E state directory, validated again immediately before mutation. */
async function validatedConfigDir(profile: E2EProfile): Promise<string> {
  assertE2EProfile(profile);
  const rawConfigDir = resolve(profile.configDir);
  const rawExpected = resolve(platformConfigDir(E2E_BUNDLE_IDENTIFIER));
  const rawTemporary = resolve(tmpdir());
  if (rawConfigDir === rawExpected) await assertNoSymlinksBelow(rawConfigDir, dirname(rawExpected));
  else if (isStrictDescendant(rawConfigDir, rawTemporary)) await assertNoSymlinksBelow(rawConfigDir, rawTemporary);
  else throw new Error('refusing destructive seed operation without a validated E2E profile');

  const configDir = await canonicalPath(rawConfigDir);
  const expected = await canonicalPath(rawExpected);
  const temporary = await canonicalPath(rawTemporary);
  const testOwned = isStrictDescendant(configDir, temporary) && basename(configDir).startsWith('mdreader-e2e-');
  if (configDir !== expected && !testOwned) {
    throw new Error('refusing destructive seed operation without a validated E2E profile');
  }
  return configDir;
}

/** Validated E2E config directory. */
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
  const dir = await validatedConfigDir(profile);
  const path = join(dir, 'session.json');
  await assertNotSymlink(path);
  await mkdir(dir, { recursive: true });
  await writeFile(path, JSON.stringify(buildSessionSeed(rootAbs), null, 2), 'utf8');
  return path;
}

/**
 * Remove every persisted store file so a run starts from a clean slate — no
 * leftover tabs/root, and no leftover UI state (see {@link STORE_FILES}).
 * Ignores files that are not there.
 */
export async function clearSeed(profile: E2EProfile): Promise<void> {
  const dir = await validatedConfigDir(profile);
  for (const file of STORE_FILES) {
    const path = join(dir, file);
    await assertNotSymlink(path);
    await rm(path, { force: true });
  }
}

/** Remove only the validated E2E profile's engine workspace state. */
export async function clearWorkspaceIndex(profile: E2EProfile): Promise<void> {
  const path = join(await validatedConfigDir(profile), 'workspaces');
  await assertNotSymlink(path);
  await rm(path, { recursive: true, force: true });
}
