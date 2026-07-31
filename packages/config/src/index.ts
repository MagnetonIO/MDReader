import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

/**
 * Typed exports of the shared config files this package publishes.
 *
 * This module exists so `@mdreader/config` has a real, buildable,
 * typecheckable, lintable, and testable source surface (matching the other
 * workspace packages) in addition to the raw config files it re-exports via
 * subpath exports (`./eslint`, `./vitest`, `./tailwind-preset`, etc.).
 */

// Resolved relative to this module's location: at build time it lives in
// `dist/`, at test time (Vitest running the TS source directly) it lives in
// `src/`. Either way, one directory up is the package root, where the actual
// config files live.
const packageRoot = join(dirname(fileURLToPath(import.meta.url)), '..');

/** File names of the shared config files, relative to the package root. */
export const CONFIG_FILE_NAMES = {
  tokensCss: 'tokens.css',
  tailwindPresetCss: 'tailwind-preset.css',
  eslintConfig: 'eslint.config.js',
  vitestBase: 'vitest.base.js',
  tsconfigBase: 'tsconfig.base.json',
} as const;

export type ConfigFileKey = keyof typeof CONFIG_FILE_NAMES;

/** Absolute paths to the shared config files, resolved from the package root. */
export const CONFIG_FILE_PATHS: Record<ConfigFileKey, string> = {
  tokensCss: join(packageRoot, CONFIG_FILE_NAMES.tokensCss),
  tailwindPresetCss: join(packageRoot, CONFIG_FILE_NAMES.tailwindPresetCss),
  eslintConfig: join(packageRoot, CONFIG_FILE_NAMES.eslintConfig),
  vitestBase: join(packageRoot, CONFIG_FILE_NAMES.vitestBase),
  tsconfigBase: join(packageRoot, CONFIG_FILE_NAMES.tsconfigBase),
};
