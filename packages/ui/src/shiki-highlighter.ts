import type { Highlighter } from 'shiki';
import type { Root as HastRoot } from 'hast';

/**
 * The theme pair used for syntax highlighting. Rendered as a dual-theme so the
 * output carries both light and dark colors and switches with the reader's
 * theme (the CSS-variable output is toggled by `prefers-color-scheme` /
 * `[data-theme]`).
 */
export const SHIKI_THEMES = { light: 'github-light', dark: 'github-dark' } as const;

let highlighterPromise: Promise<Highlighter> | null = null;

/**
 * Lazily create (once) and return the shared Shiki highlighter. Languages are
 * loaded on demand; only the two themes are eagerly registered.
 */
async function getHighlighter(): Promise<Highlighter> {
  if (!highlighterPromise) {
    highlighterPromise = import('shiki').then((shiki) =>
      shiki.createHighlighter({
        themes: [SHIKI_THEMES.light, SHIKI_THEMES.dark],
        langs: [],
      }),
    );
  }
  return highlighterPromise;
}

/** Whether Shiki bundles a grammar for the given language id. */
async function isKnownLanguage(lang: string): Promise<boolean> {
  const { bundledLanguages } = await import('shiki');
  return Object.prototype.hasOwnProperty.call(bundledLanguages, lang);
}

/**
 * Highlight a code string to a hast tree using the shared highlighter. Unknown
 * or unspecified languages fall back to plain text. Never throws — on any
 * failure it returns null so the caller can keep the plain `<pre>`.
 */
export async function highlightToHast(code: string, lang: string): Promise<HastRoot | null> {
  try {
    const normalized = lang && (await isKnownLanguage(lang)) ? lang : 'text';
    const highlighter = await getHighlighter();
    if (normalized !== 'text' && !highlighter.getLoadedLanguages().includes(normalized)) {
      await highlighter.loadLanguage(
        normalized as Parameters<Highlighter['loadLanguage']>[0],
      );
    }
    return highlighter.codeToHast(code, {
      lang: normalized,
      themes: SHIKI_THEMES,
      defaultColor: false,
    });
  } catch {
    return null;
  }
}
