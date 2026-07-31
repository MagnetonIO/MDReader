import js from '@eslint/js';
import globals from 'globals';
import tseslint from 'typescript-eslint';

// Shared ESLint 9 flat config for all @mdreader packages.
export default tseslint.config(
  {
    ignores: [
      '**/dist/**',
      '**/.next/**',
      '**/.turbo/**',
      '**/node_modules/**',
      '**/next-env.d.ts',
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    languageOptions: {
      globals: {
        ...globals.browser,
        ...globals.node,
      },
    },
    rules: {
      // TypeScript's own checker handles undefined identifiers.
      'no-undef': 'off',
    },
  },
);
