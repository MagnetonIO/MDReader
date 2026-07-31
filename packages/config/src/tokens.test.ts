import { readFileSync } from 'node:fs';
import { expect, test } from 'vitest';

import { CONFIG_FILE_PATHS } from './index.js';

const tokens = readFileSync(CONFIG_FILE_PATHS.tokensCss, 'utf8');

test('backwards-compatible color variables are still defined', () => {
  expect(tokens).toContain('--color-bg:');
  expect(tokens).toContain('--color-fg:');
  expect(tokens).toContain('--color-accent:');
});

test('reading typography tokens are present', () => {
  expect(tokens).toContain('--measure: 70ch');
  expect(tokens).toContain('--line-height-body: 1.65');
});

test('font stacks lead with the required families', () => {
  // --font-ui leads with system-ui.
  expect(tokens).toMatch(/--font-ui:\s*system-ui/);
  // Reading serif leads with Source Serif 4 and falls back to a serif.
  expect(tokens).toMatch(/--font-reading-serif:\s*"Source Serif 4"[\s\S]*serif;/);
  // Mono leads with JetBrains Mono and falls back to ui-monospace.
  expect(tokens).toMatch(/--font-mono:\s*"JetBrains Mono",\s*ui-monospace/);
});

test('the -1..+4 type scale is fully defined', () => {
  for (const step of ['--text--1', '--text-0', '--text-1', '--text-2', '--text-3', '--text-4']) {
    expect(tokens).toContain(`${step}:`);
  }
});

test('a spacing scale is defined', () => {
  expect(tokens).toContain('--space-1:');
  expect(tokens).toContain('--space-8:');
});

test('both themes are expressed, including explicit [data-theme] overrides', () => {
  expect(tokens).toContain('@media (prefers-color-scheme: dark)');
  expect(tokens).toContain('[data-theme="dark"]');
  expect(tokens).toContain('[data-theme="light"]');
});
