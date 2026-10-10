/**
 * The desktop {@link Platform} the shared MarkdownReader depends on. Relative
 * asset URLs are never filesystem URLs. Bounded native image/attachment readers
 * own relative-file admission. External links open in the OS browser
 * through the native OS opener; browser hosts use a new tab.
 */

import type { Platform } from '@mdreader/ui/platform';
import { openReaderExternal } from './external-links';

/** Build a Platform bound to the directory of the currently open document. */
export function createReaderPlatform(_docDir: string | null, onFailure?: (message: string) => void): Platform {
  return {
    openExternal(url) {
      void openReaderExternal(url).catch((cause: unknown) => onFailure?.(cause instanceof Error ? cause.message : 'The link could not be opened.'));
    },
    resolveAssetUrl: () => 'about:blank',
  };
}
