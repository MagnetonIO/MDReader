/**
 * The desktop {@link Platform} the shared MarkdownReader depends on. Relative
 * asset URLs (image `src`, relative links) resolve against the active document's
 * directory and are converted to a webview-loadable URL with Tauri's
 * `convertFileSrc` (core API — no plugin). External links open in the OS browser
 * through the native OS opener; browser hosts use a new tab.
 */

import { convertFileSrc } from '@tauri-apps/api/core';

import type { Platform } from '@mdreader/ui/platform';
import { openReaderExternal } from './external-links';

function isAbsoluteUrl(url: string): boolean {
  return /^[a-z]+:\/\//i.test(url) || url.startsWith('data:') || url.startsWith('#');
}

function joinPath(dir: string, rel: string): string {
  const base = dir.replace(/[\\/]+$/, '');
  const parts = rel.split(/[\\/]+/);
  const stack = base.split(/[\\/]+/);
  for (const part of parts) {
    if (part === '' || part === '.') continue;
    if (part === '..') stack.pop();
    else stack.push(part);
  }
  return stack.join('/');
}

/** Build a Platform bound to the directory of the currently open document. */
export function createReaderPlatform(docDir: string | null, onFailure?: (message: string) => void): Platform {
  return {
    openExternal(url) {
      void openReaderExternal(url).catch((cause: unknown) => onFailure?.(cause instanceof Error ? cause.message : 'The link could not be opened.'));
    },
    resolveAssetUrl(url) {
      if (isAbsoluteUrl(url)) return url;
      const absolute = docDir ? joinPath(docDir, url) : url;
      try {
        return convertFileSrc(absolute);
      } catch {
        return absolute;
      }
    },
  };
}
